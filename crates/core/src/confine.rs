//! Path confinement for files named by untrusted data (workflow icons, bundle icon names).
//!
//! SEC-17: a path that comes from a Script Filter, an `info.plist` or an app bundle must not be
//! able to point at an arbitrary file on disk. [`confined_file`] resolves a relative (or
//! already-inside) path below a root and rejects anything that could leave it.

use std::path::{Component, Path, PathBuf};

/// Resolve `candidate` to a regular file that lives inside `root`.
///
/// * a relative `candidate` is joined to `root`; an absolute one must already start with `root`
///   (or with its canonical form);
/// * `..` components, prefixes and empty paths are refused outright;
/// * no component below `root` may be a symlink (checked with `symlink_metadata`, so a link to a
///   file *inside* the root is refused too: no benign use needs one);
/// * the result must be a regular file whose canonical path is inside the canonical root.
///
/// Returns the joined path (not canonicalised) so callers keep a stable name for caching.
pub fn confined_file(root: &Path, candidate: &Path) -> Option<PathBuf> {
    confined(root, candidate, false)
}

/// Like [`confined_file`] but also refuses a `root` that is itself a symlink (use it when the
/// root is a directory that untrusted data could have replaced by a link).
pub fn confined_file_strict(root: &Path, candidate: &Path) -> Option<PathBuf> {
    confined(root, candidate, true)
}

fn confined(root: &Path, candidate: &Path, strict_root: bool) -> Option<PathBuf> {
    if strict_root && std::fs::symlink_metadata(root).ok()?.file_type().is_symlink() {
        return None;
    }
    let canon_root = root.canonicalize().ok()?;
    let rel: PathBuf = if candidate.is_absolute() {
        candidate
            .strip_prefix(root)
            .or_else(|_| candidate.strip_prefix(&canon_root))
            .ok()?
            .to_path_buf()
    } else {
        candidate.to_path_buf()
    };
    if rel.as_os_str().is_empty() {
        return None;
    }
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(part) => cur.push(part),
            Component::CurDir => continue,
            _ => return None, // `..`, root, prefix
        }
        if std::fs::symlink_metadata(&cur).ok()?.file_type().is_symlink() {
            return None;
        }
    }
    let meta = std::fs::symlink_metadata(&cur).ok()?;
    if !meta.is_file() {
        return None;
    }
    cur.canonicalize().ok()?.starts_with(&canon_root).then_some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-confine-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn accepts_files_inside_relative_and_absolute() {
        let d = tmp("ok");
        std::fs::create_dir_all(d.join("img")).unwrap();
        std::fs::write(d.join("img/a.png"), b"x").unwrap();
        assert_eq!(confined_file(&d, Path::new("img/a.png")), Some(d.join("img/a.png")));
        assert!(confined_file(&d, Path::new("./img/a.png")).is_some());
        assert!(confined_file(&d, &d.join("img/a.png")).is_some());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn rejects_escapes_missing_dirs_and_empty() {
        let d = tmp("bad");
        let outside = tmp("bad-outside");
        std::fs::write(outside.join("secret.png"), b"x").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/in.png"), b"x").unwrap();
        for p in [
            "../x.png",
            "sub/../sub/in.png",
            "",
            ".",
            "sub",
            "nope.png",
            "/etc/hosts",
        ] {
            assert_eq!(confined_file(&d, Path::new(p)), None, "{p:?}");
        }
        assert_eq!(confined_file(&d, &outside.join("secret.png")), None);
        let _ = std::fs::remove_dir_all(d);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn rejects_symlinks_file_dir_and_root_escape() {
        let d = tmp("sym");
        let outside = tmp("sym-outside");
        std::fs::write(outside.join("secret.png"), b"x").unwrap();
        std::fs::write(d.join("real.png"), b"x").unwrap();
        symlink(outside.join("secret.png"), d.join("link.png")).unwrap();
        symlink("real.png", d.join("inner-link.png")).unwrap();
        symlink(&outside, d.join("dirlink")).unwrap();
        assert!(confined_file(&d, Path::new("real.png")).is_some());
        assert_eq!(confined_file(&d, Path::new("link.png")), None, "link out");
        assert_eq!(confined_file(&d, Path::new("inner-link.png")), None, "link in");
        assert_eq!(confined_file(&d, Path::new("dirlink/secret.png")), None, "dir link");
        let _ = std::fs::remove_dir_all(d);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn strict_mode_refuses_a_symlinked_root() {
        let real = tmp("strict-real");
        std::fs::write(real.join("a.png"), b"x").unwrap();
        let link = real
            .parent()
            .unwrap()
            .join(format!("oa-confine-strictlink-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        symlink(&real, &link).unwrap();
        assert!(confined_file(&link, Path::new("a.png")).is_some());
        assert_eq!(confined_file_strict(&link, Path::new("a.png")), None);
        assert!(confined_file_strict(&real, Path::new("a.png")).is_some());
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_dir_all(real);
    }

    #[test]
    fn a_symlinked_root_itself_is_fine() {
        let real = tmp("root-real");
        std::fs::write(real.join("a.png"), b"x").unwrap();
        let link = real
            .parent()
            .unwrap()
            .join(format!("oa-confine-rootlink-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        symlink(&real, &link).unwrap();
        assert!(confined_file(&link, Path::new("a.png")).is_some());
        assert!(confined_file(&link, &link.join("a.png")).is_some());
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_dir_all(real);
    }
}
