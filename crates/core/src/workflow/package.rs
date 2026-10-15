//! Installing `.alfredworkflow` packages.
//!
//! A package is untrusted input until it has been extracted and validated, so the importer
//! extracts by hand (no `ZipArchive::extract`) into a private, uniquely named staging directory,
//! under hard limits, and only then swaps the finished tree into place:
//!
//! * at most [`MAX_ENTRIES`] entries, [`MAX_TOTAL_BYTES`] of uncompressed data, and a ceiling on
//!   the overall compression ratio; sizes are enforced again while copying (declared sizes can lie);
//! * names go through `enclosed_name()`; an archive with an unsafe name, a duplicate entry or a
//!   symlink that leaves the tree is rejected as a whole;
//! * symlinks are created last, so no file is ever written through one;
//! * the staging directory is removed on every failure path (drop guard);
//! * installation renames the old version aside, renames the new tree in and only then deletes the
//!   old one, restoring it if the second step fails.

use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};

use super::Workflow;
use super::script::safe_key;

/// Most entries (files, directories, links) an archive may contain.
pub const MAX_ENTRIES: usize = 2_000;
/// Most uncompressed bytes an archive may expand to.
pub const MAX_TOTAL_BYTES: u64 = 100 * 1024 * 1024;
/// Above this expanded size the archive as a whole must compress less than [`MAX_RATIO`]:1.
const RATIO_FLOOR_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RATIO: u64 = 200;
const MAX_LINK_TARGET: usize = 4096;

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A staging directory that deletes itself unless [`Staging::keep`] is called.
struct Staging {
    path: PathBuf,
    armed: bool,
}

impl Staging {
    /// Create a fresh, private (0700), uniquely named directory next to the destination.
    /// `create_dir` fails if the name exists (even as a pre-planted symlink), so it is retried.
    fn create(parent: &Path) -> Result<Staging> {
        for _ in 0..32 {
            let n = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let path = parent.join(format!(".import-{}-{n}-{nanos:x}", std::process::id()));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Staging { path, armed: true }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e).context("creating staging directory"),
            }
        }
        bail!("cannot create a unique staging directory")
    }

    fn keep(&mut self) {
        self.armed = false;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Install a `.alfredworkflow` package (a zip archive) under `dest_root`.
pub fn import_package(package: &Path, dest_root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_root)?;
    let file = std::fs::File::open(package).with_context(|| format!("opening {}", package.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid workflow archive")?;
    let mut staging = Staging::create(dest_root)?;
    extract_limited(&mut zip, &staging.path)?;

    let wf = Workflow::load(&staging.path).context("archive has no readable info.plist")?;
    let base: &str = if wf.bundle_id.is_empty() {
        &wf.name
    } else {
        &wf.bundle_id
    };
    if base.is_empty() {
        bail!("workflow has neither a bundle id nor a name");
    }
    let dest = dest_root.join(format!("user.workflow.{}", safe_key(base)));
    install(&staging.path, &dest, dest_root)?;
    staging.keep();
    Ok(dest)
}

/// Swap `new` into `dest`: the previous version moves aside first and is restored if the final
/// rename fails, so there is never a moment in which a failure loses the old version.
fn install(new: &Path, dest: &Path, scratch_parent: &Path) -> Result<()> {
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..5 {
        let n = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
        let aside = scratch_parent.join(format!(".import-old-{}-{n}-{attempt}", std::process::id()));
        let moved = match std::fs::rename(dest, &aside) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e).context("replacing the installed workflow"),
        };
        match std::fs::rename(new, dest) {
            Ok(()) => {
                if moved {
                    let _ = std::fs::remove_dir_all(&aside);
                }
                return Ok(());
            }
            Err(e) => {
                if moved {
                    let _ = std::fs::rename(&aside, dest); // restore the previous version
                }
                // another import of the same workflow won the race: try again
                if !matches!(
                    e.kind(),
                    std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::AlreadyExists
                ) {
                    return Err(e).context("installing the workflow");
                }
                last = Some(e);
            }
        }
    }
    Err(anyhow!(
        "could not install the workflow: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn extract_limited<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, root: &Path) -> Result<()> {
    if zip.len() > MAX_ENTRIES {
        bail!("archive has {} entries (limit {MAX_ENTRIES})", zip.len());
    }
    // Pass 1: metadata only. Declared sizes and the overall ratio.
    let (mut declared, mut packed) = (0u64, 0u64);
    for i in 0..zip.len() {
        let e = zip.by_index_raw(i)?;
        declared = declared.saturating_add(e.size());
        packed = packed.saturating_add(e.compressed_size());
        if declared > MAX_TOTAL_BYTES {
            bail!("archive expands to more than {} MiB", MAX_TOTAL_BYTES >> 20);
        }
    }
    if declared > RATIO_FLOOR_BYTES && declared / packed.max(1) > MAX_RATIO {
        bail!("archive compression ratio is implausible (decompression bomb?)");
    }

    // Pass 2: write regular files and directories; collect links for the end.
    let mut budget = MAX_TOTAL_BYTES;
    let mut links: Vec<(PathBuf, String)> = Vec::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i)?;
        let rel: PathBuf = e
            .enclosed_name()
            .ok_or_else(|| anyhow!("unsafe entry name {:?}", e.name()))?
            .components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect();
        if rel.as_os_str().is_empty() {
            if e.is_dir() {
                continue; // "./": the root itself
            }
            bail!("unsafe entry name {:?}", e.name());
        }
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            bail!("unsafe entry name {:?}", e.name());
        }
        let out = root.join(&rel);
        let mode = e.unix_mode().unwrap_or(if e.is_dir() { 0o755 } else { 0o644 });
        if e.is_dir() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&out)?;
        } else if mode & 0o170000 == 0o120000 {
            let mut target = Vec::new();
            (&mut e).take(MAX_LINK_TARGET as u64 + 1).read_to_end(&mut target)?;
            if target.len() > MAX_LINK_TARGET {
                bail!("symlink target too long in {:?}", e.name());
            }
            let target = String::from_utf8(target).map_err(|_| anyhow!("symlink target is not UTF-8"))?;
            links.push((rel, target));
        } else if mode & 0o170000 != 0 && mode & 0o170000 != 0o100000 {
            bail!("special file {:?} in archive", e.name());
        } else {
            if let Some(parent) = out.parent() {
                std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
            }
            // owner keeps read/write, executable bits survive, group/other never write, no setuid
            let perm = (mode & 0o755) | 0o600;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(perm)
                .open(&out)
                .with_context(|| format!("creating {}", rel.display()))?;
            let written = copy_limited(&mut e, &mut f, budget)?;
            budget -= written;
        }
    }

    // Pass 3: links, now that no later entry can be written through them.
    for (rel, target) in &links {
        check_link(rel, target)?;
        let at = root.join(rel);
        if let Some(parent) = at.parent() {
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
        }
        std::os::unix::fs::symlink(target, &at).with_context(|| format!("creating link {}", rel.display()))?;
    }
    // Chains of individually harmless links can still resolve outside: verify the final result.
    let canon = root.canonicalize()?;
    for (rel, _) in &links {
        if let Ok(resolved) = root.join(rel).canonicalize()
            && !resolved.starts_with(&canon)
        {
            bail!("symlink {} resolves outside the workflow", rel.display());
        }
    }
    Ok(())
}

/// Copy at most `budget` bytes; more than that is an error (the declared size lied).
fn copy_limited(r: &mut impl Read, w: &mut impl Write, budget: u64) -> Result<u64> {
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        total += n as u64;
        if total > budget {
            bail!("archive expands to more than {} MiB", MAX_TOTAL_BYTES >> 20);
        }
        w.write_all(&buf[..n])?;
    }
}

/// A link must be relative and, resolved lexically from the directory that holds it, stay inside
/// the tree.
fn check_link(rel: &Path, target: &str) -> Result<()> {
    if target.is_empty() || target.contains('\0') || target.starts_with('/') {
        bail!("symlink {} has an unsafe target {target:?}", rel.display());
    }
    let mut depth = rel.components().count() as i64 - 1; // directories above the link inside the tree
    for c in Path::new(target).components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::ParentDir => depth -= 1,
            Component::CurDir => {}
            _ => bail!("symlink {} has an unsafe target {target:?}", rel.display()),
        }
        if depth < 0 {
            bail!("symlink {} points outside the workflow", rel.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_must_stay_inside() {
        let ok = |rel: &str, t: &str| check_link(Path::new(rel), t).is_ok();
        assert!(ok("a", "b"));
        assert!(ok("d/a", "../b"));
        assert!(ok("d/a", "./x/../y"));
        assert!(!ok("a", "../b"));
        assert!(!ok("d/a", "../../b"));
        assert!(!ok("a", "/etc/hosts"));
        assert!(!ok("a", ""));
        assert!(!ok("a", "up/../.."));
    }

    #[test]
    fn staging_dirs_are_unique_private_and_cleaned() {
        let root = std::env::temp_dir().join(format!("oa-staging-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let a = Staging::create(&root).unwrap();
        let b = Staging::create(&root).unwrap();
        assert_ne!(a.path, b.path);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&a.path).unwrap().permissions().mode() & 0o777, 0o700);
        let (pa, pb) = (a.path.clone(), b.path.clone());
        drop(a);
        assert!(!pa.exists() && pb.exists());
        let mut b = b;
        b.keep();
        drop(b);
        assert!(pb.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
