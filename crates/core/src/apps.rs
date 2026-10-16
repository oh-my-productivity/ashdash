//! Application discovery and icon extraction.
//!
//! Discovery itself lives in [`crate::index`] (persistent, watched index); [`scan`] is the plain
//! one-shot walk. Icons are converted lazily with the system `sips` tool into a bounded cache.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::fuzzy::Candidate;
use crate::index::icons;

pub use crate::index::icons::{Limiter, MAX_CACHED_ICONS, sips_limiter};

#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    pub name: String,
    pub path: PathBuf,
    /// `name` normalized once, for [`crate::fuzzy::Query::score`].
    pub candidate: Candidate,
}

impl AppEntry {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> AppEntry {
        let name = name.into();
        AppEntry {
            candidate: Candidate::new(&name),
            name,
            path: path.into(),
        }
    }
}

/// Scan directories for `.app` bundles up to three levels deep. Bundle
/// contents are never descended into. Background-only apps are left out.
pub fn scan(dirs: &[PathBuf]) -> Vec<AppEntry> {
    crate::index::scan_entries(dirs)
}

/// Locate the `.icns` file of an app bundle from its Info.plist.
fn icns_path(app: &Path) -> Option<PathBuf> {
    let info = plist::Value::from_file(app.join("Contents/Info.plist")).ok()?;
    let name = info.as_dictionary()?.get("CFBundleIconFile")?.as_string()?.to_string();
    // SEC-17: `CFBundleIconFile` is a plain file name inside Resources; anything with a
    // separator, `..` or an absolute path (and any symlink) is refused.
    // The bundle is the confinement root and `Contents/Resources/<name>` the relative path, so
    // `Contents` and `Resources` themselves are checked: a `Resources` that is a symlink to
    // some other folder must not make that folder's images resolvable.
    let mut file = PathBuf::from(&name);
    if file.extension().is_none() {
        file.set_extension("icns");
    }
    if name.contains(['/', '\\', '\0']) {
        return None;
    }
    crate::confine::confined_file(app, &Path::new("Contents/Resources").join(file))
}

/// Convert the app's icon to a small PNG (cached) using the system `sips`
/// tool. Returns `None` for apps whose icon lives only in an asset catalog.
///
/// Safe to call from many threads: at most [`sips_limiter`]`().max()` conversions run at once
/// (the rest wait), and the cache directory is kept at [`MAX_CACHED_ICONS`] files, least
/// recently used first. A cache hit counts as a use.
pub fn app_icon_png(app: &Path, cache_dir: &Path) -> Option<PathBuf> {
    let mut h = DefaultHasher::new();
    app.hash(&mut h);
    let out = cache_dir.join(format!("{:016x}.png", h.finish()));
    if out.exists() {
        icons::touch(&out);
        return Some(out);
    }
    let icns = icns_path(app)?;
    std::fs::create_dir_all(cache_dir).ok()?;
    let _slot = sips_limiter().acquire();
    // another caller may have produced it while we waited for a slot
    if out.exists() {
        return Some(out);
    }
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let part = out.with_extension(format!(
        "{}-{}.part",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let status = Command::new(crate::actions::system_program(std::ffi::OsStr::new("sips")))
        .args(["-s", "format", "png", "-Z", "96"])
        .arg(&icns)
        .arg("--out")
        .arg(&part)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok();
    let ok = status.is_some_and(|s| s.success()) && std::fs::rename(&part, &out).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&part);
        return None;
    }
    icons::prune_dir(cache_dir, MAX_CACHED_ICONS);
    Some(out)
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
        let apps = scan(std::slice::from_ref(&root));
        let names: Vec<_> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Bar", "Foo"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn icon_conversion_is_cached_and_cache_is_bounded() {
        // Needs a bundle with a classic .icns and the system `sips`; skip cleanly otherwise.
        let Some(app) = std::fs::read_dir("/System/Applications")
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "app") && icns_path(p).is_some())
        else {
            return;
        };
        let cache = std::env::temp_dir().join(format!("oa-icon-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let Some(png) = app_icon_png(&app, &cache) else {
            let _ = std::fs::remove_dir_all(&cache);
            return;
        };
        assert!(png.exists());
        assert_eq!(
            app_icon_png(&app, &cache),
            Some(png.clone()),
            "second call is a cache hit"
        );
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1, "no .part leftovers");
        let _ = std::fs::remove_dir_all(&cache);
    }

    fn fake_app(root: &Path, icon_file: &str) -> PathBuf {
        let app = root.join("Evil.app");
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        let mut d = plist::Dictionary::new();
        d.insert("CFBundleIconFile".into(), icon_file.into());
        plist::Value::Dictionary(d)
            .to_file_xml(app.join("Contents/Info.plist"))
            .unwrap();
        app
    }

    #[test]
    fn icon_file_name_is_confined_to_resources() {
        // SEC-17
        let root = std::env::temp_dir().join(format!("oa-apps-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("secret.icns"), b"x").unwrap();
        let secret_abs = root.join("secret.icns").display().to_string();
        for bad in [
            "../../../secret.icns",
            "../secret",
            secret_abs.as_str(),
            "sub/x.icns",
            "a\\b",
        ] {
            let app = fake_app(&root, bad);
            assert_eq!(icns_path(&app), None, "{bad}");
            let _ = std::fs::remove_dir_all(&app);
        }
        // a symlink inside Resources is refused as well
        let app = fake_app(&root, "icon");
        std::os::unix::fs::symlink(root.join("secret.icns"), app.join("Contents/Resources/icon.icns")).unwrap();
        assert_eq!(icns_path(&app), None);
        std::fs::remove_file(app.join("Contents/Resources/icon.icns")).unwrap();
        // ...while a normal icon, with or without extension, resolves
        std::fs::write(app.join("Contents/Resources/icon.icns"), b"x").unwrap();
        assert_eq!(icns_path(&app), Some(app.join("Contents/Resources/icon.icns")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_symlinked_resources_folder_does_not_expose_other_images() {
        let root = std::env::temp_dir().join(format!("oa-apps-resym-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("photo.icns"), b"x").unwrap();
        let app = fake_app(&root, "photo");
        std::fs::remove_dir_all(app.join("Contents/Resources")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, app.join("Contents/Resources")).unwrap();
        assert_eq!(icns_path(&app), None, "Resources is a link to another folder");
        let _ = std::fs::remove_dir_all(&root);
    }
}
