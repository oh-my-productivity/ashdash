//! Synthetic application sets for benchmarks and latency tests (not used at runtime).

use std::path::Path;

const WORDS: [&str; 45] = [
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
];

/// `n` unique, deterministic, realistic-looking application names ("Studio Mail Pad 17").
pub fn synthetic_names(n: usize) -> Vec<String> {
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
                s.push_str(WORDS[(next() % WORDS.len() as u64) as usize]);
            }
            format!("{s} {i}")
        })
        .collect()
}

/// Create `n` fake bundles (plus "Safari" and "Visual Studio Code") directly inside `dir`.
/// With `plist`, each gets a tiny `Contents/Info.plist` so scans pay the parse cost too.
pub fn make_fake_apps(dir: &Path, n: usize, plist: bool) -> std::io::Result<()> {
    const INFO: &str = r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleName</key><string>X</string><key>CFBundleIdentifier</key><string>com.example.x</string><key>CFBundleIconFile</key><string>icon</string></dict></plist>"#;
    for name in synthetic_names(n)
        .into_iter()
        .chain(["Safari".to_string(), "Visual Studio Code".to_string()])
    {
        let app = dir.join(format!("{name}.app"));
        if plist {
            std::fs::create_dir_all(app.join("Contents"))?;
            std::fs::write(app.join("Contents/Info.plist"), INFO)?;
        } else {
            std::fs::create_dir_all(&app)?;
        }
    }
    Ok(())
}
