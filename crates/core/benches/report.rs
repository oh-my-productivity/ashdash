//! Human-readable performance report (not criterion): `cargo bench -p ashdash-core --bench report`.
//!
//! Prints p95 query latency over a synthetic 5,000-app index, cold start with and without a
//! warm index cache, the scan time of this machine's real application folders (counts only)
//! and the idle CPU of the headless engine. Uses only public API so it also runs on older
//! revisions for before/after comparisons.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ashdash_core::{Config, Engine, Paths, apps, fuzzy};

/// App counts measured by one run; override with `ASHDASH_BENCH_APPS=500,5000`.
const DEFAULT_APP_COUNTS: &[usize] = &[500, 5_000];
const QUERIES: [&str; 4] = ["saf", "vsc", "visual studio", "x"];

fn words() -> Vec<&'static str> {
    vec![
        "Visual",
        "Studio",
        "Code",
        "Safari",
        "Preview",
        "Terminal",
        "Photo",
        "Booth",
        "Final",
        "Cut",
        "Pro",
        "Logic",
        "Xcode",
        "Docker",
        "Desktop",
        "Slack",
        "Zoom",
        "Notes",
        "Music",
        "Maps",
        "Mail",
        "Calendar",
        "Reminders",
        "Keynote",
        "Pages",
        "Numbers",
        "Transmit",
        "Sketch",
        "Figma",
        "Postman",
        "Insomnia",
        "Tower",
        "Fork",
        "Kaleidoscope",
        "Xmind",
        "Audacity",
        "Handbrake",
        "Inkscape",
        "Blender",
        "Unity",
        "Hub",
        "Editor",
        "Helper",
        "Manager",
        "Pad",
    ]
}

fn synthetic_names(n: usize) -> Vec<String> {
    let w = words();
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..n)
        .map(|i| {
            let k = 1 + (next() % 3) as usize;
            let mut s = String::new();
            for _ in 0..k {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(w[(next() % w.len() as u64) as usize]);
            }
            format!("{s} {i}")
        })
        .collect()
}

fn make_apps(dir: &Path, n_apps: usize) {
    let plist = r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleName</key><string>X</string><key>CFBundleIdentifier</key><string>com.example.x</string><key>CFBundleIconFile</key><string>icon</string></dict></plist>"#;
    for (i, name) in synthetic_names(n_apps).iter().enumerate() {
        let contents = dir.join(format!("{name}.app")).join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(contents.join("Info.plist"), plist).unwrap();
        if i == 0 {
            std::fs::create_dir_all(dir.join("Safari.app")).unwrap();
            std::fs::create_dir_all(dir.join("Visual Studio Code.app")).unwrap();
        }
    }
}

fn pct(samples: &mut [Duration], p: f64) -> Duration {
    samples.sort();
    samples[((samples.len() as f64 * p).ceil() as usize).min(samples.len()) - 1]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn cpu_time(pid: u32) -> Option<(f64, u64)> {
    let out = Command::new("ps")
        .args(["-o", "cputime=,rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut it = s.split_whitespace();
    let t = it.next()?;
    let rss: u64 = it.next()?.parse().ok()?;
    // formats: M:SS.cc or H:MM:SS.cc
    let secs = t
        .split(':')
        .try_fold(0.0, |acc, p| p.parse::<f64>().ok().map(|v| acc * 60.0 + v))?;
    Some((secs, rss))
}

fn apps_report(n_apps: usize) {
    println!("\n=== {n_apps} apps ===");
    let root: PathBuf = std::env::temp_dir().join(format!("oa-report-{}-{n_apps}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let apps_dir = root.join("apps");
    let t = Instant::now();
    make_apps(&apps_dir, n_apps);
    println!(
        "synthetic apps created: {n_apps} in {:.0} ms (setup, not measured)",
        ms(t.elapsed())
    );

    let config = || Config {
        app_dirs: vec![apps_dir.clone()],
        ..Config::default()
    };
    let paths = Paths::under(&root);

    // --- cold start with no cache (first ever run) and with a warm cache
    let t = Instant::now();
    let e = Engine::new(paths.clone(), config());
    let new_ms = ms(t.elapsed());
    let t2 = Instant::now();
    let _ = e.query("saf");
    println!(
        "start (no cache): Engine::new {new_ms:.1} ms + first query {:.1} ms, apps={}",
        ms(t2.elapsed()),
        e.app_count()
    );
    drop(e);
    std::thread::sleep(Duration::from_millis(500));
    let mut warm = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let e = Engine::new(paths.clone(), config());
        let r = e.query("saf");
        warm.push(t.elapsed());
        assert!(!r.is_empty());
        drop(e);
    }
    println!(
        "start (warm cache, best/median of 5): to first query ready {:.1} / {:.1} ms",
        ms(*warm.iter().min().unwrap()),
        ms(pct(&mut warm.clone(), 0.5))
    );

    // --- query latency
    let e = Engine::new(paths.clone(), config());
    // wait for a possible background refresh to settle so we measure steady state
    std::thread::sleep(Duration::from_secs(2));
    let mut all = Vec::new();
    for q in QUERIES {
        for _ in 0..20 {
            let _ = e.query(q);
        }
        let mut s = Vec::new();
        for _ in 0..300 {
            let t = Instant::now();
            let r = e.query(q);
            s.push(t.elapsed());
            std::hint::black_box(r);
        }
        println!(
            "engine.query({q:?}) p50 {:.3} ms  p95 {:.3} ms  max {:.3} ms",
            ms(pct(&mut s.clone(), 0.5)),
            ms(pct(&mut s.clone(), 0.95)),
            ms(*s.iter().max().unwrap())
        );
        all.extend(s);
    }
    println!(
        "engine.query overall p95 {:.3} ms (apps={})",
        ms(pct(&mut all, 0.95)),
        e.app_count()
    );

    // --- bare fuzzy loop
    let names = synthetic_names(n_apps);
    for q in QUERIES {
        let mut s = Vec::new();
        for _ in 0..200 {
            let t = Instant::now();
            let mut hits = 0;
            for n in &names {
                if fuzzy::score(q, n).is_some() {
                    hits += 1;
                }
            }
            std::hint::black_box(hits);
            s.push(t.elapsed());
        }
        println!(
            "fuzzy::score loop over {n_apps} names, {q:?}: p95 {:.3} ms",
            ms(pct(&mut s, 0.95))
        );
    }

    // --- idle CPU of the headless engine
    if let Some((c0, rss)) = cpu_time(std::process::id()) {
        std::thread::sleep(Duration::from_secs(5));
        let (c1, _) = cpu_time(std::process::id()).unwrap_or((c0, rss));
        println!(
            "idle: {:.2} cpu-seconds over 5 s (ps resolution 10 ms), rss {} MB",
            c1 - c0,
            rss / 1024
        );
    } else {
        println!("idle CPU: n/a");
    }
    drop(e);

    let _ = std::fs::remove_dir_all(&root);
}

fn real_scan() {
    println!("\n=== real application folders ===");
    let mut dirs = vec![PathBuf::from("/Applications"), PathBuf::from("/System/Applications")];
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(h).join("Applications"));
    }
    for _ in 0..1 {
        let t = Instant::now();
        let found = apps::scan(&dirs);
        println!(
            "real scan of {:?}: {} apps in {:.1} ms",
            dirs,
            found.len(),
            ms(t.elapsed())
        );
    }
}

/// Clipboard history search at the default cap (200) and at larger histories.
fn clipboard_report() {
    println!("\n=== clipboard history search (in memory, synthetic text) ===");
    let names = synthetic_names(5_000);
    for n in [200usize, 1_000, 5_000] {
        let mut h = ashdash_core::clipboard::history::ClipboardHistory::in_memory(n);
        for (i, name) in names.iter().take(n).enumerate() {
            h.push(&format!("{name} clipboard entry {i} lorem ipsum dolor sit amet"));
        }
        for q in ["saf", "visual studio", "zzz-no-match"] {
            for _ in 0..10 {
                std::hint::black_box(h.search(q, 20));
            }
            let mut s = Vec::new();
            for _ in 0..200 {
                let t = Instant::now();
                std::hint::black_box(h.search(q, 20));
                s.push(t.elapsed());
            }
            println!(
                "clipboard.search entries={} {q:?}: p50 {:.3} ms  p95 {:.3} ms",
                h.len(),
                ms(pct(&mut s.clone(), 0.5)),
                ms(pct(&mut s, 0.95))
            );
        }
    }
}

/// The directory-walk fallback of file search must honour its budget: a tree far bigger than
/// the entry cap returns `Partial` within the time limit. (Spotlight itself is not measured.)
fn file_budget_report() {
    use ashdash_core::files::{Completeness, FileQuery, FileSearchScope, SearchBudget, SearchMode, WalkBackend};
    println!("\n=== file search budget (directory walk fallback, synthetic tree) ===");
    let root = std::env::temp_dir().join(format!("oa-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let t = Instant::now();
    let mut total = 0usize;
    for a in 0..20 {
        for b in 0..20 {
            let d = root.join(format!("d{a}")).join(format!("e{b}"));
            std::fs::create_dir_all(&d).unwrap();
            for f in 0..25 {
                std::fs::write(d.join(format!("file-{a}-{b}-{f}.txt")), b"x").unwrap();
                total += 1;
            }
        }
    }
    println!(
        "synthetic tree: {total} files (+440 dirs) in {:.0} ms (setup, not measured)",
        ms(t.elapsed())
    );
    let q = FileQuery::parse(SearchMode::Name, "file-19-19-24");
    let scope = |b: SearchBudget| FileSearchScope::new(vec![root.clone()], vec![]).with_budget(b);
    let now = std::time::SystemTime::now();
    let cases = [
        ("default budget (200k entries, 2 s)", SearchBudget::default()),
        (
            "walk_max_entries = 1000",
            SearchBudget {
                walk_max_entries: 1_000,
                ..SearchBudget::default()
            },
        ),
        (
            "walk_timeout = 5 ms",
            SearchBudget {
                walk_timeout: Duration::from_millis(5),
                ..SearchBudget::default()
            },
        ),
    ];
    for (label, budget) in cases {
        let sc = scope(budget);
        let backend = WalkBackend::from_budget(&sc.budget);
        let mut s = Vec::new();
        let mut last = None;
        for _ in 0..5 {
            let t = Instant::now();
            let o = ashdash_core::files::search_with_outcome(&backend, &sc, &q, 20, now).unwrap();
            s.push(t.elapsed());
            last = Some((o.paths.len(), o.completeness));
        }
        let (hits, c) = last.unwrap();
        println!(
            "walk {label}: median {:.1} ms max {:.1} ms, hits={hits}, {}",
            ms(pct(&mut s.clone(), 0.5)),
            ms(*s.iter().max().unwrap()),
            if c == Completeness::Partial {
                "Partial"
            } else {
                "Complete"
            }
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn main() {
    let counts: Vec<usize> = match std::env::var("ASHDASH_BENCH_APPS") {
        Ok(v) => v.split(',').filter_map(|x| x.trim().parse().ok()).collect(),
        Err(_) => DEFAULT_APP_COUNTS.to_vec(),
    };
    for n in counts {
        apps_report(n);
    }
    clipboard_report();
    file_budget_report();
    real_scan();
}
