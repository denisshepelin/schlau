use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

/// Mean and population standard deviation of a query's scores over the whole
/// corpus. Hybrid ranking compares meaning hits to keyword hits by z-score:
/// `z = (cos - mean) / std`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreStats {
    pub mean: f32,
    pub std: f32,
}

impl ScoreStats {
    pub fn of(scores: &[f32]) -> Self {
        if scores.is_empty() {
            return Self {
                mean: 0.0,
                std: 0.0,
            };
        }
        let n = scores.len() as f64;
        let (sum, sum_sq) = scores.iter().fold((0.0f64, 0.0f64), |(s, sq), &x| {
            let x = f64::from(x);
            (s + x, sq + x * x)
        });
        let mean = sum / n;
        let variance = (sum_sq / n - mean * mean).max(0.0);
        Self {
            mean: mean as f32,
            std: variance.sqrt() as f32,
        }
    }

    /// Standard score of `score`; 0 when all scores are equal.
    pub fn z(&self, score: f32) -> f32 {
        if self.std > f32::EPSILON {
            (score - self.mean) / self.std
        } else {
            0.0
        }
    }
}

#[derive(PartialEq)]
struct Ranked(f32, usize);

impl Eq for Ranked {}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .total_cmp(&other.0)
            .then_with(|| other.1.cmp(&self.1))
    }
}

/// Indices and scores of the `k` highest scores, best first. Ties are broken by
/// the lower index. NaN scores rank below every number.
pub fn top_k(scores: &[f32], k: usize) -> Vec<(usize, f32)> {
    if k == 0 {
        return Vec::new();
    }
    let sanitize = |s: f32| if s.is_nan() { f32::NEG_INFINITY } else { s };
    let heap =
        scores
            .iter()
            .enumerate()
            .fold(BinaryHeap::with_capacity(k + 1), |mut heap, (i, &s)| {
                let item = Reverse(Ranked(sanitize(s), i));
                if heap.len() < k {
                    heap.push(item);
                } else if heap.peek().is_some_and(|worst| item < *worst) {
                    heap.pop();
                    heap.push(item);
                }
                heap
            });
    heap.into_sorted_vec()
        .into_iter()
        .map(|Reverse(Ranked(_, i))| (i, scores[i]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive_top_k(scores: &[f32], k: usize) -> Vec<(usize, f32)> {
        let mut all: Vec<(usize, f32)> = scores.iter().copied().enumerate().collect();
        all.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        all.truncate(k);
        all
    }

    #[test]
    fn top_k_matches_sort() {
        let scores: Vec<f32> = (0..1000)
            .map(|i| ((i * 7919) % 1009) as f32 / 1009.0)
            .collect();
        for k in [0, 1, 5, 100, 1000, 5000] {
            assert_eq!(top_k(&scores, k), naive_top_k(&scores, k), "k = {k}");
        }
    }

    #[test]
    fn top_k_ties_and_nan() {
        let scores = [0.5, f32::NAN, 0.9, 0.5, 0.9];
        assert_eq!(top_k(&scores, 3), vec![(2, 0.9), (4, 0.9), (0, 0.5)]);
        assert_eq!(top_k(&scores, 5)[4].0, 1);
    }

    #[test]
    fn stats_and_z() {
        let stats = ScoreStats::of(&[1.0, 2.0, 3.0, 4.0]);
        assert!((stats.mean - 2.5).abs() < 1e-6);
        assert!((stats.std - 1.25f32.sqrt()).abs() < 1e-6);
        assert!((stats.z(4.0) - 1.5 / 1.25f32.sqrt()).abs() < 1e-5);
        assert_eq!(ScoreStats::of(&[0.3; 5]).z(0.9), 0.0);
        assert_eq!(
            ScoreStats::of(&[]),
            ScoreStats {
                mean: 0.0,
                std: 0.0
            }
        );
    }
}
