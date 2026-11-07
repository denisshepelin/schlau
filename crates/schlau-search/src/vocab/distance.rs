//! Bounded edit distance (optimal string alignment: insertions, deletions, substitutions and adjacent
//! transpositions, each costing 1) and the character signatures that prefilter candidates for it.

/// Longest term, in characters, that takes part in typo correction. Longer tokens (hashes, URLs) only match
/// exactly or by prefix.
pub(crate) const MAX_CORRECTABLE: usize = 48;

const WIDTH: usize = MAX_CORRECTABLE + 2;

/// The optimal string alignment distance between `a` and `b` if it is at most `k`, computed only along the
/// diagonal band of width `2k + 1` and abandoned as soon as a whole row exceeds `k`.
///
/// Both inputs must be at most `MAX_CORRECTABLE` long and `k` at most 3.
pub(crate) fn osa_within<T: Eq + Copy>(a: &[T], b: &[T], k: usize) -> Option<u8> {
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > k || n > MAX_CORRECTABLE || m > MAX_CORRECTABLE {
        return None;
    }
    let inf = k as u8 + 1;
    let mut rows = [[inf; WIDTH]; 3];
    (0..=m.min(k)).for_each(|j| rows[0][j] = j as u8);
    for i in 1..=n {
        let [cur, prev, prev2] = rotate(&mut rows, i);
        let lo = i.saturating_sub(k).max(1);
        let hi = m.min(i + k);
        cur[lo - 1] = if lo == 1 && i <= k { i as u8 } else { inf };
        let mut row_min = cur[lo - 1];
        for j in lo..=hi {
            let cost = u8::from(a[i - 1] != b[j - 1]);
            let mut v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(prev2[j - 2] + 1);
            }
            cur[j] = v.min(inf);
            row_min = row_min.min(cur[j]);
        }
        if hi < m {
            cur[hi + 1] = inf;
        }
        if row_min > k as u8 {
            return None;
        }
    }
    let last = rows[n % 3][m];
    (last <= k as u8).then_some(last)
}

/// Row `i` (current), `i - 1` and `i - 2` of a three-row rolling table.
fn rotate(rows: &mut [[u8; WIDTH]; 3], i: usize) -> [&mut [u8; WIDTH]; 3] {
    let [r0, r1, r2] = rows;
    match i % 3 {
        0 => [r0, r2, r1],
        1 => [r1, r0, r2],
        _ => [r2, r1, r0],
    }
}

/// A 64-bit signature of the characters of a term: one bit per ASCII letter present, one per digit present,
/// one per ASCII letter present at least twice, and other characters hashed into those last 26 bits.
///
/// Each bit is an OR of features (`c` occurs at least once, `c` occurs at least twice). An edit changes one
/// character count (two for a substitution) by one, which flips at most one feature per count, so terms
/// within `k` edits differ in at most `2k` bits.
pub(crate) fn signature(chars: impl Iterator<Item = char>) -> u64 {
    let mut counts = [0u8; 26];
    let other = chars.fold(0u64, |sig, c| match c {
        'a'..='z' => {
            let n = &mut counts[c as usize - 'a' as usize];
            *n = n.saturating_add(1);
            sig
        }
        '0'..='9' => sig | 1u64 << (26 + (c as u32 - '0' as u32)),
        _ => sig | 1u64 << (36 + (c as u32).wrapping_mul(2_654_435_761) % 28),
    });
    counts.iter().enumerate().fold(other, |sig, (i, &n)| {
        let once = if n >= 1 { 1u64 << i } else { 0 };
        let twice = if n >= 2 { 1u64 << (36 + i) } else { 0 };
        sig | once | twice
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(a: &str, b: &str, k: usize) -> Option<u8> {
        osa_within(a.as_bytes(), b.as_bytes(), k)
    }

    #[test]
    fn identical() {
        assert_eq!(d("receive", "receive", 2), Some(0));
        assert_eq!(d("", "", 1), Some(0));
    }

    #[test]
    fn single_edits() {
        assert_eq!(d("recieve", "receive", 1), Some(1));
        assert_eq!(d("invocie", "invoice", 1), Some(1));
        assert_eq!(d("shcema", "schema", 1), Some(1));
        assert_eq!(d("invoce", "invoice", 1), Some(1));
        assert_eq!(d("invoicee", "invoice", 1), Some(1));
        assert_eq!(d("invoixe", "invoice", 1), Some(1));
    }

    #[test]
    fn two_edits() {
        assert_eq!(d("acommodate", "accommodate", 2), Some(1));
        assert_eq!(d("acomodate", "accommodate", 2), Some(2));
        assert_eq!(d("acomodate", "accommodate", 1), None);
    }

    #[test]
    fn beyond_budget() {
        assert_eq!(d("invoice", "receipt", 2), None);
        assert_eq!(d("abc", "abcdef", 2), None);
    }

    #[test]
    fn edges() {
        assert_eq!(d("a", "", 1), Some(1));
        assert_eq!(d("", "ab", 2), Some(2));
        assert_eq!(d("ab", "ba", 1), Some(1));
        assert_eq!(d("abcd", "badc", 2), Some(2));
    }

    #[test]
    fn matches_full_dp() {
        fn full(a: &[u8], b: &[u8]) -> usize {
            let (n, m) = (a.len(), b.len());
            let mut t = vec![vec![0usize; m + 1]; n + 1];
            (0..=n).for_each(|i| t[i][0] = i);
            (0..=m).for_each(|j| t[0][j] = j);
            for i in 1..=n {
                for j in 1..=m {
                    let cost = usize::from(a[i - 1] != b[j - 1]);
                    t[i][j] = (t[i - 1][j] + 1)
                        .min(t[i][j - 1] + 1)
                        .min(t[i - 1][j - 1] + cost);
                    if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                        t[i][j] = t[i][j].min(t[i - 2][j - 2] + 1);
                    }
                }
            }
            t[n][m]
        }
        let words = [
            "", "a", "ab", "ba", "abc", "acb", "bca", "abcd", "abdc", "xbcd", "abcde", "aabb",
            "baba",
        ];
        words.iter().for_each(|a| {
            words.iter().for_each(|b| {
                let exact = full(a.as_bytes(), b.as_bytes());
                (0..=3).for_each(|k| {
                    let got = osa_within(a.as_bytes(), b.as_bytes(), k);
                    let want = (exact <= k).then_some(exact as u8);
                    assert_eq!(got, want, "{a} {b} k={k}");
                });
            })
        });
    }

    #[test]
    fn signature_bound_holds() {
        let pairs = [
            ("recieve", "receive"),
            ("invoice", "invoixe"),
            ("schema", "shcema"),
            ("café", "cafe"),
        ];
        pairs.iter().for_each(|(a, b)| {
            let x = signature(a.chars()) ^ signature(b.chars());
            assert!(x.count_ones() <= 2, "{a} {b}");
        });
    }
}
