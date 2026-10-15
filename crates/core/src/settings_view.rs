//! Read and write any setting by dotted path (`clipboard.max_entries`,
//! `bookmarks.enabled_sources`, `file_search.roots`).
//!
//! The configuration is viewed as JSON. A change is applied to that view and
//! then pushed back through [`Config`]'s deserialiser, so a value of the
//! wrong type is rejected with the same rules the file itself is held to, and
//! a key that does not exist is an error instead of being silently dropped
//! (serde ignores unknown keys when loading).

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::config::Config;

fn view(config: &Config) -> Value {
    serde_json::to_value(config).unwrap_or(Value::Null)
}

fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(root);
    }
    let mut cur = root;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(o) => o.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn lookup_mut<'a>(root: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    let mut cur = root;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(o) => o.get_mut(part)?,
            Value::Array(a) => a.get_mut(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Every settable key (leaf path), e.g. for `ashdash config get` listings and
/// completion. Arrays and `null` options are leaves; objects are descended into.
pub fn keys(config: &Config) -> Vec<String> {
    fn walk(prefix: &str, v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, v) in o {
                    let p = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&p, v, out);
                }
            }
            _ => out.push(prefix.to_string()),
        }
    }
    let mut out = Vec::new();
    walk("", &view(config), &mut out);
    out.sort();
    out
}

/// The value at `path` (empty path = the whole configuration).
pub fn get(config: &Config, path: &str) -> Result<Value> {
    let v = view(config);
    lookup(&v, path)
        .cloned()
        .with_context(|| format!("no setting '{path}' (see `ashdash config get`)"))
}

/// Interpret user text as a value for a setting that currently holds `current`.
/// Text that parses as JSON is taken as JSON (`42`, `true`, `null`, `["a","b"]`,
/// `"quoted"`). Otherwise it is a string, or for a list setting a
/// comma-separated list of strings.
fn parse_value(raw: &str, current: &Value) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(raw.trim()) {
        // A bare number for a string setting (`theme 42`) stays a string.
        if !(v.is_number() || v.is_boolean()) || !current.is_string() {
            return v;
        }
    }
    if current.is_array() {
        let parts: Vec<Value> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Value::String(s.to_string()))
            .collect();
        return Value::Array(parts);
    }
    Value::String(raw.to_string())
}

/// Set `path` to `raw` and return the validated configuration. `config` is
/// only modified when the change is valid.
pub fn set(config: &mut Config, path: &str, raw: &str) -> Result<()> {
    if path.is_empty() {
        bail!("a setting name is required");
    }
    let mut doc = view(config);
    let current = lookup(&doc, path)
        .cloned()
        .with_context(|| format!("no setting '{path}' (see `ashdash config get`)"))?;
    if current.is_object() {
        bail!("'{path}' is a group of settings; set one of its keys");
    }
    let new = parse_value(raw, &current);
    *lookup_mut(&mut doc, path).context("setting vanished")? = new.clone();
    let updated: Config = serde_json::from_value(doc).with_context(|| format!("invalid value for '{path}': {raw}"))?;
    // Round trip: what was stored must be what comes back (nothing clamped or dropped).
    if lookup(&view(&updated), path) != Some(&new) {
        bail!("invalid value for '{path}': {raw}");
    }
    *config = updated;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn get_reads_nested_values_and_whole_documents() {
        let c = Config::default();
        assert_eq!(get(&c, "clipboard.max_entries").unwrap(), json!(200));
        assert_eq!(get(&c, "max_results").unwrap(), json!(30));
        assert_eq!(get(&c, "theme").unwrap(), Value::Null);
        assert_eq!(get(&c, "bookmarks.include_in_general_search").unwrap(), json!(true));
        assert!(get(&c, "").unwrap().is_object());
        assert_eq!(get(&c, "web_searches.0.keyword").unwrap(), json!("google"));
        assert!(get(&c, "clipboard.nope").is_err());
        assert!(get(&c, "web_searches.9999").is_err());
    }

    #[test]
    fn set_validates_types_and_unknown_keys() {
        let mut c = Config::default();
        set(&mut c, "clipboard.max_entries", "50").unwrap();
        assert_eq!(c.clipboard.max_entries, 50);
        set(&mut c, "clipboard.max_age_days", "30").unwrap();
        assert_eq!(c.clipboard.max_age_days, Some(30));
        set(&mut c, "clipboard.max_age_days", "null").unwrap();
        assert_eq!(c.clipboard.max_age_days, None);
        set(&mut c, "hotkey", "cmd+space").unwrap();
        assert_eq!(c.hotkey, "cmd+space");
        set(&mut c, "file_search.include_hidden", "true").unwrap();
        assert!(c.file_search.include_hidden);

        let before = serde_json::to_value(&c).unwrap();
        for (k, v) in [
            ("clipboard.max_entries", "lots"),
            ("clipboard.max_entries", "-3"),
            ("max_results", "1.5"),
            ("file_search.include_hidden", "maybe"),
            ("no_such_key", "1"),
            ("clipboard.max_entires", "1"),
            ("clipboard", "{}"),
            ("", "1"),
        ] {
            assert!(set(&mut c, k, v).is_err(), "{k}={v}");
        }
        assert_eq!(serde_json::to_value(&c).unwrap(), before, "failed sets change nothing");
    }

    #[test]
    fn strings_lists_and_options_accept_plain_text() {
        let mut c = Config::default();
        set(&mut c, "hotkey", "alt+space").unwrap();
        set(&mut c, "theme", "/tmp/dark.alfredappearance").unwrap();
        assert_eq!(
            c.theme.as_deref(),
            Some(std::path::Path::new("/tmp/dark.alfredappearance"))
        );
        set(&mut c, "providers_disabled", "calc, system").unwrap();
        assert_eq!(c.providers_disabled, vec!["calc", "system"]);
        set(&mut c, "providers_disabled", r#"["shell"]"#).unwrap();
        assert_eq!(c.providers_disabled, vec!["shell"]);
        set(&mut c, "providers_disabled", "[]").unwrap();
        assert!(c.providers_disabled.is_empty());
        // a number-looking value for a string setting stays text
        set(&mut c, "hotkey", "42").unwrap();
        assert_eq!(c.hotkey, "42");
        set(&mut c, "clipboard.ignored_apps", "com.a.b").unwrap();
        assert_eq!(c.clipboard.ignored_apps, vec!["com.a.b"]);
    }

    #[test]
    fn keys_lists_leaves() {
        let k = keys(&Config::default());
        for want in [
            "hotkey",
            "clipboard.max_entries",
            "clipboard.ignored_apps",
            "file_search.roots",
            "bookmarks.enabled_sources",
            "providers_disabled",
            "theme",
        ] {
            assert!(k.iter().any(|x| x == want), "missing {want} in {k:?}");
        }
        assert!(!k.iter().any(|x| x == "clipboard"));
    }
}
