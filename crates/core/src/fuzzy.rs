//! Matching and scoring of a typed query against a candidate title.
//!
//! The reference launcher matches on prefixes of words and on word initials ("vsc" finds
//! "Visual Studio Code"). This module reproduces that behaviour from
//! observation; the exact numeric scores are our own.

fn normalize(s: &str) -> Vec<char> {
    s.chars().flat_map(|c| c.to_lowercase()).collect()
}

/// Split on non-alphanumerics and camelCase boundaries, lower-casing each word.
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

/// Every query word must be a prefix of a later title word, in order.
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

/// Query characters are consumed as leading runs of successive words.
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

/// Returns `None` if `text` does not match `query`, otherwise a score where
/// larger is better. An empty query matches everything with score 0.
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
    fn empty_query_matches() {
        assert_eq!(score("", "anything"), Some(0.0));
    }

    #[test]
    fn case_insensitive() {
        assert!(score("SAF", "safari").is_some());
    }
}
