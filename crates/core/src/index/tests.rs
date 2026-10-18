use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use super::scan::{Root, Target, classify};
use super::*;

static SEQ: AtomicU32 = AtomicU32::new(0);

struct Env {
    root: PathBuf,
    apps: PathBuf,
    paths: Paths,
}

impl Env {
    fn new(tag: &str) -> Env {
        let root = std::env::temp_dir().join(format!(
            "oa-index-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let apps = root.join("apps");
        std::fs::create_dir_all(&apps).unwrap();
        Env {
            paths: Paths::under(&root),
            root,
            apps,
        }
    }

    fn cache_file(&self) -> PathBuf {
        self.paths.cache.join(store::FILE_NAME)
    }

    fn dirs(&self) -> Vec<PathBuf> {
        vec![self.apps.clone()]
    }

    fn open(&self, opts: IndexOptions) -> (AppIndex, Arc<RwLock<Vec<AppEntry>>>) {
        let live = Arc::new(RwLock::new(Vec::new()));
        let idx = AppIndex::open_with(&self.paths, &self.dirs(), live.clone(), opts);
        (idx, live)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const QUIET: IndexOptions = IndexOptions {
    watch: false,
    refresh_on_start: false,
};

fn plist(background: bool) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleName</key><string>X</string><key>LSBackgroundOnly</key><{}/></dict></plist>"#,
        background
    )
}

fn fake_app(dir: &Path, name: &str) -> PathBuf {
    let app = dir.join(format!("{name}.app"));
    std::fs::create_dir_all(app.join("Contents")).unwrap();
    std::fs::write(app.join("Contents/Info.plist"), plist(false)).unwrap();
    app
}

#[cfg(unix)]
#[test]
fn a_symlinked_bundle_does_not_hide_the_apps_after_it() {
    // Safari is a symlink into a cryptex on macOS 13+. walkdir's `skip_current_dir` on a symlink entry used to skip the rest of the
    // parent directory, so the result depended on directory order (seen on GitHub-hosted runners: one Xcode and no Safari).
    let env = Env::new("symlink-order");
    let elsewhere = env.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut expected = Vec::new();
    for i in 0..20 {
        fake_app(&env.apps, &format!("Real{i:02}"));
        expected.push(format!("Real{i:02}"));
        let target = fake_app(&elsewhere, &format!("Target{i:02}"));
        std::os::unix::fs::symlink(&target, env.apps.join(format!("Link{i:02}.app"))).unwrap();
        expected.push(format!("Link{i:02}"));
    }
    let (_idx, live) = env.open(QUIET);
    let mut got = names(&live);
    got.sort();
    expected.sort();
    assert_eq!(
        got, expected,
        "every real and every linked bundle is listed, whatever order the directory returns them in"
    );
}

fn names(live: &RwLock<Vec<AppEntry>>) -> Vec<String> {
    live.read().unwrap().iter().map(|a| a.name.clone()).collect()
}

/// Poll `cond` for a few seconds: FSEvents latency is not under our control.
fn wait_until(what: &str, cond: impl Fn() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for: {what}");
}

#[test]
fn first_run_scans_synchronously_and_writes_a_private_versioned_cache() {
    let env = Env::new("first");
    fake_app(&env.apps, "Zeta");
    fake_app(&env.apps, "alpha");
    let (idx, live) = env.open(QUIET);
    assert!(!idx.loaded_from_cache());
    assert!(idx.refreshed());
    // case-insensitive order, ready before open() returned
    assert_eq!(names(&live), ["alpha", "Zeta"]);

    let text = std::fs::read_to_string(env.cache_file()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["version"], INDEX_VERSION);
    assert_eq!(json["apps"].as_array().unwrap().len(), 2);
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(env.cache_file()).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn warm_cache_is_served_without_touching_the_directories() {
    let env = Env::new("warm");
    fake_app(&env.apps, "Safari");
    fake_app(&env.apps, "Notes");
    drop(env.open(QUIET));

    // Remove the bundles: only the cache can still know about them.
    std::fs::remove_dir_all(&env.apps).unwrap();
    std::fs::create_dir_all(&env.apps).unwrap();
    let (idx, live) = env.open(QUIET);
    assert!(idx.loaded_from_cache());
    assert_eq!(names(&live), ["Notes", "Safari"]);
}

#[test]
fn background_refresh_corrects_a_stale_cache() {
    let env = Env::new("refresh");
    fake_app(&env.apps, "Old");
    drop(env.open(QUIET));

    std::fs::remove_dir_all(env.apps.join("Old.app")).unwrap();
    fake_app(&env.apps, "New");
    let opts = IndexOptions {
        watch: false,
        refresh_on_start: true,
    };
    let (idx, live) = env.open(opts);
    assert!(idx.loaded_from_cache());
    wait_until("refresh finished", || idx.refreshed());
    assert_eq!(names(&live), ["New"]);
    // and the corrected list was persisted
    let (_idx2, live2) = env.open(QUIET);
    assert_eq!(names(&live2), ["New"]);
}

#[test]
fn rescan_reports_whether_anything_changed() {
    let env = Env::new("rescan");
    fake_app(&env.apps, "One");
    let (idx, live) = env.open(QUIET);
    assert!(!idx.rescan());
    fake_app(&env.apps, "Two");
    assert!(idx.rescan());
    assert_eq!(names(&live), ["One", "Two"]);
    assert!(!idx.rescan());
}

fn write_cache(env: &Env, body: &[u8]) {
    std::fs::create_dir_all(&env.paths.cache).unwrap();
    std::fs::write(env.cache_file(), body).unwrap();
}

fn assert_falls_back_to_scan(env: &Env) {
    let (idx, live) = env.open(QUIET);
    assert!(!idx.loaded_from_cache(), "bad cache must not be trusted");
    assert_eq!(names(&live), ["Real"]);
    // the bad file was replaced by a good one
    assert!(store::load(&env.cache_file(), &env.dirs()).is_ok());
}

#[test]
fn corrupt_cache_falls_back_to_a_scan() {
    for (i, junk) in [
        &b""[..],
        b"not json at all",
        b"{\"version\":1,\"dirs\":",
        b"\xff\xfe\x00\x01",
        b"[1,2,3]",
        b"{\"version\":1}",
    ]
    .into_iter()
    .enumerate()
    {
        let env = Env::new(&format!("corrupt{i}"));
        fake_app(&env.apps, "Real");
        write_cache(&env, junk);
        assert_eq!(
            store::load(&env.cache_file(), &env.dirs()).unwrap_err(),
            Miss::Corrupt,
            "{junk:?}"
        );
        assert_falls_back_to_scan(&env);
    }
}

#[test]
fn other_versions_are_ignored_not_misread() {
    let env = Env::new("version");
    fake_app(&env.apps, "Real");
    let ghost = env.apps.join("Ghost.app");
    for v in [0u32, INDEX_VERSION + 1, 999] {
        let body = serde_json::json!({
            "version": v,
            "dirs": env.dirs(),
            "apps": [{"name":"Ghost","path":ghost,"mtime":1,"background":false}],
        });
        write_cache(&env, body.to_string().as_bytes());
        assert_eq!(
            store::load(&env.cache_file(), &env.dirs()).unwrap_err(),
            Miss::Version(v)
        );
        assert_falls_back_to_scan(&env);
    }
    // a future file with a different shape still reports its version
    write_cache(&env, br#"{"version": 7, "entries": "different layout"}"#);
    assert_eq!(
        store::load(&env.cache_file(), &env.dirs()).unwrap_err(),
        Miss::Version(7)
    );
}

#[test]
fn cache_for_other_directories_is_a_miss() {
    let env = Env::new("dirs");
    fake_app(&env.apps, "Real");
    drop(env.open(QUIET));
    let other = vec![env.apps.clone(), env.root.join("more")];
    assert_eq!(store::load(&env.cache_file(), &other).unwrap_err(), Miss::DifferentDirs);
}

#[test]
fn implausible_records_are_dropped_from_a_tampered_cache() {
    let env = Env::new("tamper");
    let good = env.apps.join("Good.app");
    let rec = |name: &str, path: &Path| serde_json::json!({"name":name,"path":path,"mtime":1,"background":false});
    let body = serde_json::json!({
        "version": INDEX_VERSION,
        "dirs": env.dirs(),
        "apps": [
            rec("Good", &good),
            rec("Evil", Path::new("/bin/sh")),
            rec("Elsewhere", Path::new("/tmp/Elsewhere.app")),
            rec("Mismatch", &env.apps.join("Other.app")),
            rec("Rel", Path::new("Rel.app")),
        ],
    });
    write_cache(&env, body.to_string().as_bytes());
    let (idx, live) = env.open(QUIET);
    assert!(idx.loaded_from_cache());
    assert_eq!(names(&live), ["Good"]);
}

#[test]
fn background_only_apps_are_kept_out_of_the_list_but_not_reparsed() {
    let env = Env::new("bg");
    fake_app(&env.apps, "Visible");
    let helper = fake_app(&env.apps, "Helper");
    std::fs::write(helper.join("Contents/Info.plist"), plist(true)).unwrap();
    let (idx, live) = env.open(QUIET);
    assert_eq!(names(&live), ["Visible"]);
    let cached = store::load(&env.cache_file(), &env.dirs()).unwrap();
    assert_eq!(cached.len(), 2, "helper is remembered");
    assert!(!idx.rescan());
}

#[test]
fn a_changed_info_plist_is_re_read() {
    let env = Env::new("mtime");
    let app = fake_app(&env.apps, "Flip");
    let (idx, live) = env.open(QUIET);
    assert_eq!(names(&live), ["Flip"]);
    std::fs::write(app.join("Contents/Info.plist"), plist(true)).unwrap();
    // make sure the stamp moves even on coarse-grained file systems
    let f = std::fs::OpenOptions::new()
        .write(true)
        .open(app.join("Contents/Info.plist"))
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    assert!(idx.rescan());
    assert!(names(&live).is_empty());
}

#[test]
fn duplicate_directories_and_nested_bundles() {
    let env = Env::new("nested");
    fake_app(&env.apps, "Top");
    fake_app(&env.apps.join("Utilities"), "Deep");
    // bundles inside bundles are not separate apps
    std::fs::create_dir_all(env.apps.join("Top.app/Contents/Helpers/Inner.app")).unwrap();
    // beyond the depth limit
    std::fs::create_dir_all(env.apps.join("a/b/c/TooDeep.app")).unwrap();
    let live = Arc::new(RwLock::new(Vec::new()));
    let dirs = vec![env.apps.clone(), env.apps.clone()];
    let _idx = AppIndex::open_with(&env.paths, &dirs, live.clone(), QUIET);
    assert_eq!(names(&live), ["Deep", "Top"]);
}

#[test]
fn watcher_adds_and_removes_apps_incrementally() {
    let env = Env::new("watch");
    fake_app(&env.apps, "Existing");
    let (idx, live) = env.open(IndexOptions::default());
    assert!(idx.watching(), "FSEvents watcher should arm on a temp dir");
    assert_eq!(names(&live), ["Existing"]);

    let added = fake_app(&env.apps, "Brand New");
    wait_until("new app appears", || names(&live).contains(&"Brand New".to_string()));

    // a sub-folder created later, with an app inside
    fake_app(&env.apps.join("Utilities"), "Nested Tool");
    wait_until("nested app appears", || {
        names(&live).contains(&"Nested Tool".to_string())
    });

    std::fs::remove_dir_all(&added).unwrap();
    wait_until("removed app disappears", || {
        !names(&live).contains(&"Brand New".to_string())
    });

    std::fs::remove_dir_all(env.apps.join("Utilities")).unwrap();
    wait_until("removed folder prunes its apps", || {
        !names(&live).contains(&"Nested Tool".to_string())
    });
    assert_eq!(names(&live), ["Existing"]);

    // the cache follows the changes
    wait_until("cache persisted", || {
        store::load(&env.cache_file(), &env.dirs()).is_ok_and(|r| r.len() == 1)
    });
}

#[test]
fn nothing_is_published_while_the_file_system_is_quiet() {
    let env = Env::new("quiet");
    fake_app(&env.apps, "A");
    let (idx, live) = env.open(IndexOptions::default());
    assert!(idx.watching());
    let before = std::fs::metadata(env.cache_file()).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(600));
    let after = std::fs::metadata(env.cache_file()).unwrap().modified().unwrap();
    assert_eq!(before, after);
    assert_eq!(names(&live), ["A"]);
}

#[test]
fn readers_never_see_a_half_built_list_during_refreshes() {
    let env = Env::new("consistent");
    for i in 0..40 {
        fake_app(&env.apps, &format!("Steady {i}"));
    }
    let (idx, live) = env.open(QUIET);
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let (live, stop) = (live.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut reads = 0u32;
            while !stop.load(Ordering::SeqCst) {
                let n = live.read().unwrap().len();
                assert!(n >= 40, "list shrank to {n} during a refresh");
                reads += 1;
            }
            reads
        })
    };
    for i in 0..30 {
        fake_app(&env.apps, &format!("Churn {i}"));
        idx.rescan();
        let _ = std::fs::remove_dir_all(env.apps.join(format!("Churn {i}.app")));
        idx.rescan();
    }
    stop.store(true, Ordering::SeqCst);
    assert!(reader.join().unwrap() > 0);
    assert_eq!(live.read().unwrap().len(), 40);
}

#[test]
fn big_changes_rebuild_the_list_and_stay_sorted() {
    let env = Env::new("big");
    fake_app(&env.apps, "Middle");
    let (idx, live) = env.open(QUIET);
    for i in 0..(REBUILD_THRESHOLD + 10) {
        fake_app(&env.apps, &format!("Bulk {i:03}"));
    }
    assert!(idx.rescan());
    let n = names(&live);
    assert_eq!(n.len(), REBUILD_THRESHOLD + 11);
    let mut sorted = n.clone();
    sorted.sort_by_key(|s| s.to_lowercase());
    assert_eq!(n, sorted);
}

#[test]
fn classify_maps_events_to_the_smallest_piece() {
    let root = Path::new("/apps");
    let roots = [Root::new(root)];
    let c = |p: &str| classify(&roots, Path::new(p));
    assert_eq!(c("/apps/Foo.app"), Some(Target::Bundle("/apps/Foo.app".into())));
    // anything inside a bundle belongs to the outermost bundle
    assert_eq!(
        c("/apps/Foo.app/Contents/Info.plist"),
        Some(Target::Bundle("/apps/Foo.app".into()))
    );
    assert_eq!(
        c("/apps/Sub/Bar.app/Contents/MacOS/bar"),
        Some(Target::Bundle("/apps/Sub/Bar.app".into()))
    );
    assert_eq!(
        c("/apps/Sub"),
        Some(Target::Subtree {
            path: "/apps/Sub".into(),
            depth: MAX_DEPTH - 1
        })
    );
    assert_eq!(
        c("/apps"),
        Some(Target::Subtree {
            path: "/apps".into(),
            depth: MAX_DEPTH
        })
    );
    // deeper than apps are looked for: irrelevant
    assert_eq!(c("/apps/a/b/c/file.txt"), None);
    assert_eq!(c("/apps/a/b/c/Too.app"), None);
    assert_eq!(c("/elsewhere/Foo.app"), None);
    assert_eq!(c("/apps/../etc/Foo.app"), None);
}

#[test]
fn scan_entries_matches_the_index_view() {
    let env = Env::new("oneshot");
    fake_app(&env.apps, "b");
    fake_app(&env.apps, "A");
    let one: Vec<String> = scan_entries(&env.dirs()).into_iter().map(|a| a.name).collect();
    assert_eq!(one, ["A", "b"]);
}

/// Generous ceiling for the whole engine query path at 5,000 apps (release target is 10 ms;
/// this guards against accidental O(n^2) or per-candidate allocation, in debug builds too).
#[test]
fn query_latency_p95_at_5000_apps_stays_under_the_ceiling() {
    let env = Env::new("latency");
    synth::make_fake_apps(&env.apps, 5_000, false).unwrap();
    let e = crate::Engine::new(
        env.paths.clone(),
        crate::Config {
            app_dirs: env.dirs(),
            ..crate::Config::default()
        },
    );
    assert!(e.app_count() >= 5_000);
    // Warm up (first calls pay for page faults and lazy init), then measure in independent
    // rounds. Parallel tests and a debug build make any single p95 noisy: a descheduled
    // thread adds tens of milliseconds to a handful of samples. A real regression (O(n^2),
    // per-candidate allocation) slows EVERY round, so the best round's p95 is the figure that
    // is compared with the ceiling; every round is printed for diagnosis.
    for q in ["saf", "vsc", "visual studio", "x"] {
        assert!(!e.query(q).is_empty(), "{q}");
        for _ in 0..3 {
            std::hint::black_box(e.query(q));
        }
    }
    assert_eq!(e.query("saf")[0].title, "Safari");
    assert_eq!(e.query("vsc")[0].title, "Visual Studio Code");
    let p95_of = |mut v: Vec<Duration>| {
        v.sort();
        v[(v.len() * 95 / 100).min(v.len() - 1)]
    };
    let rounds: Vec<Duration> = (0..5)
        .map(|_| {
            let mut samples = Vec::new();
            for q in ["saf", "vsc", "visual studio", "x"] {
                for _ in 0..20 {
                    let t = Instant::now();
                    std::hint::black_box(e.query(q));
                    samples.push(t.elapsed());
                }
            }
            p95_of(samples)
        })
        .collect();
    let best = *rounds.iter().min().unwrap();
    eprintln!("p95 per round at 5,000 apps: {rounds:?}; best {best:?}");
    assert!(
        best < Duration::from_millis(25),
        "best-round p95 = {best:?} (all rounds: {rounds:?})"
    );
}

#[test]
fn dropping_the_index_stops_its_background_thread() {
    let env = Env::new("drop");
    fake_app(&env.apps, "A");
    drop(env.open(QUIET)); // write a cache so the next open refreshes in the background
    let (idx, live) = env.open(IndexOptions::default());
    wait_until("initial refresh", || idx.refreshed() && idx.watching());
    assert!(Arc::strong_count(&live) > 1, "worker holds the list while running");
    drop(idx);
    // the worker owns the last other reference; it releases it when it exits
    wait_until("worker exits", || Arc::strong_count(&live) == 1);
}
