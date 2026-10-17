//! Property / fuzz tests on the public API. Inputs are random; the properties are
//! "never panics, never hangs, output stays sane". Case counts are kept small enough for
//! the whole file to run in a few seconds (override with `PROPTEST_CASES`).

mod common;

use std::time::{Duration, Instant};

use ashdash_core::calc;
use ashdash_core::config::{Config, Paths};
use ashdash_core::fuzzy;
use ashdash_core::item::Action;
use ashdash_core::websearch::percent_encode;
use ashdash_core::workflow::Workflow;
use common::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn cfg(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        // Do not write *.proptest-regressions files into the source tree.
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

// ---------------------------------------------------------------- percent_encode

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

proptest! {
    #![proptest_config(cfg(512))]

    #[test]
    fn percent_encode_round_trips_and_is_url_safe(s in "\\PC{0,64}") {
        let enc = percent_encode(&s);
        prop_assert!(enc.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~%".contains(&b)),
            "unsafe byte in {enc:?}");
        // every % is followed by two upper-case hex digits
        let bytes = enc.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'%' {
                prop_assert!(i + 2 < bytes.len(), "truncated escape in {enc:?}");
                prop_assert!(bytes[i + 1..i + 3].iter().all(|h| h.is_ascii_digit() || (b'A'..=b'F').contains(h)));
            }
        }
        prop_assert_eq!(percent_decode(&enc), s.as_bytes().to_vec());
    }

    #[test]
    fn percent_encode_of_any_text_has_no_reserved_url_characters(s in any::<String>()) {
        let enc = percent_encode(&s);
        for bad in ['/', '?', '#', '&', '=', ' ', '"', '<', '>', ':', '+', '\n'] {
            prop_assert!(!enc.contains(bad), "{bad:?} survived in {enc:?}");
        }
    }
}

// ---------------------------------------------------------------- calc

proptest! {
    #![proptest_config(cfg(2000))]

    #[test]
    fn calc_never_panics_on_arbitrary_text(s in any::<String>()) {
        if let Some(v) = calc::eval(&s) { prop_assert!(v.is_finite()); }
        let _ = calc::looks_like_expression(&s);
    }

    #[test]
    fn calc_never_panics_on_expression_like_text(s in "[0-9a-z+*/^%().,×÷ \\-]{0,48}") {
        if let Some(v) = calc::eval(&s) {
            prop_assert!(v.is_finite());
            let _ = calc::format(v);
        }
    }

    #[test]
    fn calc_format_never_panics(v in any::<f64>()) {
        let _ = calc::format(v);
    }

    #[test]
    fn calc_integer_addition_agrees_with_rust(a in -1_000_000i64..1_000_000, b in -1_000_000i64..1_000_000) {
        prop_assert_eq!(calc::eval(&format!("{a}+{b}")), Some((a + b) as f64));
        prop_assert_eq!(calc::eval(&format!("({a})*({b})")), Some((a * b) as f64));
    }
}

/// Deeply nested input must be rejected or evaluated, never overflow the stack (an overflow
/// aborts the whole launcher and cannot be caught). Each shape runs in a child process on a
/// 2 MiB stack (the default for spawned threads) so the abort becomes an ordinary failure.
fn nesting_shape(kind: &str, n: usize) -> String {
    match kind {
        "parens" => format!("{}1{}", "(".repeat(n), ")".repeat(n)),
        "negation" => format!("{}1", "-".repeat(n)),
        "power" => format!("1{}", "^2".repeat(n)),
        "calls" => format!("{}1", "sqrt(".repeat(n)),
        _ => unreachable!(),
    }
}

#[test]
fn calc_nesting_child() {
    // Only does work when launched by `calc_deep_nesting_*`; a no-op in a normal run.
    let Ok(kind) = std::env::var("QA_CALC_SHAPE") else {
        return;
    };
    let n: usize = std::env::var("QA_CALC_N").unwrap().parse().unwrap();
    let expr = nesting_shape(&kind, n);
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let _ = calc::eval(&expr);
        })
        .unwrap()
        .join()
        .unwrap();
}

fn survives_nesting(kind: &str, n: usize) -> bool {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "calc_nesting_child", "--nocapture", "--test-threads=1"])
        .env("QA_CALC_SHAPE", kind)
        .env("QA_CALC_N", n.to_string())
        .output()
        .unwrap();
    out.status.success()
}

#[test]
fn calc_moderate_nesting_is_fine() {
    for kind in ["parens", "negation", "power", "calls"] {
        assert!(survives_nesting(kind, 100), "{kind} x100");
    }
}

#[test]
fn calc_deep_nesting_does_not_overflow_the_stack() {
    for kind in ["parens", "negation", "power", "calls"] {
        assert!(survives_nesting(kind, 100_000), "{kind} x100000 crashed the process");
    }
}

#[test]
fn calc_pathological_numbers_are_bounded_in_time() {
    let start = Instant::now();
    for e in [
        "9^9^9^9",
        "10^1000",
        "1/0",
        "0/0",
        "99999999999999999999999999999999999999999",
        "1e999999999",
        "2^2^2^2^2^2",
        "%%%%",
        "pow(9,pow(9,9))",
    ] {
        let _ = calc::eval(e);
    }
    assert!(start.elapsed() < Duration::from_secs(2));
}

// ---------------------------------------------------------------- fuzzy

proptest! {
    #![proptest_config(cfg(2000))]

    #[test]
    fn fuzzy_never_panics_on_random_unicode(q in any::<String>(), t in any::<String>()) {
        if let Some(s) = fuzzy::score(&q, &t) { prop_assert!(!s.is_nan()); }
    }

    #[test]
    fn fuzzy_empty_query_matches_everything(t in any::<String>()) {
        prop_assert_eq!(fuzzy::score("", &t), Some(0.0));
        prop_assert_eq!(fuzzy::score("   ", &t), Some(0.0));
    }

    #[test]
    fn fuzzy_text_matches_itself_best(t in "[A-Za-z0-9 éüß]{1,24}") {
        prop_assume!(!t.trim().is_empty());
        let own = fuzzy::score(&t, &t);
        prop_assert!(own.is_some(), "{t:?} does not match itself");
        // nothing may outrank an exact match
        let other = fuzzy::score(&t, &format!("{t} extra"));
        if let (Some(o), Some(x)) = (own, other) { prop_assert!(o >= x, "{o} < {x}"); }
    }

    #[test]
    fn fuzzy_prefix_of_text_always_matches(t in "[a-z]{3,16}", cut in 1usize..3) {
        let q = &t[..t.len().min(cut)];
        prop_assert!(fuzzy::score(q, &t).is_some());
    }

    #[test]
    fn fuzzy_is_case_insensitive(q in "[a-zA-Z]{1,6}", t in "[a-zA-Z ]{1,24}") {
        prop_assert_eq!(fuzzy::score(&q.to_lowercase(), &t), fuzzy::score(&q.to_uppercase(), &t));
    }
}

/// Runs `child` (a no-op test unless `QA_CASE` is set) in a fresh process on a 2 MiB thread stack,
/// so that a stack-overflow abort becomes an ordinary assertion failure.
fn child_survives(child: &str, case: &str) -> bool {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", child, "--nocapture", "--test-threads=1"])
        .env("QA_CASE", case)
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn fuzzy_long_child() {
    let Ok(case) = std::env::var("QA_CASE") else {
        return;
    };
    let (kind, n) = case.split_once(':').unwrap();
    let n: usize = n.parse().unwrap();
    let (q, t) = match kind {
        "words" => ("a".repeat(8), "ab ".repeat(n)),
        "qwords" => ("a ".repeat(n), "ab ".repeat(8)),
        "chars" => ("a".repeat(n), "ab".repeat(n)),
        "unicode" => ("é".repeat(n), "e\u{301}".repeat(n)),
        _ => unreachable!(),
    };
    let start = Instant::now();
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let _ = fuzzy::score(&q, &t);
            let _ = fuzzy::score(&t, &q);
        })
        .unwrap()
        .join()
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
}

#[test]
fn fuzzy_moderately_long_input_is_fine_and_fast() {
    for kind in ["words", "qwords", "chars", "unicode"] {
        assert!(child_survives("fuzzy_long_child", &format!("{kind}:300")), "{kind}:300");
    }
}

#[test]
fn fuzzy_pathologically_long_input_neither_crashes_nor_stalls() {
    for kind in ["words", "qwords", "chars", "unicode"] {
        assert!(
            child_survives("fuzzy_long_child", &format!("{kind}:30000")),
            "{kind}:30000"
        );
    }
}

// ---------------------------------------------------------------- config migration

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        (-1e12f64..1e12).prop_map(|f| json!(f)),
        "\\PC{0,12}".prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            prop::collection::hash_map("[a-z_]{1,14}", inner, 0..6)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

/// Same, but with the real config keys present so that deserialisation goes deep.
fn arb_config_json() -> impl Strategy<Value = Value> {
    let keys = [
        "version",
        "hotkey",
        "max_results",
        "app_dirs",
        "workflow_dirs",
        "web_searches",
        "theme",
        "clipboard_history_size",
    ];
    prop::collection::vec((prop::sample::select(keys.to_vec()), arb_json()), 0..8)
        .prop_map(|kv| Value::Object(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect()))
}

proptest! {
    #![proptest_config(cfg(300))]

    #[test]
    fn config_load_survives_arbitrary_json(doc in prop_oneof![arb_json(), arb_config_json()]) {
        let t = TempDir::new("cfg-fuzz");
        let paths = Paths::under(&t.join("env"));
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), serde_json::to_vec(&doc).unwrap()).unwrap();
        let c = Config::load(&paths);
        // whatever came in, the result is usable and re-savable (unless it is from the future)
        let future = doc.get("version").and_then(Value::as_u64).is_some_and(|v| v > ashdash_core::config::CURRENT_VERSION as u64);
        let saved = c.save(&paths);
        if future { prop_assert!(saved.is_err()); } else { prop_assert!(saved.is_ok(), "{saved:?}"); }
    }

    #[test]
    fn config_load_survives_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let t = TempDir::new("cfg-bytes");
        let paths = Paths::under(&t.join("env"));
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), &bytes).unwrap();
        let c = Config::load(&paths);
        prop_assert_eq!(c.version, ashdash_core::config::CURRENT_VERSION);
    }
}

#[test]
fn config_load_roundtrip_and_version_zero_migration() {
    let t = TempDir::new("cfg-mig");
    let paths = Paths::under(&t.join("env"));
    std::fs::create_dir_all(&paths.data).unwrap();
    // an old file with no version field and a custom value
    std::fs::write(paths.config_file(), br#"{"max_results": 7}"#).unwrap();
    let c = Config::load(&paths);
    assert_eq!(c.max_results, 7);
    assert_eq!(c.version, ashdash_core::config::CURRENT_VERSION);
    c.save(&paths).unwrap();
    assert_eq!(Config::load(&paths).max_results, 7);
}

// ---------------------------------------------------------------- config v2 fields and dotted paths

/// Hostile-but-plausible JSON for one setting: right types at the extremes, wrong types, junk.
fn arb_setting_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        Just(json!(0)),
        Just(json!(-1)),
        Just(json!(u64::MAX)),
        Just(json!(i64::MIN)),
        Just(json!(1.5)),
        Just(json!(u32::MAX)),
        Just(json!("")),
        Just(json!("\u{0}")),
        Just(json!("~/../../..")),
        Just(json!("/")),
        Just(json!(["a", 1, null, ["nested"]])),
        Just(json!([])),
        Just(json!({})),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        "\\PC{0,24}".prop_map(Value::from),
        prop::collection::vec("\\PC{0,12}", 0..5).prop_map(|v| json!(v)),
        arb_json(),
    ]
}

/// Every key Config v2 added (and the older ones), with per-field hostile values.
const V2_KEYS: &[(&str, &[&str])] = &[
    ("bookmarks", &["enabled_sources", "include_in_general_search"]),
    (
        "file_search",
        &[
            "roots",
            "excluded_paths",
            "excluded_names",
            "excluded_substrings",
            "include_hidden",
            "include_icloud_drive",
            "mdfind_timeout_ms",
            "mdfind_max_bytes",
            "walk_timeout_ms",
            "walk_max_entries",
        ],
    ),
    (
        "clipboard",
        &["max_entries", "max_age_days", "ignored_apps", "max_image_bytes"],
    ),
];

fn arb_v2_config_json() -> impl Strategy<Value = Value> {
    let sections = V2_KEYS
        .iter()
        .map(|(name, fields)| {
            let fields: Vec<&'static str> = fields.to_vec();
            prop::collection::vec((prop::sample::select(fields), arb_setting_value()), 0..6).prop_map(move |kv| {
                (
                    name.to_string(),
                    Value::Object(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect()),
                )
            })
        })
        .collect::<Vec<_>>();
    (
        sections,
        arb_setting_value(),
        arb_setting_value(),
        arb_setting_value(),
        arb_setting_value(),
        prop::sample::select(vec![0u64, 1, 2, 3, u64::MAX]),
    )
        .prop_map(|(secs, providers, results, web, hotkey, version)| {
            let mut m: serde_json::Map<String, Value> = secs.into_iter().collect();
            m.insert("providers_disabled".into(), providers);
            m.insert("max_results".into(), results);
            m.insert("web_searches".into(), web);
            m.insert("hotkey".into(), hotkey);
            m.insert("version".into(), json!(version));
            Value::Object(m)
        })
}

/// An OS that does nothing and finds nothing: the engine below must never reach the real machine.
struct NullOs;
impl ashdash_core::actions::Os for NullOs {
    fn run(&self, _: &ashdash_core::actions::Cmd) -> anyhow::Result<()> {
        Ok(())
    }
    fn output(&self, _: &ashdash_core::actions::Cmd) -> anyhow::Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn spawn(&self, _: &ashdash_core::actions::Cmd) -> anyhow::Result<Box<dyn ashdash_core::actions::Process>> {
        anyhow::bail!("no processes in this test")
    }
}

proptest! {
    #![proptest_config(cfg(300))]

    /// Hostile values in every v2 section: loading never panics, the result can be saved and
    /// loaded again unchanged, and numeric budgets always end up inside their documented ranges.
    #[test]
    fn config_v2_fields_survive_hostile_values(doc in arb_v2_config_json()) {
        use ashdash_core::config::{
            MDFIND_MAX_BYTES_RANGE, MDFIND_TIMEOUT_MS_RANGE, WALK_MAX_ENTRIES_RANGE, WALK_TIMEOUT_MS_RANGE,
        };
        let t = TempDir::new("cfg-v2");
        let paths = Paths::under(&t.join("env"));
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), serde_json::to_vec(&doc).unwrap()).unwrap();
        let c = Config::load(&paths);
        let _ = Config::load_strict(&paths); // may be Err, must not panic
        prop_assert!(MDFIND_TIMEOUT_MS_RANGE.contains(&c.file_search.mdfind_timeout_ms));
        prop_assert!(MDFIND_MAX_BYTES_RANGE.contains(&c.file_search.mdfind_max_bytes));
        prop_assert!(WALK_TIMEOUT_MS_RANGE.contains(&c.file_search.walk_timeout_ms));
        prop_assert!(WALK_MAX_ENTRIES_RANGE.contains(&c.file_search.walk_max_entries));
        let _ = c.file_search.to_scope_in(t.path()); // budget + tilde expansion on junk
        let future = doc.get("version").and_then(Value::as_u64).is_some_and(|v| v > ashdash_core::config::CURRENT_VERSION as u64);
        if !future {
            c.save(&paths).unwrap();
            let again = Config::load(&paths);
            prop_assert_eq!(serde_json::to_value(&again).unwrap(), serde_json::to_value(&c).unwrap());
        }
    }
}

proptest! {
    // Building an engine starts index threads: fewer cases.
    #![proptest_config(cfg(24))]

    /// Whatever the settings file says, an engine built from it answers queries without panicking
    /// (no file queries: those would walk the real disk for hostile roots).
    #[test]
    fn engine_built_from_hostile_v2_settings_answers_queries(doc in arb_v2_config_json()) {
        let t = TempDir::new("cfg-engine");
        let paths = Paths::under(&t.join("env"));
        std::fs::create_dir_all(&paths.data).unwrap();
        std::fs::write(paths.config_file(), serde_json::to_vec(&doc).unwrap()).unwrap();
        let mut c = Config::load(&paths);
        c.app_dirs = vec![t.join("apps")]; // never scan the real machine
        c.workflow_dirs.clear();
        let home = t.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let e = ashdash_core::Engine::with_env(paths, c, Box::new(NullOs), home);
        for q in ["a", "clip x", "clipboard", "snip x", "bm x", "2+2", "g rust", ">ls", "sys sleep", "  "] {
            let _ = e.query(q);
        }
    }
}

/// Dotted-path edits as `ashdash config set` performs them.
fn arb_path() -> impl Strategy<Value = String> {
    let real = Config::default();
    let keys = ashdash_core::settings_view::keys(&real);
    prop_oneof![
        prop::sample::select(keys),
        prop::sample::select(vec![
            "",
            ".",
            "..",
            "a..b",
            "clipboard.",
            ".clipboard",
            "clipboard..max_entries",
            "web_searches.0",
            "web_searches.0.url",
            "web_searches.99999999999999999999",
            "web_searches.-1",
            "web_searches.00",
            "file_search.roots.0",
            "file_search.roots.99",
            "providers_disabled.0",
            "theme.x",
            "version",
            "bookmarks",
            "clipboard",
            "file_search",
            "hotkey.x",
            "max_results.1",
        ])
        .prop_map(String::from),
        "[a-z_.0-9]{0,24}",
        "\\PC{0,24}",
    ]
}

fn arb_raw_value() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("null".to_string()),
        Just("[]".to_string()),
        Just("{}".to_string()),
        Just("-1".to_string()),
        Just("18446744073709551616".to_string()),
        Just("1e400".to_string()),
        Just("\"\u{0}\"".to_string()),
        Just("a,b,,c".to_string()),
        "\\PC{0,32}",
        arb_json().prop_map(|v| v.to_string()),
        any::<i64>().prop_map(|n| n.to_string()),
    ]
}

proptest! {
    #![proptest_config(cfg(1000))]

    /// `settings_view::set` never panics; a rejected edit changes nothing; an accepted one reads
    /// back as stored, survives save + load and keeps the file loadable.
    #[test]
    fn config_set_with_dotted_paths_is_total_and_atomic(path in arb_path(), raw in arb_raw_value()) {
        use ashdash_core::settings_view::{get, set};
        let mut c = Config::default();
        let before = serde_json::to_value(&c).unwrap();
        match set(&mut c, &path, &raw) {
            Err(_) => prop_assert_eq!(serde_json::to_value(&c).unwrap(), before, "failed set must not change anything"),
            Ok(()) => {
                prop_assert!(get(&c, &path).is_ok(), "an accepted key can be read back");
                let t = TempDir::new("cfg-set");
                let paths = Paths::under(&t.join("env"));
                c.save(&paths).unwrap();
                let back = Config::load_strict(&paths);
                prop_assert!(back.is_ok(), "saved settings load strictly: {back:?}");
                prop_assert_eq!(serde_json::to_value(back.unwrap()).unwrap(), serde_json::to_value(&c).unwrap());
            }
        }
    }

    /// Sequences of edits on real keys never leave a configuration that cannot be saved and loaded.
    #[test]
    fn config_set_sequences_keep_the_file_loadable(
        edits in prop::collection::vec((prop::sample::select(ashdash_core::settings_view::keys(&Config::default())), arb_raw_value()), 1..8)
    ) {
        let mut c = Config::default();
        for (k, v) in &edits {
            let _ = ashdash_core::settings_view::set(&mut c, k, v);
            // in-memory round trip after every edit (cheap) ...
            let again: Result<Config, _> = serde_json::from_value(serde_json::to_value(&c).unwrap());
            prop_assert!(again.is_ok(), "after {k}={v:?}: {again:?}");
        }
        // ... and one real save + strict load at the end
        let t = TempDir::new("cfg-seq");
        let paths = Paths::under(&t.join("env"));
        c.save(&paths).unwrap();
        prop_assert!(Config::load_strict(&paths).is_ok());
    }
}

// ---------------------------------------------------------------- Script Filter output

/// A Script Filter whose script is `cat <file>`: the file holds the fuzz bytes verbatim.
fn script_filter_from_file(file: &std::path::Path) -> Value {
    script_filter_workflow("fz", &format!("cat '{}'", file.display()))
}

fn run_filter(t: &TempDir, paths: &Paths, payload: &[u8]) -> Vec<ashdash_core::Item> {
    let file = t.join("payload");
    std::fs::write(&file, payload).unwrap();
    let wf_dir = t.join("wf");
    std::fs::create_dir_all(&wf_dir).unwrap();
    let wf = Workflow::from_json(&wf_dir, "wf", &script_filter_from_file(&file)).unwrap();
    wf.keyword_items("fz query", paths).0
}

proptest! {
    #![proptest_config(cfg(96))]

    #[test]
    fn script_filter_survives_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let t = TempDir::new("sf-bytes");
        let paths = Paths::under(&t.join("env"));
        let items = run_filter(&t, &paths, &bytes);
        prop_assert!(items.len() <= 10_000);
    }

    #[test]
    fn script_filter_survives_json_with_wrong_types(doc in arb_json()) {
        let t = TempDir::new("sf-json");
        let paths = Paths::under(&t.join("env"));
        let _ = run_filter(&t, &paths, &serde_json::to_vec(&doc).unwrap());
        // same, but wrapped in the expected top-level shape
        let wrapped = json!({"items": [doc.clone(), {"title": doc.clone(), "subtitle": doc.clone(), "arg": doc.clone(),
            "variables": doc.clone(), "mods": doc.clone(), "icon": doc.clone(), "valid": doc.clone(),
            "uid": doc.clone(), "autocomplete": doc.clone(), "text": doc.clone(), "quicklookurl": doc.clone()}],
            "variables": doc.clone(), "rerun": doc.clone(), "cache": doc});
        let _ = run_filter(&t, &paths, &serde_json::to_vec(&wrapped).unwrap());
    }

    #[test]
    fn script_filter_survives_mangled_valid_json(cut in 0usize..220, flip in 0usize..220, byte in any::<u8>()) {
        let good = br#"{"items":[{"title":"Hello \u00e9","subtitle":"s","arg":"a b","uid":"1","valid":true,
            "variables":{"k":"v"},"mods":{"cmd":{"arg":"x","subtitle":"y","valid":false}},"icon":{"path":"i.png"}}],"rerun":1}"#.to_vec();
        let mut v = good.clone();
        let n = v.len();
        v[flip % n] = byte;
        v.truncate(cut.min(n).max(1));
        let t = TempDir::new("sf-mangle");
        let paths = Paths::under(&t.join("env"));
        let _ = run_filter(&t, &paths, &v);
    }
}

/// Item text coming out of a Script Filter keeps NULs, newlines and BiDi controls out of ids
/// the engine will later use as keys; mostly this documents behaviour.
#[test]
fn script_filter_hostile_strings_come_through_as_data() {
    let t = TempDir::new("sf-hostile");
    let paths = Paths::under(&t.join("env"));
    let payload = json!({"items": [{
        "title": "a\u{0}b\u{202e}c\n\r\t",
        "arg": "$(touch /tmp/never)",
        "subtitle": "`x`",
        "variables": {"a b": "c\nd", "": "empty-key"}
    }]});
    let items = run_filter(&t, &paths, &serde_json::to_vec(&payload).unwrap());
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0].action, Action::Workflow(_)));
}
