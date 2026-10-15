//! Keyword web searches ("g rust lifetimes").

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSearch {
    pub keyword: String,
    pub title: String,
    /// URL template; `{query}` is replaced by the percent-encoded query.
    pub url: String,
    /// Offered as a fallback when nothing else matches.
    #[serde(default)]
    pub fallback: bool,
}

impl WebSearch {
    pub fn url_for(&self, query: &str) -> String {
        self.url.replace("{query}", &percent_encode(query))
    }
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub fn defaults() -> Vec<WebSearch> {
    let mk = |k: &str, t: &str, u: &str, f: bool| WebSearch {
        keyword: k.into(),
        title: t.into(),
        url: u.into(),
        fallback: f,
    };
    vec![
        mk("google", "Google", "https://www.google.com/search?q={query}", true),
        mk("g", "Google", "https://www.google.com/search?q={query}", false),
        mk("ddg", "DuckDuckGo", "https://duckduckgo.com/?q={query}", false),
        mk("bing", "Bing", "https://www.bing.com/search?q={query}", false),
        mk("amazon", "Amazon", "https://www.amazon.com/s?k={query}", true),
        mk(
            "wiki",
            "Wikipedia",
            "https://en.wikipedia.org/w/index.php?search={query}",
            true,
        ),
        mk(
            "youtube",
            "YouTube",
            "https://www.youtube.com/results?search_query={query}",
            false,
        ),
        mk("imdb", "IMDb", "https://www.imdb.com/find?q={query}", false),
        mk("maps", "Apple Maps", "https://maps.apple.com/?q={query}", false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_query() {
        let w = &defaults()[0];
        assert_eq!(
            w.url_for("rust & c++"),
            "https://www.google.com/search?q=rust%20%26%20c%2B%2B"
        );
    }
}
