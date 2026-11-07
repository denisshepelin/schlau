//! Vector utilities shared by the embedder and the store.

mod index;
mod kernels;
mod rank;

use half::f16;
use half::slice::HalfFloatSliceExt;

use crate::{Error, Result};

pub use index::{DocId, Hit, Precision, VectorIndex};
pub use kernels::dot;
pub use rank::{ScoreStats, top_k};

/// Encode a vector as little-endian IEEE 754 half floats (2 bytes per dimension),
/// the format vectors are stored in on disk.
pub fn pack_f16(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|&x| f16::from_f32(x).to_le_bytes())
        .collect()
}

/// Decode a blob written by [`pack_f16`].
pub fn unpack_f16(blob: &[u8]) -> Result<Vec<f32>> {
    let halves = f16_from_le_bytes(blob)?;
    let mut out = vec![0.0; halves.len()];
    halves.convert_to_f32_slice(&mut out);
    Ok(out)
}

pub(crate) fn f16_from_le_bytes(blob: &[u8]) -> Result<Vec<f16>> {
    let (pairs, rest) = blob.as_chunks::<2>();
    if rest.is_empty() {
        Ok(pairs.iter().map(|&pair| f16::from_le_bytes(pair)).collect())
    } else {
        Err(Error::Blob(blob.len()))
    }
}

/// Euclidean norm.
pub fn l2_norm(vector: &[f32]) -> f32 {
    dot(vector, vector).sqrt()
}

/// Scale `vector` to unit length in place. Zero vectors are left unchanged.
pub fn l2_normalize(vector: &mut [f32]) {
    let norm = l2_norm(vector);
    if norm > f32::EPSILON {
        vector.iter_mut().for_each(|x| *x /= norm);
    }
}

/// Unit-length copy of `vector`.
pub fn l2_normalized(vector: &[f32]) -> Vec<f32> {
    let mut out = vector.to_vec();
    l2_normalize(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_round_trip_is_close() {
        let v: Vec<f32> = (0..256).map(|i| ((i as f32) * 0.37).sin()).collect();
        let blob = pack_f16(&v);
        assert_eq!(blob.len(), 512);
        let back = unpack_f16(&blob).unwrap();
        assert_eq!(back.len(), v.len());
        assert!(v.iter().zip(&back).all(|(a, b)| (a - b).abs() <= 1e-3));
    }

    #[test]
    fn f16_layout_is_little_endian() {
        assert_eq!(pack_f16(&[1.0, -2.0]), vec![0x00, 0x3c, 0x00, 0xc0]);
    }

    #[test]
    fn odd_blob_is_rejected() {
        assert!(matches!(unpack_f16(&[0, 0, 0]), Err(Error::Blob(3))));
        assert_eq!(unpack_f16(&[]).unwrap(), Vec::<f32>::new());
    }

    #[test]
    fn normalization() {
        let v = l2_normalized(&[3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
        assert!((l2_norm(&v) - 1.0).abs() < 1e-6);
        let mut zero = vec![0.0; 4];
        l2_normalize(&mut zero);
        assert_eq!(zero, vec![0.0; 4]);
    }
}
