//! Application discovery and icon extraction.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    pub name: String,
    pub path: PathBuf,
}

/// Scan directories for `.app` bundles up to three levels deep. Bundle
/// contents are never descended into.
pub fn scan(dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let mut it = WalkDir::new(dir).max_depth(3).follow_links(false).into_iter();
        while let Some(entry) = it.next() {
            let Ok(entry) = entry else { continue };
            let p = entry.path();
            // `is_dir` follows symlinks: Safari lives behind one on recent macOS.
            if p.extension().is_some_and(|e| e == "app") && p.is_dir() {
                it.skip_current_dir();
                if is_background_only(p) {
                    continue;
                }
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if seen.insert(p.to_path_buf()) {
                        out.push(AppEntry {
                            name: stem.to_string(),
                            path: p.to_path_buf(),
                        });
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Faceless helpers (System Events, ...) are not launchable by users.
fn is_background_only(app: &Path) -> bool {
    plist::Value::from_file(app.join("Contents/Info.plist"))
        .ok()
        .and_then(|v| v.as_dictionary()?.get("LSBackgroundOnly")?.as_boolean())
        .unwrap_or(false)
}

/// Locate the `.icns` file of an app bundle from its Info.plist.
fn icns_path(app: &Path) -> Option<PathBuf> {
    let resources = app.join("Contents/Resources");
    let info = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    let name = info
        .as_dictionary()?
        .get("CFBundleIconFile")?
        .as_string()?
        .to_string();
    let mut p = resources.join(&name);
    if p.extension().is_none() {
        p.set_extension("icns");
    }
    p.exists().then_some(p)
}

/// Convert the app's icon to a small PNG (cached) using the system `sips`
/// tool. Returns `None` for apps whose icon lives only in an asset catalog.
pub fn app_icon_png(app: &Path, cache_dir: &Path) -> Option<PathBuf> {
    let mut h = DefaultHasher::new();
    app.hash(&mut h);
    let out = cache_dir.join(format!("{:016x}.png", h.finish()));
    if out.exists() {
        return Some(out);
    }
    let icns = icns_path(app)?;
    std::fs::create_dir_all(cache_dir).ok()?;
    let status = Command::new("sips")
        .args(["-s", "format", "png", "-Z", "96"])
        .arg(&icns)
        .arg("--out")
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    (status.success() && out.exists()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_fake_apps_without_descending() {
        let root = std::env::temp_dir().join(format!("oa-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Foo.app/Contents/Nested.app")).unwrap();
        std::fs::create_dir_all(root.join("Utilities/Bar.app")).unwrap();
        let apps = scan(&[root.clone()]);
        let names: Vec<_> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Bar", "Foo"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
