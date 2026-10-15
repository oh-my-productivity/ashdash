//! Matching and scoring of a typed query against a candidate title.
//!
//! The reference launcher matches on prefixes of words and on word initials ("vsc" finds
//! "Visual Studio Code"). This module reproduces that behaviour from
//! observation; the exact numeric scores are our own.
//!
//! Hot paths (thousands of candidates per keystroke) should build a
//! [`Query`] once per keystroke and a [`Candidate`] once per candidate (at
//! index time), then call [`Query::score`]: that path performs no allocation.
//! [`score`] is the convenience wrapper for one-off comparisons.

/// Longest query (characters, after trimming) that can match anything. A longer one is a paste,
/// not a search; it matches nothing instead of costing time (SEC-09).
pub const MAX_QUERY_CHARS: usize = 256;
/// Words of a candidate title taken into account; later words are ignored.
const MAX_CANDIDATE_WORDS: usize = 1024;
/// Initials matching tracks the consumed query prefix in a `u128`, so it needs `len < 128`.
const MAX_INITIALS_CHARS: usize = 127;

/// A candidate title with its normalized and word-split forms precomputed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Candidate {
    /// The whole title, lower-cased.
    lower: Box<[char]>,
    /// All words (lower-cased, see [`split_words`]) back to back.
    word_chars: Box<[char]>,
    /// `(start, end)` of every word inside `word_chars`.
    words: Box<[(u32, u32)]>,
}

impl Candidate {
    pub fn new(text: &str) -> Candidate {
        let lower: Box<[char]> = normalize(text).into_boxed_slice();
        let mut word_chars: Vec<char> = Vec::with_capacity(lower.len());
        let mut words: Vec<(u32, u32)> = Vec::new();
        let mut start = 0usize;
        let mut prev: Option<char> = None;
        for ch in text.chars() {
            if words.len() >= MAX_CANDIDATE_WORDS {
                start = word_chars.len();
                break;
            }
            if !ch.is_alphanumeric() {
                if word_chars.len() > start {
                    words.push((start as u32, word_chars.len() as u32));
                    start = word_chars.len();
                }
                prev = None;
                continue;
            }
            if ch.is_uppercase() && prev.is_some_and(|p| p.is_lowercase()) && word_chars.len() > start {
                words.push((start as u32, word_chars.len() as u32));
                start = word_chars.len();
            }
            word_chars.extend(ch.to_lowercase());
            prev = Some(ch);
        }
        if word_chars.len() > start && words.len() < MAX_CANDIDATE_WORDS {
            words.push((start as u32, word_chars.len() as u32));
        }
        Candidate {
            lower,
            word_chars: word_chars.into_boxed_slice(),
            words: words.into_boxed_slice(),
        }
    }

    #[inline]
    fn word(&self, i: usize) -> &[char] {
        let (s, e) = self.words[i];
        &self.word_chars[s as usize..e as usize]
    }
}

/// A typed query with its normalized forms precomputed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Trimmed, lower-cased query.
    q: Vec<char>,
    /// `(start, end)` of the whitespace-separated words inside `q`.
    qwords: Vec<(usize, usize)>,
    /// `q` without whitespace (for initials matching).
    compact: Vec<char>,
    /// Longer than [`MAX_QUERY_CHARS`]: matches nothing.
    too_long: bool,
}

impl Query {
    pub fn new(query: &str) -> Query {
        let trimmed = query.trim();
        let too_long = trimmed.chars().nth(MAX_QUERY_CHARS).is_some();
        let q = if too_long { Vec::new() } else { normalize(trimmed) };
        let mut qwords = Vec::new();
        let mut start = None;
        for (i, c) in q.iter().enumerate() {
            match (c.is_whitespace(), start) {
                (false, None) => start = Some(i),
                (true, Some(s)) => {
                    qwords.push((s, i));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            qwords.push((s, q.len()));
        }
        let compact = q.iter().copied().filter(|c| !c.is_whitespace()).collect();
        Query {
            q,
            qwords,
            compact,
            too_long,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.q.is_empty() && !self.too_long
    }

    /// Returns `None` if `cand` does not match, otherwise a score where
    /// larger is better. An empty query matches everything with score 0.
    /// Does not allocate.
    pub fn score(&self, cand: &Candidate) -> Option<f64> {
        if self.too_long {
            return None;
        }
        let q = &self.q[..];
        if q.is_empty() {
            return Some(0.0);
        }
        let t = &cand.lower[..];
        let len_penalty = t.len() as f64 * 0.01;

        if t == q {
            return Some(1000.0);
        }
        if t.starts_with(q) {
            return Some(900.0 - (t.len() - q.len()) as f64 * 0.5);
        }
        if self.words_prefix_match(cand) {
            return Some(800.0 - len_penalty);
        }
        if (2..=MAX_INITIALS_CHARS).contains(&self.compact.len()) && initials_match(&self.compact, cand) {
            return Some(600.0 - len_penalty);
        }
        if let Some(pos) = find_sub(t, q) {
            return Some(400.0 - pos as f64 * 0.5 - len_penalty);
        }
        None
    }

    /// Every query word must be a prefix of a later title word, in order.
    fn words_prefix_match(&self, cand: &Candidate) -> bool {
        let mut ti = 0;
        for &(s, e) in &self.qwords {
            let qw = &self.q[s..e];
            loop {
                if ti >= cand.words.len() {
                    return false;
                }
                let tw = cand.word(ti);
                ti += 1;
                if tw.starts_with(qw) {
                    break;
                }
            }
        }
        true
    }
}

fn normalize(s: &str) -> Vec<char> {
    s.chars().flat_map(|c| c.to_lowercase()).collect()
}

/// Query characters are consumed as leading runs of successive words (a word may also be
/// skipped). Tracks, per word, the set of query prefix lengths already consumed in a `u128`:
/// no recursion, no allocation, O(words x query length) (the naive search was exponential).
/// Requires `2 <= q.len() <= MAX_INITIALS_CHARS`.
fn initials_match(q: &[char], cand: &Candidate) -> bool {
    debug_assert!(q.len() <= MAX_INITIALS_CHARS);
    let goal = 1u128 << q.len();
    let mut reach: u128 = 1; // bit i: the first i query characters are consumed
    for wi in 0..cand.words.len() {
        let w = cand.word(wi);
        let mut next = reach;
        let mut bits = reach;
        while bits != 0 {
            let i = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let rest = &q[i..];
            let max = w.len().min(rest.len());
            let common = rest.iter().zip(w).take(max).take_while(|(a, b)| a == b).count();
            // consuming 1..=common characters of the remaining query, all matching the word's start
            for k in 1..=common {
                next |= 1u128 << (i + k);
            }
        }
        if next & goal != 0 {
            return true;
        }
        reach = next;
    }
    false
}

fn find_sub(t: &[char], q: &[char]) -> Option<usize> {
    if q.is_empty() || q.len() > t.len() {
        return None;
    }
    let first = q[0];
    (0..=t.len() - q.len()).find(|&i| t[i] == first && t[i..i + q.len()] == *q)
}

/// Returns `None` if `text` does not match `query`, otherwise a score where
/// larger is better. An empty query matches everything with score 0.
///
/// Convenience wrapper; allocates. Prefer [`Query`] + [`Candidate`] in loops.
pub fn score(query: &str, text: &str) -> Option<f64> {
    Query::new(query).score(&Candidate::new(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_beats_prefix_beats_initials() {
        let exact = score("safari", "Safari").unwrap();
        let prefix = score("saf", "Safari").unwrap();
        let init = score("vsc", "Visual Studio Code").unwrap();
        let sub = score("udio", "Visual Studio Code").unwrap();
        assert!(exact > prefix && prefix > init && init > sub);
    }

    #[test]
    fn word_prefix_matches_later_words() {
        assert!(score("stu", "Visual Studio Code").unwrap() >= 800.0 - 1.0);
        assert!(score("vis cod", "Visual Studio Code").is_some());
        assert!(score("code vis", "Visual Studio Code").is_none());
    }

    #[test]
    fn initials_work_with_partial_words() {
        assert!(score("vsc", "Visual Studio Code").is_some());
        assert!(score("viscode", "Visual Studio Code").is_some());
        assert!(score("xyz", "Visual Studio Code").is_none());
    }

    #[test]
    fn camel_case_words_and_no_loose_subsequences() {
        assert!(score("sm", "PreviewShellMac").is_some());
        assert!(score("vsc", "PreviewShellMac").is_none());
        assert!(score("sleep", "Install Command Line Developer Tools").is_none());
    }

    #[test]
    fn overlong_queries_match_nothing_and_pathological_titles_stay_fast() {
        let long = "a".repeat(MAX_QUERY_CHARS + 1);
        assert_eq!(score(&long, &long), None);
        assert!(!Query::new(&long).is_empty());
        assert!(score(&"a".repeat(MAX_QUERY_CHARS), &"a".repeat(MAX_QUERY_CHARS)).is_some());
        // the old recursive search was exponential here (every word can be skipped or used)
        let title = "a ".repeat(5000);
        let t = std::time::Instant::now();
        assert_eq!(score(&format!("{}b", "a".repeat(100)), &title), None);
        assert!(score("aaaa", &title).is_some());
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn empty_query_matches() {
        assert_eq!(score("", "anything"), Some(0.0));
    }

    #[test]
    fn case_insensitive() {
        assert!(score("SAF", "safari").is_some());
    }

    /// The pre-`Candidate` implementation, kept verbatim as the oracle for the equivalence test.
    mod reference {
        fn normalize(s: &str) -> Vec<char> {
            s.chars().flat_map(|c| c.to_lowercase()).collect()
        }

        fn split_words(text: &str) -> Vec<Vec<char>> {
            let mut words: Vec<Vec<char>> = Vec::new();
            let mut cur: Vec<char> = Vec::new();
            let mut prev: Option<char> = None;
            for ch in text.chars() {
                if !ch.is_alphanumeric() {
                    if !cur.is_empty() {
                        words.push(std::mem::take(&mut cur));
                    }
                    prev = None;
                    continue;
                }
                if ch.is_uppercase() && prev.is_some_and(|p| p.is_lowercase()) && !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
                cur.extend(ch.to_lowercase());
                prev = Some(ch);
            }
            if !cur.is_empty() {
                words.push(cur);
            }
            words
        }

        fn words_prefix_match(qwords: &[&[char]], twords: &[&[char]]) -> bool {
            let mut ti = 0;
            for qw in qwords {
                loop {
                    if ti >= twords.len() {
                        return false;
                    }
                    let tw = twords[ti];
                    ti += 1;
                    if tw.starts_with(qw) {
                        break;
                    }
                }
            }
            true
        }

        fn initials_match(q: &[char], words: &[&[char]]) -> bool {
            if q.is_empty() {
                return true;
            }
            if words.is_empty() {
                return false;
            }
            let w = words[0];
            let max = w.len().min(q.len());
            for k in (1..=max).rev() {
                if q[..k] == w[..k] && initials_match(&q[k..], &words[1..]) {
                    return true;
                }
            }
            initials_match(q, &words[1..])
        }

        fn find_sub(t: &[char], q: &[char]) -> Option<usize> {
            if q.is_empty() || q.len() > t.len() {
                return None;
            }
            (0..=t.len() - q.len()).find(|&i| &t[i..i + q.len()] == q)
        }

        pub fn score(query: &str, text: &str) -> Option<f64> {
            let q = normalize(query.trim());
            if q.is_empty() {
                return Some(0.0);
            }
            let t = normalize(text);
            let len_penalty = t.len() as f64 * 0.01;
            if t == q {
                return Some(1000.0);
            }
            if t.starts_with(&q) {
                return Some(900.0 - (t.len() - q.len()) as f64 * 0.5);
            }
            let twords_owned = split_words(text);
            let twords: Vec<&[char]> = twords_owned.iter().map(|w| w.as_slice()).collect();
            let qwords_owned: Vec<Vec<char>> = q
                .split(|c| c.is_whitespace())
                .filter(|w| !w.is_empty())
                .map(|w| w.to_vec())
                .collect();
            let qwords: Vec<&[char]> = qwords_owned.iter().map(|w| w.as_slice()).collect();
            if words_prefix_match(&qwords, &twords) {
                return Some(800.0 - len_penalty);
            }
            let qc: Vec<char> = q.iter().copied().filter(|c| !c.is_whitespace()).collect();
            if qc.len() >= 2 && initials_match(&qc, &twords) {
                return Some(600.0 - len_penalty);
            }
            if let Some(pos) = find_sub(&t, &q) {
                return Some(400.0 - pos as f64 * 0.5 - len_penalty);
            }
            None
        }
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn string(&mut self, alphabet: &[char], max_len: usize) -> String {
            let n = (self.next() % (max_len as u64 + 1)) as usize;
            (0..n)
                .map(|_| alphabet[(self.next() % alphabet.len() as u64) as usize])
                .collect()
        }
    }

    const ALPHABET: &[char] = &[
        'a', 'b', 'c', 's', 'v', 'x', 'A', 'B', 'S', 'V', 'C', ' ', ' ', ' ', '-', '_', '.', '1', '2', '\u{e9}',
        '\u{c9}', '\u{130}', '\u{df}', '\u{65e5}', '\u{672c}', '\t',
    ];

    /// Texts and queries drawn from the same small alphabet so that every branch (exact, prefix,
    /// word prefix, initials, substring, no match) is hit often.
    #[test]
    fn candidate_scoring_equals_string_scoring_on_random_input() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut branches = std::collections::BTreeSet::new();
        for _ in 0..40_000 {
            let text = rng.string(ALPHABET, 14);
            let query = if rng.next().is_multiple_of(3) {
                // a query derived from the text makes matches likely
                let chars: Vec<char> = text.chars().collect();
                let a = (rng.next() % (chars.len() as u64 + 1)) as usize;
                let b = a + (rng.next() % (chars.len() as u64 + 1 - a as u64)) as usize;
                chars[a..b].iter().collect::<String>()
            } else {
                rng.string(ALPHABET, 5)
            };
            let expected = reference::score(&query, &text);
            assert_eq!(score(&query, &text), expected, "wrapper: q={query:?} t={text:?}");
            let q = Query::new(&query);
            let c = Candidate::new(&text);
            assert_eq!(q.score(&c), expected, "candidate: q={query:?} t={text:?}");
            // a candidate/query can be reused: scoring twice gives the same answer
            assert_eq!(q.score(&c), expected);
            branches.insert(match expected {
                None => "none",
                Some(s) if s >= 999.0 => "exact",
                Some(s) if s >= 850.0 => "prefix",
                Some(s) if s >= 700.0 => "words",
                Some(s) if s >= 500.0 => "initials",
                Some(_) => "substring-or-empty",
            });
        }
        // every scoring tier was exercised
        for tier in ["none", "exact", "prefix", "words", "initials", "substring-or-empty"] {
            assert!(branches.contains(tier), "tier {tier:?} never hit: {branches:?}");
        }
    }

    #[test]
    fn candidate_handles_unicode_and_degenerate_text() {
        for (q, t) in [
            ("a  b", "a b"),
            ("\u{130}", "\u{130}stanbul"),
            ("", ""),
            ("x", ""),
            ("  ", "abc"),
            ("\u{672c}", "\u{65e5}\u{672c}"),
        ] {
            assert_eq!(score(q, t), reference::score(q, t), "{q:?} {t:?}");
        }
    }
}
