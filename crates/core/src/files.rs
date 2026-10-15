//! File search through Spotlight (`mdfind`) and path navigation.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fuzzy;

pub fn tilde(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(rest);
    }
    PathBuf::from(s)
}

fn is_noise(p: &Path, home: &Path) -> bool {
    if p.starts_with(home.join("Library")) || p.to_string_lossy().contains("/pkg/mod/") {
        return true;
    }
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s.starts_with('.') && s.len() > 1 || s == "node_modules" || s == "target"
    })
}

/// Spotlight search by file name inside the home folder.
pub fn search(query: &str, limit: usize) -> Vec<PathBuf> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let home = dirs::home_dir().unwrap_or_default();
    let out = Command::new("mdfind")
        .arg("-onlyin")
        .arg(&home)
        .arg("-name")
        .arg(q)
        .output();
    let Ok(out) = out else { return Vec::new() };
    let mut scored: Vec<(f64, PathBuf)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(PathBuf::from)
        .filter(|p| !is_noise(p, &home))
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().to_string();
            // shallower paths win ties
            let s = fuzzy::score(q, &name)? - p.components().count() as f64 * 0.1;
            Some((s, p))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(_, p)| p).collect()
}

/// List a directory for path-navigation queries such as `~/Doc` or `/usr/l`.
pub fn browse(query: &str, limit: usize) -> Vec<PathBuf> {
    let full = expand_tilde(query);
    let (dir, prefix) = if query.ends_with('/') {
        (full.clone(), String::new())
    } else {
        (
            full.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/")),
            full.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
        )
    };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let show_hidden = prefix.starts_with('.');
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            (show_hidden || !n.starts_with('.')) && n.starts_with(&prefix.to_lowercase())
        })
        .collect();
    v.sort_by_key(|p| p.file_name().map(|s| s.to_string_lossy().to_lowercase()));
    v.truncate(limit);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browse_lists_matching_children() {
        let root = std::env::temp_dir().join(format!("oa-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        std::fs::create_dir_all(root.join("alps")).unwrap();
        std::fs::create_dir_all(root.join("beta")).unwrap();
        let q = format!("{}/al", root.display());
        let names: Vec<_> = browse(&q, 10)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["alpha", "alps"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tilde_roundtrip() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~/x"), home.join("x"));
        assert_eq!(tilde(&home.join("x")), "~/x");
    }
}
