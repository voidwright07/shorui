//! Fuzzy matching for the command palette.
//!
//! Subsequence matching with a score that favours prefixes, word starts and runs, which is
//! what makes "comp" rank Compress above "archival compliance".

/// A match: its score (higher is better) and the byte ranges of `haystack` that matched.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub score: i32,
    pub ranges: Vec<(usize, usize)>,
}

/// Match `needle` against `haystack`, ignoring case. `None` when the letters of `needle` do
/// not all appear in order. An empty needle matches everything with score 0.
pub fn fuzzy(needle: &str, haystack: &str) -> Option<Match> {
    let n: Vec<char> = needle.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect();
    if n.is_empty() {
        return Some(Match { score: 0, ranges: Vec::new() });
    }
    let h: Vec<(usize, char)> = haystack.char_indices().collect();
    let lower: Vec<char> = h.iter().map(|(_, c)| c.to_lowercase().next().unwrap_or(*c)).collect();
    if n.len() > h.len() {
        return None;
    }

    // Best score ending with needle[i] matched at haystack[j], by dynamic programming.
    const NONE: i32 = i32::MIN / 2;
    let mut score = vec![vec![NONE; h.len()]; n.len()];
    let mut from = vec![vec![usize::MAX; h.len()]; n.len()];
    let bonus = |j: usize| -> i32 {
        if j == 0 {
            return 16;
        }
        let prev = h[j - 1].1;
        let cur = h[j].1;
        if !prev.is_alphanumeric() {
            10
        } else if prev.is_lowercase() && cur.is_uppercase() {
            8
        } else {
            0
        }
    };
    for j in 0..h.len() {
        if lower[j] == n[0] {
            score[0][j] = 10 + bonus(j) - (j as i32).min(12);
        }
    }
    for i in 1..n.len() {
        for j in i..h.len() {
            if lower[j] != n[i] {
                continue;
            }
            // Inputs are a few dozen characters at most, so the quadratic scan is fine.
            for k in (i - 1)..j {
                let prev = score[i - 1][k];
                if prev <= NONE {
                    continue;
                }
                let gap = (j - k - 1) as i32;
                let step = if gap == 0 { 10 + 12 } else { 10 + bonus(j) - gap.min(8) - 2 };
                if prev + step > score[i][j] {
                    score[i][j] = prev + step;
                    from[i][j] = k;
                }
            }
        }
    }
    let last = n.len() - 1;
    let (mut j, best) = (0..h.len()).map(|j| (j, score[last][j])).max_by_key(|(_, s)| *s)?;
    if best <= NONE {
        return None;
    }
    let mut positions = vec![0usize; n.len()];
    for i in (0..n.len()).rev() {
        positions[i] = j;
        if i > 0 {
            j = from[i][j];
            if j == usize::MAX {
                return None;
            }
        }
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for p in positions {
        let start = h[p].0;
        let end = start + h[p].1.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.1 == start => last.1 = end,
            _ => ranges.push((start, end)),
        }
    }
    // Shorter targets win ties: "Compress" over "Compress with the Strong preset".
    Some(Match { score: best * 4 - (h.len() as i32).min(60), ranges })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_beats_scattered() {
        let a = fuzzy("comp", "Compress").unwrap();
        let b = fuzzy("comp", "Archival compliance").unwrap();
        let c = fuzzy("comp", "Crop & Resize map").unwrap();
        assert!(a.score > b.score, "{a:?} {b:?}");
        assert!(b.score > c.score, "{b:?} {c:?}");
        assert_eq!(a.ranges, vec![(0, 4)]);
        assert_eq!(b.ranges, vec![(9, 13)]);
    }

    #[test]
    fn subsequence_and_misses() {
        assert!(fuzzy("mrg", "Merge").is_some());
        assert!(fuzzy("xyz", "Merge").is_none());
        assert!(fuzzy("", "Merge").is_some());
        assert!(fuzzy("mergee", "Merge").is_none());
        assert_eq!(fuzzy("pdfa", "PDF/A").unwrap().ranges, vec![(0, 3), (4, 5)]);
    }

    #[test]
    fn word_starts_are_preferred() {
        let m = fuzzy("sm", "Strip Metadata").unwrap();
        assert_eq!(m.ranges, vec![(0, 1), (6, 7)]);
    }

    #[test]
    fn shorter_wins_ties() {
        let a = fuzzy("compress", "Compress").unwrap();
        let b = fuzzy("compress", "Compress with the Strong preset").unwrap();
        assert!(a.score > b.score);
    }
}
