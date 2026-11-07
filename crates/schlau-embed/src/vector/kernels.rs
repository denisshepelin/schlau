//! Dot-product kernels written so LLVM vectorizes them (NEON on Apple Silicon,
//! AVX2/SSE on x86): every reduction keeps `LANES` independent accumulators, so
//! no floating-point reassociation is needed.

use half::f16;
use half::slice::HalfFloatSliceExt;

const LANES: usize = 16;

/// Rows converted from f16 per block. 64 rows x 256 dims x 4 bytes = 64 KiB.
const F16_BLOCK_ROWS: usize = 64;

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn fmadd(a: f32, b: f32, acc: f32) -> f32 {
    a.mul_add(b, acc)
}

#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
fn fmadd(a: f32, b: f32, acc: f32) -> f32 {
    acc + a * b
}

/// Dot product of two equally long f32 slices.
#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let (a_chunks, a_rest) = a.as_chunks::<LANES>();
    let (b_chunks, b_rest) = b.as_chunks::<LANES>();
    let lanes = a_chunks
        .iter()
        .zip(b_chunks)
        .fold([0.0f32; LANES], |mut acc, (x, y)| {
            for i in 0..LANES {
                acc[i] = fmadd(x[i], y[i], acc[i]);
            }
            acc
        });
    lanes.iter().sum::<f32>() + a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum::<f32>()
}

#[inline]
fn dot_i8(query: &[f32], row: &[i8]) -> f32 {
    debug_assert_eq!(query.len(), row.len());
    let (q_chunks, q_rest) = query.as_chunks::<LANES>();
    let (r_chunks, r_rest) = row.as_chunks::<LANES>();
    let lanes = q_chunks
        .iter()
        .zip(r_chunks)
        .fold([0.0f32; LANES], |mut acc, (q, r)| {
            for i in 0..LANES {
                acc[i] = fmadd(q[i], f32::from(r[i]), acc[i]);
            }
            acc
        });
    lanes.iter().sum::<f32>()
        + q_rest
            .iter()
            .zip(r_rest)
            .map(|(q, &r)| q * f32::from(r))
            .sum::<f32>()
}

/// Score `rows` (row-major f16 matrix with `query.len()` columns) into `out`.
pub(super) fn score_f16(query: &[f32], rows: &[f16], out: &mut [f32]) {
    let dim = query.len();
    let mut scratch = vec![0.0f32; F16_BLOCK_ROWS * dim];
    rows.chunks(F16_BLOCK_ROWS * dim)
        .zip(out.chunks_mut(F16_BLOCK_ROWS))
        .for_each(|(block, scores)| {
            let floats = &mut scratch[..block.len()];
            block.convert_to_f32_slice(floats);
            floats
                .chunks_exact(dim)
                .zip(scores.iter_mut())
                .for_each(|(row, score)| *score = dot(query, row));
        });
}

/// Score an int8 matrix with one dequantization scale per row into `out`.
pub(super) fn score_i8(query: &[f32], rows: &[i8], scales: &[f32], out: &mut [f32]) {
    rows.chunks_exact(query.len())
        .zip(scales)
        .zip(out.iter_mut())
        .for_each(|((row, &scale), score)| *score = scale * dot_i8(query, row));
}

/// Symmetric per-row int8 quantization: `row ~= scale * q` with `q` in [-127, 127].
pub(super) fn quantize_i8(row: &[f32]) -> (Vec<i8>, f32) {
    let max = row.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if max == 0.0 {
        return (vec![0; row.len()], 0.0);
    }
    let scale = max / 127.0;
    let quantized = row
        .iter()
        .map(|x| (x / scale).round().clamp(-127.0, 127.0) as i8)
        .collect();
    (quantized, scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    fn series(n: usize, k: f32) -> Vec<f32> {
        (0..n).map(|i| ((i as f32) * k).cos()).collect()
    }

    #[test]
    fn dot_matches_naive_for_odd_lengths() {
        for n in [0, 1, 15, 16, 17, 255, 256, 300] {
            let (a, b) = (series(n, 0.1), series(n, 0.7));
            assert!((dot(&a, &b) - naive(&a, &b)).abs() < 1e-3, "n = {n}");
        }
    }

    #[test]
    fn i8_quantization_round_trips() {
        let row = series(256, 0.3);
        let (q, scale) = quantize_i8(&row);
        assert!(
            row.iter()
                .zip(&q)
                .all(|(x, &v)| (x - scale * f32::from(v)).abs() <= scale)
        );
        let query = series(256, 0.11);
        let mut out = [0.0];
        score_i8(&query, &q, &[scale], &mut out);
        assert!((out[0] - naive(&query, &row)).abs() < 0.05);
        assert_eq!(quantize_i8(&[0.0, 0.0]), (vec![0, 0], 0.0));
    }

    #[test]
    fn f16_blocks_cover_partial_tail() {
        let dim = 20;
        let n = F16_BLOCK_ROWS + 3;
        let rows: Vec<f32> = series(n * dim, 0.05);
        let halves: Vec<f16> = rows.iter().map(|&x| f16::from_f32(x)).collect();
        let query = series(dim, 0.9);
        let mut out = vec![0.0; n];
        score_f16(&query, &halves, &mut out);
        rows.chunks_exact(dim)
            .zip(&out)
            .for_each(|(row, &s)| assert!((s - naive(&query, row)).abs() < 1e-2));
    }
}
