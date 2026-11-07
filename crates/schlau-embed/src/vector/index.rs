use half::f16;
use rayon::prelude::*;

use super::f16_from_le_bytes;
use super::kernels::{quantize_i8, score_f16, score_i8};
use super::rank::{ScoreStats, top_k};
use crate::{Error, Result};

/// Document identifier, the store's SQLite rowid.
pub type DocId = i64;

/// Rows per parallel task. Below `2 * PAR_CHUNK_ROWS` rows scoring stays on the
/// calling thread.
const PAR_CHUNK_ROWS: usize = 8192;

/// In-memory element type of a [`VectorIndex`].
///
/// - `F16`: 2 bytes per dimension, scores within ~1e-3 of exact f32 cosine.
/// - `I8`: 1 byte per dimension plus one f32 scale per row; half the memory and
///   bandwidth of `F16`, scores within ~1e-2; recall@10 against exact f32 was
///   0.99 vs 1.00 for `F16` on 200k clustered vectors (`examples/bench.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Precision {
    #[default]
    F16,
    I8,
}

#[derive(Debug, Clone)]
enum Matrix {
    F16(Vec<f16>),
    I8 { rows: Vec<i8>, scales: Vec<f32> },
}

/// A scored document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub id: DocId,
    pub score: f32,
}

/// All document vectors in one contiguous row-major matrix, scored by brute
/// force. Exact (up to storage precision), no index build, and fast enough for a
/// personal mailbox: 200k x 256 dims is ~100 MB of f16 read once per query.
#[derive(Debug, Clone)]
pub struct VectorIndex {
    dim: usize,
    ids: Vec<DocId>,
    matrix: Matrix,
}

impl VectorIndex {
    pub fn new(dim: usize, precision: Precision) -> Self {
        Self::with_capacity(dim, precision, 0)
    }

    pub fn with_capacity(dim: usize, precision: Precision, rows: usize) -> Self {
        let matrix = match precision {
            Precision::F16 => Matrix::F16(Vec::with_capacity(rows * dim)),
            Precision::I8 => Matrix::I8 {
                rows: Vec::with_capacity(rows * dim),
                scales: Vec::with_capacity(rows),
            },
        };
        Self {
            dim,
            ids: Vec::with_capacity(rows),
            matrix,
        }
    }

    /// Build from `(id, blob)` pairs as stored on disk ([`super::pack_f16`]).
    pub fn from_f16_blobs<'a>(
        dim: usize,
        precision: Precision,
        blobs: impl IntoIterator<Item = (DocId, &'a [u8])>,
    ) -> Result<Self> {
        blobs
            .into_iter()
            .try_fold(Self::new(dim, precision), |mut index, (id, blob)| {
                index.push_f16_blob(id, blob)?;
                Ok(index)
            })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn ids(&self) -> &[DocId] {
        &self.ids
    }

    pub fn precision(&self) -> Precision {
        match self.matrix {
            Matrix::F16(_) => Precision::F16,
            Matrix::I8 { .. } => Precision::I8,
        }
    }

    /// Bytes held by the vector matrix.
    pub fn memory_bytes(&self) -> usize {
        match &self.matrix {
            Matrix::F16(rows) => rows.len() * 2,
            Matrix::I8 { rows, scales } => rows.len() + scales.len() * 4,
        }
    }

    fn check_dim(&self, actual: usize) -> Result<()> {
        if actual == self.dim {
            Ok(())
        } else {
            Err(Error::Dimension {
                expected: self.dim,
                actual,
            })
        }
    }

    /// Append a vector. Ids are not deduplicated; call [`Self::remove`] first to
    /// replace one.
    pub fn push(&mut self, id: DocId, vector: &[f32]) -> Result<()> {
        self.check_dim(vector.len())?;
        match &mut self.matrix {
            Matrix::F16(rows) => rows.extend(vector.iter().map(|&x| f16::from_f32(x))),
            Matrix::I8 { rows, scales } => {
                let (quantized, scale) = quantize_i8(vector);
                rows.extend(quantized);
                scales.push(scale);
            }
        }
        self.ids.push(id);
        Ok(())
    }

    /// Append a vector stored as a little-endian f16 blob.
    pub fn push_f16_blob(&mut self, id: DocId, blob: &[u8]) -> Result<()> {
        let halves = f16_from_le_bytes(blob)?;
        self.check_dim(halves.len())?;
        match &mut self.matrix {
            Matrix::F16(rows) => {
                rows.extend(halves);
                self.ids.push(id);
                Ok(())
            }
            Matrix::I8 { .. } => {
                let floats: Vec<f32> = halves.iter().map(|h| h.to_f32()).collect();
                self.push(id, &floats)
            }
        }
    }

    /// Remove every row with `id`; returns whether any was removed. Moves the
    /// last row into the freed slot, so row order is not preserved.
    pub fn remove(&mut self, id: DocId) -> bool {
        let before = self.len();
        while let Some(row) = self.ids.iter().rposition(|&x| x == id) {
            self.swap_remove_row(row);
        }
        self.len() != before
    }

    fn swap_remove_row(&mut self, row: usize) {
        let dim = self.dim;
        let last = self.len() - 1;
        self.ids.swap_remove(row);
        match &mut self.matrix {
            Matrix::F16(rows) => swap_remove_row(rows, row, last, dim),
            Matrix::I8 { rows, scales } => {
                swap_remove_row(rows, row, last, dim);
                scales.swap_remove(row);
            }
        }
    }

    /// Dot product of `query` with every row, in row order (aligned with
    /// [`Self::ids`]). With unit-length vectors this is cosine similarity.
    pub fn score_all(&self, query: &[f32]) -> Result<Vec<f32>> {
        self.check_dim(query.len())?;
        let mut scores = vec![0.0f32; self.len()];
        if self.dim == 0 {
            return Ok(scores);
        }
        let dim = self.dim;
        let parallel = self.len() >= 2 * PAR_CHUNK_ROWS;
        match &self.matrix {
            Matrix::F16(rows) if parallel => scores
                .par_chunks_mut(PAR_CHUNK_ROWS)
                .zip(rows.par_chunks(PAR_CHUNK_ROWS * dim))
                .for_each(|(out, rows)| score_f16(query, rows, out)),
            Matrix::F16(rows) => score_f16(query, rows, &mut scores),
            Matrix::I8 { rows, scales } if parallel => scores
                .par_chunks_mut(PAR_CHUNK_ROWS)
                .zip(rows.par_chunks(PAR_CHUNK_ROWS * dim))
                .zip(scales.par_chunks(PAR_CHUNK_ROWS))
                .for_each(|((out, rows), scales)| score_i8(query, rows, scales, out)),
            Matrix::I8 { rows, scales } => score_i8(query, rows, scales, &mut scores),
        }
        Ok(scores)
    }

    /// The `k` best documents for `query`, best first.
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<Hit>> {
        Ok(self.hits(&self.score_all(query)?, k))
    }

    /// The `k` best documents plus the statistics of all scores, for z-score fusion.
    pub fn search_with_stats(&self, query: &[f32], k: usize) -> Result<(Vec<Hit>, ScoreStats)> {
        let scores = self.score_all(query)?;
        Ok((self.hits(&scores, k), ScoreStats::of(&scores)))
    }

    fn hits(&self, scores: &[f32], k: usize) -> Vec<Hit> {
        top_k(scores, k)
            .into_iter()
            .map(|(row, score)| Hit {
                id: self.ids[row],
                score,
            })
            .collect()
    }
}

fn swap_remove_row<T: Copy>(data: &mut Vec<T>, row: usize, last: usize, dim: usize) {
    if row != last {
        data.copy_within(last * dim..(last + 1) * dim, row * dim);
    }
    data.truncate(last * dim);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::{dot, l2_normalized, pack_f16};

    fn pseudo_random(seed: u64, n: usize) -> Vec<f32> {
        (0..n)
            .scan(
                seed.wrapping_mul(6364136223846793005).wrapping_add(1),
                |s, _| {
                    *s = s
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    Some(((*s >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0)
                },
            )
            .collect()
    }

    fn corpus(n: usize, dim: usize) -> Vec<Vec<f32>> {
        (0..n)
            .map(|i| l2_normalized(&pseudo_random(i as u64, dim)))
            .collect()
    }

    fn build(vectors: &[Vec<f32>], precision: Precision) -> VectorIndex {
        let dim = vectors.first().map_or(0, Vec::len);
        vectors
            .iter()
            .enumerate()
            .fold(VectorIndex::new(dim, precision), |mut index, (i, v)| {
                index.push(i as DocId * 10, v).unwrap();
                index
            })
    }

    fn check_against_naive(n: usize, precision: Precision, tolerance: f32) {
        let dim = 64;
        let vectors = corpus(n, dim);
        let index = build(&vectors, precision);
        let query = l2_normalized(&pseudo_random(9999, dim));
        let scores = index.score_all(&query).unwrap();
        let naive: Vec<f32> = vectors.iter().map(|v| dot(&query, v)).collect();
        assert_eq!(scores.len(), n);
        scores
            .iter()
            .zip(&naive)
            .for_each(|(s, e)| assert!((s - e).abs() < tolerance, "{s} vs {e}"));

        let hits = index.search(&query, 10).unwrap();
        let expected = top_k(&naive, 10);
        let overlap = hits
            .iter()
            .filter(|h| expected.iter().any(|&(row, _)| h.id == row as DocId * 10))
            .count();
        assert!(overlap >= 9, "overlap {overlap}");
        assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn f16_scores_match_naive_serial_and_parallel() {
        check_against_naive(1000, Precision::F16, 2e-3);
        check_against_naive(3 * PAR_CHUNK_ROWS + 17, Precision::F16, 2e-3);
    }

    #[test]
    fn i8_scores_match_naive_serial_and_parallel() {
        check_against_naive(1000, Precision::I8, 2e-2);
        check_against_naive(3 * PAR_CHUNK_ROWS + 17, Precision::I8, 2e-2);
    }

    #[test]
    fn blobs_dimension_and_removal() {
        let vectors = corpus(5, 8);
        let blobs: Vec<Vec<u8>> = vectors.iter().map(|v| pack_f16(v)).collect();
        for precision in [Precision::F16, Precision::I8] {
            let mut index = VectorIndex::from_f16_blobs(
                8,
                precision,
                blobs
                    .iter()
                    .enumerate()
                    .map(|(i, b)| (i as DocId, b.as_slice())),
            )
            .unwrap();
            assert_eq!(index.len(), 5);
            assert!(matches!(
                index.push(9, &[0.0; 3]),
                Err(Error::Dimension {
                    expected: 8,
                    actual: 3
                })
            ));
            assert!(index.remove(1));
            assert!(!index.remove(1));
            assert_eq!(index.ids(), &[0, 4, 2, 3]);
            let hit = index.search(&vectors[4], 1).unwrap()[0];
            assert_eq!(hit.id, 4);
            assert!((hit.score - 1.0).abs() < 1e-2);
            assert!(index.remove(3));
            assert_eq!(index.ids(), &[0, 4, 2]);
            assert_eq!(index.score_all(&vectors[0]).unwrap().len(), 3);
        }
    }

    #[test]
    fn stats_cover_all_rows() {
        let index = build(&corpus(500, 32), Precision::F16);
        let query = l2_normalized(&pseudo_random(1, 32));
        let (hits, stats) = index.search_with_stats(&query, 3).unwrap();
        let scores = index.score_all(&query).unwrap();
        assert_eq!(stats, ScoreStats::of(&scores));
        assert!(stats.z(hits[0].score) > 2.0);
        assert!(
            VectorIndex::new(4, Precision::F16)
                .search(&[0.0; 4], 5)
                .unwrap()
                .is_empty()
        );
    }
}
