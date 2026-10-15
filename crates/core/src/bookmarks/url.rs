//! URL hygiene for bookmarks: which schemes we are willing to open, display
//! host, and the key used to de-duplicate.

const ALLOWED: &[&str] = &["http", "https", "file", "ftp"];

/// Trim `raw` and return it only if it is safe to hand to `open`: an
/// http/https/file/ftp URL without control characters. `javascript:`
/// bookmarklets, `data:`, `place:`, `chrome://` and friends yield `None`.
pub fn clean_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    if url.is_empty() || url.chars().any(|c| c.is_control()) {
        return None;
    }
    let (scheme, rest) = url.split_once(':')?;
    let scheme = scheme.to_ascii_lowercase();
    if !ALLOWED.contains(&scheme.as_str()) {
        return None;
    }
    match scheme.as_str() {
        "file" => (rest.starts_with("//") || rest.starts_with('/')).then(|| url.to_string()),
        _ => {
            let auth = authority(rest.strip_prefix("//")?);
            (!host_of(auth).is_empty()).then(|| url.to_string())
        }
    }
}

/// `host[:port]/...` -> the authority part (up to `/`, `?` or `#`).
fn authority(after_slashes: &str) -> &str {
    let end = after_slashes.find(['/', '?', '#']).unwrap_or(after_slashes.len());
    &after_slashes[..end]
}

/// Host of an authority: no userinfo, no port.
fn host_of(auth: &str) -> &str {
    let auth = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    if auth.starts_with('[') {
        return auth.find(']').map_or(auth, |i| &auth[..=i]);
    }
    auth.split(':').next().unwrap_or("")
}

/// Host for display (`www.` stripped). Empty for `file:` URLs.
pub fn display_host(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once(':') else {
        return String::new();
    };
    if scheme.eq_ignore_ascii_case("file") {
        return String::new();
    }
    let Some(rest) = rest.strip_prefix("//") else {
        return String::new();
    };
    let host = host_of(authority(rest));
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// Two URLs with the same key are the same bookmark: scheme and authority are
/// case-insensitive and a bare trailing `/` on the root path is ignored.
pub fn dedupe_key(url: &str) -> String {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    if scheme.eq_ignore_ascii_case("file") {
        return url.to_string();
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (auth, tail) = rest.split_at(end);
    let tail = if tail == "/" { "" } else { tail };
    format!(
        "{}://{}{}",
        scheme.to_ascii_lowercase(),
        auth.to_ascii_lowercase(),
        tail
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_only_safe_schemes() {
        for ok in [
            "http://a.com",
            "HTTPS://a.com/x?y=1#z",
            "ftp://files.example.org/pub",
            "file:///Users/me/doc.html",
            "  https://a.com  ",
        ] {
            assert!(clean_url(ok).is_some(), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "JavaScript:void(0)",
            "data:text/html,hi",
            "place:folder=BOOKMARKS_MENU",
            "chrome://settings",
            "about:blank",
            "mailto:a@b.c",
            "http://",
            "https:///nohost",
            "http:example.com",
            "https://a.com/\nx",
            "",
            "no scheme at all",
        ] {
            assert!(clean_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(clean_url("  https://a.com/ ").as_deref(), Some("https://a.com/"));
    }

    #[test]
    fn hosts() {
        assert_eq!(display_host("https://www.rust-lang.org/learn"), "rust-lang.org");
        assert_eq!(display_host("http://user:pw@host.io:8080/p"), "host.io");
        assert_eq!(display_host("http://[::1]:3000/"), "[::1]");
        assert_eq!(display_host("file:///tmp/x.html"), "");
    }

    #[test]
    fn dedupe_keys() {
        assert_eq!(dedupe_key("https://A.com/"), dedupe_key("https://a.com"));
        assert_eq!(dedupe_key("HTTPS://a.com"), dedupe_key("https://a.com/"));
        assert_ne!(dedupe_key("https://a.com/x"), dedupe_key("https://a.com/x/"));
        assert_ne!(dedupe_key("https://a.com/X"), dedupe_key("https://a.com/x"));
        assert_ne!(dedupe_key("http://a.com"), dedupe_key("https://a.com"));
    }
}
