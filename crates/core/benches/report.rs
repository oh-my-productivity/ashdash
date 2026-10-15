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

const N_APPS: usize = 5_000;
const QUERIES: [&str; 4] = ["saf", "vsc", "visual studio", "x"];

fn words() -> Vec<&'static str> {
    vec![
        "Visual", "Studio", "Code", "Safari", "Preview", "Terminal", "Photo", "Booth", "Final", "Cut", "Pro", "Logic",
        "Xcode", "Docker", "Desktop", "Slack", "Zoom", "Notes", "Music", "Maps", "Mail", "Calendar", "Reminders",
        "Keynote", "Pages", "Numbers", "Transmit", "Sketch", "Figma", "Postman", "Insomnia", "Tower", "Fork", "Kaleidoscope",
        "Xmind", "Audacity", "Handbrake", "Inkscape", "Blender", "Unity", "Hub", "Editor", "Helper", "Manager", "Pad",
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

fn make_apps(dir: &Path) {
    let plist = r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleName</key><string>X</string><key>CFBundleIdentifier</key><string>com.example.x</string><key>CFBundleIconFile</key><string>icon</string></dict></plist>"#;
    for (i, name) in synthetic_names(N_APPS).iter().enumerate() {
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
    let secs = t.split(':').try_fold(0.0, |acc, p| p.parse::<f64>().ok().map(|v| acc * 60.0 + v))?;
    Some((secs, rss))
}

fn main() {
    let root: PathBuf = std::env::temp_dir().join(format!("oa-report-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let apps_dir = root.join("apps");
    let t = Instant::now();
    make_apps(&apps_dir);
    println!("synthetic apps created: {N_APPS} in {:.0} ms (setup, not measured)", ms(t.elapsed()));

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
    println!("engine.query overall p95 {:.3} ms (apps={})", ms(pct(&mut all, 0.95)), e.app_count());

    // --- bare fuzzy loop
    let names = synthetic_names(N_APPS);
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
        println!("fuzzy::score loop over {N_APPS} names, {q:?}: p95 {:.3} ms", ms(pct(&mut s, 0.95)));
    }

    // --- idle CPU of the headless engine
    if let Some((c0, rss)) = cpu_time(std::process::id()) {
        std::thread::sleep(Duration::from_secs(5));
        let (c1, _) = cpu_time(std::process::id()).unwrap_or((c0, rss));
        println!("idle: {:.2} cpu-seconds over 5 s (ps resolution 10 ms), rss {} MB", c1 - c0, rss / 1024);
    } else {
        println!("idle CPU: n/a");
    }
    drop(e);

    // --- real application folders (counts only)
    let mut dirs = vec![PathBuf::from("/Applications"), PathBuf::from("/System/Applications")];
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(h).join("Applications"));
    }
    for _ in 0..1 {
        let t = Instant::now();
        let found = apps::scan(&dirs);
        println!("real scan of {:?}: {} apps in {:.1} ms", dirs, found.len(), ms(t.elapsed()));
    }
    let _ = std::fs::remove_dir_all(&root);
}
