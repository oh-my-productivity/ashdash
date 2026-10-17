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

/// The one rule for web-search URL templates (also used by the preferences window):
/// `https://host/path?q={query}`: http(s) only, a literal host, `{query}` after it, no other
/// placeholders, no credentials, no whitespace. The query is percent-encoded but the template
/// is not, so this is what keeps `file:`, `ssh:` or `x-apple...:` URLs out of `open`.
pub fn validate_url_template(raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Err("enter a URL".into());
    }
    if t.len() > 2000 {
        return Err("the URL is too long".into());
    }
    if t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("a URL cannot contain spaces".into());
    }
    let Some((scheme, rest)) = t.split_once("://") else {
        return Err("start with https:// (or http://)".into());
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err("only http:// and https:// addresses are allowed".into());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("the URL needs a host name".into());
    }
    if authority.contains('@') {
        return Err("do not put a user name or password in the URL".into());
    }
    if authority.contains('{') {
        return Err("{query} must come after the host name".into());
    }
    if !t.contains("{query}") {
        return Err("the URL must contain {query} where the search text goes".into());
    }
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        if !rest[i..].starts_with("{query}") {
            return Err("only {query} is supported as a placeholder".into());
        }
        rest = &rest[i + "{query}".len()..];
    }
    Ok(t.to_string())
}

/// The searches whose URL template passes [`validate_url_template`], plus a warning for each
/// one that does not (it is skipped).
pub fn checked(searches: &[WebSearch]) -> (Vec<WebSearch>, Vec<String>) {
    let (mut ok, mut warnings) = (Vec::new(), Vec::new());
    for ws in searches {
        match validate_url_template(&ws.url) {
            Ok(_) => ok.push(ws.clone()),
            Err(e) => warnings.push(format!(
                "web search '{}' ignored: its URL {:?} is not allowed ({e})",
                ws.keyword, ws.url
            )),
        }
    }
    (ok, warnings)
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
    fn url_templates_allow_only_plain_web_urls_with_query() {
        let ok = |u: &str| validate_url_template(u).unwrap();
        assert_eq!(ok(" https://e.com/?q={query} "), "https://e.com/?q={query}");
        assert_eq!(ok("HTTP://e.com/{query}/x"), "HTTP://e.com/{query}/x");
        for bad in [
            "",
            "file:///Applications/Foo.app/{query}",
            "ssh://host/{query}",
            "ftp://e.com/{query}",
            "javascript:alert({query})",
            "https://user:pw@e.com/?q={query}",
            "https://{query}.e.com/",
            "https:///path/{query}",
            "https://e.com/ ?q={query}",
            "https://e.com/?q={other}{query}",
            "https://e.com/?q=1",
        ] {
            assert!(validate_url_template(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn checked_skips_bad_templates_and_reports_each() {
        let mk = |k: &str, u: &str| WebSearch {
            keyword: k.into(),
            title: k.into(),
            url: u.into(),
            fallback: true,
        };
        let (ok, warnings) = checked(&[
            mk("good", "https://e.com/?q={query}"),
            mk("evil", "file:///etc/{query}"),
            mk("creds", "https://u:p@e.com/{query}"),
        ]);
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].keyword, "good");
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("evil") && warnings[1].contains("creds"));
        assert!(checked(&defaults()).1.is_empty(), "the defaults are valid");
    }

    #[test]
    fn encodes_query() {
        let w = &defaults()[0];
        assert_eq!(
            w.url_for("rust & c++"),
            "https://www.google.com/search?q=rust%20%26%20c%2B%2B"
        );
    }
}
