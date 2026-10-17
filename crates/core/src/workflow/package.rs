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
//!   old one, restoring it if the second step fails; when the staging folder and the workflows
//!   folder are on different volumes (`EXDEV`) the tree is copied and fsynced beside the
//!   destination first, so a half-written install is never visible.

use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};

use super::script::safe_key;
use super::{TrustSummary, Workflow};
use crate::config::Paths;

/// File extensions of the reference launcher workflow packages (the reference launcher all use the same zip format), lower
/// case, without the dot. The launcher, Finder opens, the CLI and the preferences window share
/// this list.
pub const WORKFLOW_PACKAGE_EXTENSIONS: [&str; 4] = [
    "alfredworkflow",
    "alfred3workflow",
    "alfred4workflow",
    "alfred5workflow",
];

/// Whether `path` has one of the [`WORKFLOW_PACKAGE_EXTENSIONS`] (case-insensitive).
pub fn is_workflow_package(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| WORKFLOW_PACKAGE_EXTENSIONS.iter().any(|w| e.eq_ignore_ascii_case(w)))
}

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
#[derive(Debug)]
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

/// Staging directories older than this are leftovers of a crashed run and are removed when the
/// next package is staged.
const STALE_STAGING: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Install a `.alfredworkflow` package (a zip archive) under `dest_root`, without asking.
///
/// For trusted callers (tests, scripts). Anything that shows the user a confirmation must use
/// [`stage_package`] and install the [`StagedPackage`] it returns, so that what was confirmed is
/// what is installed.
pub fn import_package(package: &Path, dest_root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_root)?;
    stage_in(package, dest_root, dest_root, None)?.install()
}

/// What a package declares about itself and what it can do, read without installing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagePreview {
    pub name: String,
    pub bundle_id: String,
    /// `createdby` from `info.plist` (free text chosen by the package's author: never verified).
    pub author: String,
    pub description: String,
    pub version: String,
    pub trust: TrustSummary,
    /// The installed workflow this package would replace (same destination folder), if any.
    pub replaces: Option<InstalledWorkflow>,
    /// Values saved for this workflow's identity by an earlier install exist and stay in force:
    /// the new package reads them as its own configuration (they may hold tokens or passwords).
    /// Only known when the package was staged with [`stage_package`] (which has the data folder).
    pub saved_settings: bool,
}

impl PackagePreview {
    pub fn replaces_installed(&self) -> bool {
        self.replaces.is_some()
    }
}

/// An installed workflow that a package would replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledWorkflow {
    pub name: String,
    pub version: String,
}

/// A package that has been extracted once into a private staging directory and described. The
/// bytes of the original file are never read again: [`StagedPackage::install`] moves exactly the
/// tree that [`StagedPackage::preview`] described into place, so the file can change, vanish or
/// be swapped after the user was asked without affecting what gets installed. Dropping it
/// without installing deletes the staging directory.
#[derive(Debug)]
pub struct StagedPackage {
    preview: PackagePreview,
    staging: Staging,
    dest: PathBuf,
    dest_root: PathBuf,
}

impl StagedPackage {
    /// What the user is asked about.
    pub fn preview(&self) -> &PackagePreview {
        &self.preview
    }

    /// The folder the workflow will be installed to.
    pub fn destination(&self) -> &Path {
        &self.dest
    }

    /// Move the staged tree into the workflows folder (replacing the installed version, which
    /// is restored if the swap fails).
    pub fn install(mut self) -> Result<PathBuf> {
        if !self.staging.path.is_dir() {
            bail!("the staged copy of the workflow is gone (it expired or was removed): open the file again");
        }
        install(&self.staging.path, &self.dest, &self.dest_root)?;
        self.staging.keep();
        Ok(self.dest.clone())
    }
}

/// Open a `.alfredworkflow` package once: extract it into a private (0700) staging directory
/// under the data folder, with the importer's limits, parse and describe it. Nothing is
/// installed and nothing runs. Install the result with [`StagedPackage::install`] after
/// confirming; drop it to cancel.
pub fn stage_package(paths: &Paths, package: &Path) -> Result<StagedPackage> {
    let root = paths.data.join(STAGING_DIR);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&root)
        .context("creating the import staging folder")?;
    sweep_stale(&root);
    let dest_root = paths.workflows_dir();
    std::fs::create_dir_all(&dest_root)?;
    sweep_interrupted_installs(&dest_root, STALE_SWAP);
    stage_in(package, &root, &dest_root, Some(paths))
}

/// Hidden folders beside an installed workflow while it is swapped: the complete new tree, and the
/// previous version moved aside. Both names carry the start time (seconds), the old one also the
/// installed folder's name so a crash between the two renames can be undone.
const COPY_PREFIX: &str = ".import-copy-";
const OLD_PREFIX: &str = ".import-old-";
/// A swap takes microseconds (the copy is made first); leftovers older than this are a crash's.
pub(super) const STALE_SWAP: std::time::Duration = std::time::Duration::from_secs(3600);

/// Name of the staging folder inside the data folder.
const STAGING_DIR: &str = "import-staging";

fn sweep_stale(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_STAGING);
        if old && e.file_name().to_string_lossy().starts_with(".import-") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Clean up what an interrupted install left in a workflows folder `root`: `.import-*` staging
/// folders older than a day, a half-made `.import-copy-*` tree, and an `.import-old-*` previous
/// version -- which is put back if its installed folder is missing (the crash fell between the
/// two renames) and removed otherwise. Anything younger than `stale_after` (a swap in another
/// process) is left alone. Only names this module writes are touched.
pub(super) fn sweep_interrupted_installs(root: &Path, stale_after: std::time::Duration) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let now = unix_secs();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(".import-") {
            continue;
        }
        let path = e.path();
        let started_before = |secs: &str| {
            secs.parse::<u64>()
                .is_ok_and(|s| now.saturating_sub(s) >= stale_after.as_secs())
        };
        if let Some(rest) = name.strip_prefix(OLD_PREFIX) {
            let mut parts = rest.splitn(5, '-');
            let (secs, dest) = (parts.next().unwrap_or(""), parts.nth(3).unwrap_or(""));
            if !started_before(secs) || dest.is_empty() || dest.contains('/') {
                continue;
            }
            if std::fs::symlink_metadata(root.join(dest)).is_err() {
                let _ = std::fs::rename(&path, root.join(dest));
            } else {
                let _ = std::fs::remove_dir_all(&path);
            }
        } else if let Some(rest) = name.strip_prefix(COPY_PREFIX) {
            if started_before(rest.split('-').next().unwrap_or("")) {
                let _ = std::fs::remove_dir_all(&path);
            }
        } else {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > STALE_STAGING);
            if old {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

fn stage_in(package: &Path, staging_root: &Path, dest_root: &Path, paths: Option<&Paths>) -> Result<StagedPackage> {
    let file = std::fs::File::open(package).with_context(|| format!("opening {}", package.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid workflow archive")?;
    let staging = Staging::create(staging_root)?; // dropped (and deleted) on every error path
    extract_limited(&mut zip, &staging.path)?;

    let mut wf = Workflow::load(&staging.path).context("archive has no readable info.plist")?;
    let base: &str = if wf.bundle_id.is_empty() {
        &wf.name
    } else {
        &wf.bundle_id
    };
    if base.is_empty() {
        bail!("workflow has neither a bundle id nor a name");
    }
    let dest_name = format!("user.workflow.{}", safe_key(base));
    let dest = dest_root.join(&dest_name);

    let text = |key: &str| -> String {
        super::model::read_info_plist(&staging.path)
            .ok()
            .and_then(|v| v.into_dictionary())
            .and_then(|d| d.get(key).and_then(|v| v.as_string().map(str::to_owned)))
            .unwrap_or_default()
    };
    let replaces = match std::fs::symlink_metadata(&dest) {
        Ok(_) => Some(match Workflow::load(&dest) {
            Ok(old) => InstalledWorkflow {
                name: old.name,
                version: old.version,
            },
            Err(_) => InstalledWorkflow {
                name: dest_name.clone(),
                version: String::new(),
            },
        }),
        Err(_) => None,
    };
    // The saved-values file is named after the installed folder for id-less workflows.
    wf.id = dest_name;
    let saved_settings = paths.is_some_and(|p| wf.user_values_file(p).is_file());
    let preview = PackagePreview {
        name: wf.name.clone(),
        bundle_id: wf.bundle_id.clone(),
        author: text("createdby"),
        description: text("description"),
        version: wf.version.clone(),
        trust: wf.trust_summary(),
        replaces,
        saved_settings,
    };
    Ok(StagedPackage {
        preview,
        staging,
        dest,
        dest_root: dest_root.to_path_buf(),
    })
}

/// Swap `new` into `dest`: the previous version moves aside first and is restored if the final
/// rename fails, so there is never a moment in which a failure loses the old version.
fn install(new: &Path, dest: &Path, scratch_parent: &Path) -> Result<()> {
    install_with(new, dest, scratch_parent, &|from, to| std::fs::rename(from, to))
}

/// `EXDEV`: source and destination are on different volumes (or one of the folders is a link to
/// another volume), so `rename` cannot move the tree.
fn is_cross_device(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::CrossesDevices || e.raw_os_error() == Some(18)
}

/// Copy a directory tree (files with their permission bits, directories, symlinks as links) and
/// fsync every file and directory. `to` must not exist; it is created 0700 first and gets the
/// source's mode last.
fn copy_tree_synced(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn walk(from: &Path, to: &Path) -> std::io::Result<()> {
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let (src, dst) = (e.path(), to.join(e.file_name()));
            let meta = std::fs::symlink_metadata(&src)?;
            if meta.file_type().is_symlink() {
                symlink(std::fs::read_link(&src)?, &dst)?;
            } else if meta.is_dir() {
                std::fs::DirBuilder::new().mode(0o700).create(&dst)?;
                walk(&src, &dst)?;
                std::fs::set_permissions(
                    &dst,
                    std::fs::Permissions::from_mode(meta.permissions().mode() & 0o7777),
                )?;
                std::fs::File::open(&dst)?.sync_all()?;
            } else if meta.is_file() {
                let mut input = std::fs::File::open(&src)?;
                let mut out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&dst)?;
                std::io::copy(&mut input, &mut out)?;
                out.set_permissions(std::fs::Permissions::from_mode(meta.permissions().mode() & 0o7777))?;
                out.sync_all()?;
            } // sockets, devices and the like never come out of the extractor
        }
        Ok(())
    }
    let meta = std::fs::metadata(from)?;
    std::fs::DirBuilder::new().mode(0o700).create(to)?;
    walk(from, to)?;
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(meta.permissions().mode() & 0o7777))?;
    std::fs::File::open(to)?.sync_all()?;
    if let Some(parent) = to.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Seconds since the epoch, as written into the names of swap leftovers.
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How the complete tree got beside the destination.
enum Ready {
    /// `new` was renamed to the ready path (same volume); give it back if the swap fails.
    Moved,
    /// `new` was copied across a volume boundary; `new` is removed after a successful install.
    Copied,
}

/// Put a complete copy of `new` at `ready`, a hidden folder beside the destination (hence on its
/// volume). The slow part of an install -- on a different volume the tree is copied, modes and
/// links kept, and fsynced -- happens here, **before** the installed version is touched.
fn make_ready(
    new: &Path,
    ready: &Path,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<Ready> {
    match rename(new, ready) {
        Ok(()) => Ok(Ready::Moved),
        Err(e) if is_cross_device(&e) => match copy_tree_synced(new, ready) {
            Ok(()) => Ok(Ready::Copied),
            Err(e) => {
                let _ = std::fs::remove_dir_all(ready);
                Err(e)
            }
        },
        Err(e) => Err(e),
    }
}

/// Install `new` as `dest`. The complete tree is first put beside `dest` ([`make_ready`]); only
/// then is the previous version moved aside and the tree renamed into place: two renames, so the
/// workflow is missing for microseconds rather than for the length of a cross-volume copy. If the
/// final rename fails the previous version is restored. `new` is removed after a successful copy
/// install and otherwise left (or put back) so the caller may retry.
fn install_with(
    new: &Path,
    dest: &Path,
    scratch_parent: &Path,
    rename: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    let n = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let secs = unix_secs();
    let ready = scratch_parent.join(format!("{COPY_PREFIX}{secs}-{}-{n}", std::process::id()));
    let how = make_ready(new, &ready, rename).context("installing the workflow")?;
    let give_back = |ready: &Path| match how {
        Ready::Moved => {
            let _ = rename(ready, new);
        }
        Ready::Copied => {
            let _ = std::fs::remove_dir_all(ready);
        }
    };
    let dest_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut last: Option<std::io::Error> = None;
    for attempt in 0..5 {
        let aside = scratch_parent.join(format!(
            "{OLD_PREFIX}{secs}-{}-{n}-{attempt}-{dest_name}",
            std::process::id()
        ));
        let moved = match rename(dest, &aside) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                give_back(&ready);
                return Err(e).context("replacing the installed workflow");
            }
        };
        match rename(&ready, dest) {
            Ok(()) => {
                if moved {
                    let _ = std::fs::remove_dir_all(&aside);
                }
                if matches!(how, Ready::Copied) {
                    let _ = std::fs::remove_dir_all(new);
                }
                return Ok(());
            }
            Err(e) => {
                if moved {
                    let _ = rename(&aside, dest); // restore the previous version
                }
                // another import of the same workflow won the race: try again
                if !matches!(
                    e.kind(),
                    std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::AlreadyExists
                ) {
                    give_back(&ready);
                    return Err(e).context("installing the workflow");
                }
                last = Some(e);
            }
        }
    }
    give_back(&ready);
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
    #[test]
    fn every_reference_package_extension_is_recognised() {
        use super::{WORKFLOW_PACKAGE_EXTENSIONS, is_workflow_package};
        use std::path::Path;
        assert_eq!(WORKFLOW_PACKAGE_EXTENSIONS.len(), 4);
        for e in WORKFLOW_PACKAGE_EXTENSIONS {
            assert!(is_workflow_package(Path::new(&format!("/x/a.{e}"))), "{e}");
            assert!(
                is_workflow_package(Path::new(&format!("a.{}", e.to_uppercase()))),
                "{e} upper"
            );
        }
        for no in [
            "a.alfred2workflow",
            "a.zip",
            "alfredworkflow",
            "a.alfredworkflow.txt",
            "",
            "a.",
        ] {
            assert!(!is_workflow_package(Path::new(no)), "{no:?}");
        }
    }

    use super::*;
    use crate::config::Paths;

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

    /// A staged-looking tree: nested directory, an executable script, a relative symlink.
    fn make_tree(dir: &Path) {
        use std::os::unix::fs::{PermissionsExt, symlink};
        std::fs::create_dir_all(dir.join("lib/deep")).unwrap();
        std::fs::write(dir.join("info.plist"), "plist").unwrap();
        std::fs::write(dir.join("lib/deep/run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(dir.join("lib/deep/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink("deep/run.sh", dir.join("lib/link.sh")).unwrap();
    }

    fn exdev() -> std::io::Error {
        std::io::Error::from_raw_os_error(18)
    }

    fn leftovers(root: &Path) -> Vec<String> {
        std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".import-"))
            .collect()
    }

    #[test]
    fn install_across_volumes_copies_next_to_the_destination() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("oa-exdev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (staging, root) = (base.join("staging/.import-x"), base.join("workflows"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        make_tree(&staging);
        let dest = root.join("user.workflow.t.x");
        // Simulated second volume: any rename out of the staging folder fails with EXDEV.
        let renames = std::cell::RefCell::new(Vec::<(PathBuf, PathBuf)>::new());
        let rename = |from: &Path, to: &Path| -> std::io::Result<()> {
            renames.borrow_mut().push((from.to_path_buf(), to.to_path_buf()));
            if from.starts_with(base.join("staging")) {
                return Err(exdev());
            }
            std::fs::rename(from, to)
        };
        install_with(&staging, &dest, &root, &rename).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("info.plist")).unwrap(), "plist");
        let run = dest.join("lib/deep/run.sh");
        assert_eq!(std::fs::read_to_string(&run).unwrap(), "#!/bin/sh\necho hi\n");
        assert_eq!(std::fs::metadata(&run).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(
            std::fs::read_link(dest.join("lib/link.sh")).unwrap(),
            PathBuf::from("deep/run.sh")
        );
        assert!(
            !staging.exists(),
            "the staged copy is removed after a successful install"
        );
        assert!(leftovers(&root).is_empty(), "{:?}", leftovers(&root));
        // the final rename into place came from the destination's own folder
        let last = renames.borrow().last().cloned().unwrap();
        assert_eq!(last.1, dest);
        assert_eq!(last.0.parent().unwrap(), root);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_failed_cross_volume_install_keeps_the_old_version_and_leaves_nothing_behind() {
        let base = std::env::temp_dir().join(format!("oa-exdev-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (staging, root) = (base.join("staging/.import-y"), base.join("workflows"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        make_tree(&staging);
        let dest = root.join("user.workflow.t.y");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("info.plist"), "OLD").unwrap();
        // staging -> EXDEV; the copy's final rename into place also fails (disk full, say)
        let rename = |from: &Path, to: &Path| -> std::io::Result<()> {
            if from.starts_with(base.join("staging")) {
                return Err(exdev());
            }
            if to == dest
                && from
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(".import-copy-"))
            {
                return Err(std::io::Error::other("simulated failure"));
            }
            std::fs::rename(from, to)
        };
        let err = install_with(&staging, &dest, &root, &rename).unwrap_err();
        assert!(format!("{err:#}").contains("simulated failure"), "{err:#}");
        assert_eq!(std::fs::read_to_string(dest.join("info.plist")).unwrap(), "OLD");
        assert!(leftovers(&root).is_empty(), "no half install: {:?}", leftovers(&root));
        assert!(staging.exists(), "the staged tree is untouched so the caller may retry");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cross_volume_install_replaces_an_installed_version() {
        let base = std::env::temp_dir().join(format!("oa-exdev-rep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (staging, root) = (base.join("staging/.import-z"), base.join("workflows"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        make_tree(&staging);
        let dest = root.join("user.workflow.t.z");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("info.plist"), "OLD").unwrap();
        std::fs::write(dest.join("old-only.txt"), "x").unwrap();
        let rename = |from: &Path, to: &Path| -> std::io::Result<()> {
            if from.starts_with(base.join("staging")) {
                return Err(exdev());
            }
            std::fs::rename(from, to)
        };
        install_with(&staging, &dest, &root, &rename).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("info.plist")).unwrap(), "plist");
        assert!(!dest.join("old-only.txt").exists());
        assert!(leftovers(&root).is_empty(), "{:?}", leftovers(&root));
        let _ = std::fs::remove_dir_all(&base);
    }

    fn minimal_workflow(dir: &Path, name: &str, bundle: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict>\
             <key>name</key><string>{name}</string><key>bundleid</key><string>{bundle}</string>\
             <key>objects</key><array/></dict></plist>"
        );
        std::fs::write(dir.join("info.plist"), xml).unwrap();
    }

    #[test]
    fn load_all_skips_dot_directories_so_a_swap_in_progress_is_not_a_second_workflow() {
        // Review item 9: `.import-old-*`, `.import-copy-*` and `.import-*` staging folders sit in
        // the workflows folder and hold a complete info.plist.
        let root = std::env::temp_dir().join(format!("oa-loadall-dots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        minimal_workflow(&root.join("user.workflow.a"), "A", "t.a");
        minimal_workflow(&root.join(".import-old-0-1-2-0-user.workflow.a"), "A", "t.a");
        minimal_workflow(&root.join(".import-copy-0-1-2"), "A", "t.a");
        minimal_workflow(&root.join(".import-1-2-abc"), "A", "t.a");
        minimal_workflow(&root.join(".hidden"), "H", "t.h");
        let all = crate::workflow::load_all(std::slice::from_ref(&root));
        assert_eq!(all.len(), 1, "{:?}", all.iter().map(|w| &w.id).collect::<Vec<_>>());
        assert_eq!(all[0].id, "user.workflow.a");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_old_version_stays_visible_until_the_new_copy_is_complete() {
        // Review item 9: on a cross-volume install the copy is the slow part. It happens first;
        // only then is the old version moved aside and the copy renamed into place.
        let base = std::env::temp_dir().join(format!("oa-exdev-vis-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (staging, root) = (base.join("staging/.import-v"), base.join("workflows"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        make_tree(&staging);
        let dest = root.join("user.workflow.t.v");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("info.plist"), "OLD").unwrap();
        let log = std::cell::RefCell::new(Vec::<String>::new());
        let rename = |from: &Path, to: &Path| -> std::io::Result<()> {
            let old_visible = std::fs::read_to_string(dest.join("info.plist")).ok();
            let what = if from == dest {
                format!("old->aside (copy complete: {})", to.parent() == Some(root.as_path()))
            } else if to == dest {
                format!(
                    "copy->dest (copy has info.plist: {}, old visible: {})",
                    from.join("info.plist").is_file(),
                    old_visible.is_some()
                )
            } else {
                format!("other (old visible: {})", old_visible.as_deref() == Some("OLD"))
            };
            log.borrow_mut().push(what);
            if from.starts_with(base.join("staging")) {
                return Err(exdev());
            }
            std::fs::rename(from, to)
        };
        install_with(&staging, &dest, &root, &rename).unwrap();
        let log = log.into_inner();
        assert_eq!(
            log,
            [
                "other (old visible: true)", // the staging rename that fails with EXDEV: the copy follows
                "old->aside (copy complete: true)",
                "copy->dest (copy has info.plist: true, old visible: false)",
            ],
            "{log:#?}"
        );
        // Between "old->aside" and "copy->dest" nothing slow happens: no copy is made there.
        assert_eq!(std::fs::read_to_string(dest.join("info.plist")).unwrap(), "plist");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn interrupted_installs_are_swept_and_a_lost_workflow_is_put_back() {
        let root = std::env::temp_dir().join(format!("oa-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let now = unix_secs();
        let make = |name: &str, body: &str| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("info.plist"), body).unwrap();
        };
        // crash between the two renames: the old version is the only copy -> put back
        make(".import-old-1000-1-1-0-user.workflow.lost", "LOST");
        // successful swap whose cleanup failed: the installed folder exists -> remove the old one
        make("user.workflow.kept", "NEW");
        make(".import-old-1000-1-2-0-user.workflow.kept", "OLD");
        // a half-made copy
        make(".import-copy-1000-1-3", "HALF");
        // a swap in another process, started just now: untouched
        make(&format!(".import-old-{now}-9-9-0-user.workflow.live"), "LIVE");
        make(&format!(".import-copy-{now}-9-8"), "LIVE");
        // something that is not ours, and an old-looking name with junk in it
        make(".DS_Store.d", "X");
        make(".import-old-notanumber-1-1-0-user.workflow.z", "Z");
        make(".import-old-1000-1-1-0-", "NODEST");

        sweep_interrupted_installs(&root, STALE_SWAP);

        let read = |n: &str| std::fs::read_to_string(root.join(n).join("info.plist")).ok();
        assert_eq!(read("user.workflow.lost").as_deref(), Some("LOST"));
        assert_eq!(read("user.workflow.kept").as_deref(), Some("NEW"));
        assert!(!root.join(".import-old-1000-1-1-0-user.workflow.lost").exists());
        assert!(!root.join(".import-old-1000-1-2-0-user.workflow.kept").exists());
        assert!(!root.join(".import-copy-1000-1-3").exists());
        assert!(
            root.join(format!(".import-old-{now}-9-9-0-user.workflow.live"))
                .exists()
        );
        assert!(root.join(format!(".import-copy-{now}-9-8")).exists());
        assert!(root.join(".DS_Store.d").exists());
        assert!(root.join(".import-old-notanumber-1-1-0-user.workflow.z").exists());
        assert!(root.join(".import-old-1000-1-1-0-").exists());
        // and a reload brings the restored workflow back
        let ids: Vec<_> = crate::workflow::load_all(std::slice::from_ref(&root))
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert!(ids.is_empty(), "the test plists are not workflows: {ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn staging_a_package_sweeps_the_workflows_folder() {
        let root = std::env::temp_dir().join(format!("oa-sweep-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let paths = Paths::under(&root);
        let wfs = paths.workflows_dir();
        std::fs::create_dir_all(wfs.join(".import-copy-1000-1-3")).unwrap();
        let pkg = root.join("sample.alfredworkflow");
        sample_package(&pkg);
        let _staged = stage_package(&paths, &pkg).unwrap();
        assert!(!wfs.join(".import-copy-1000-1-3").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_same_volume_install_gives_the_staged_tree_back() {
        let base = std::env::temp_dir().join(format!("oa-giveback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (staging, root) = (base.join("staging/.import-g"), base.join("workflows"));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        make_tree(&staging);
        let dest = root.join("user.workflow.t.g");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("info.plist"), "OLD").unwrap();
        let rename = |from: &Path, to: &Path| -> std::io::Result<()> {
            if to == dest
                && from
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(COPY_PREFIX))
            {
                return Err(std::io::Error::other("simulated failure"));
            }
            std::fs::rename(from, to)
        };
        install_with(&staging, &dest, &root, &rename).unwrap_err();
        assert_eq!(std::fs::read_to_string(dest.join("info.plist")).unwrap(), "OLD");
        assert!(staging.join("info.plist").is_file(), "the caller may retry");
        assert!(leftovers(&root).is_empty(), "{:?}", leftovers(&root));
        let _ = std::fs::remove_dir_all(&base);
    }

    fn sample_package(path: &Path) {
        use std::io::Write;
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>name</key><string>Preview Sample</string>
<key>bundleid</key><string>test.preview.sample</string>
<key>createdby</key><string>Ann Author</string>
<key>description</key><string>Does things</string>
<key>objects</key><array>
<dict><key>uid</key><string>A</string><key>type</key><string>alfred.workflow.action.script</string>
<key>config</key><dict><key>type</key><integer>0</integer><key>script</key><string>echo hi</string></dict></dict>
<dict><key>uid</key><string>B</string><key>type</key><string>alfred.workflow.action.openurl</string>
<key>config</key><dict><key>url</key><string>https://example.com/{query}</string></dict></dict>
</array></dict></plist>"#;
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        z.start_file("info.plist", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(plist.as_bytes()).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn staging_describes_a_package_without_installing_it() {
        let root = std::env::temp_dir().join(format!("oa-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let pkg = root.join("sample.alfredworkflow");
        sample_package(&pkg);
        let paths = Paths::under(&root);

        let staged = stage_package(&paths, &pkg).unwrap();
        let p = staged.preview();
        assert_eq!(p.name, "Preview Sample");
        assert_eq!(p.bundle_id, "test.preview.sample");
        assert_eq!(p.author, "Ann Author");
        assert_eq!(p.description, "Does things");
        assert_eq!(p.trust.scripts, 1);
        assert_eq!(p.trust.open_url_schemes, ["https"]);
        assert!(p.replaces.is_none() && !p.saved_settings);
        assert!(!paths.workflows_dir().join("user.workflow.test.preview.sample").exists());

        // dropping it deletes the staged copy
        let staging_root = paths.data.join(STAGING_DIR);
        assert_eq!(std::fs::read_dir(&staging_root).unwrap().count(), 1);
        drop(staged);
        assert_eq!(std::fs::read_dir(&staging_root).unwrap().count(), 0);

        assert!(stage_package(&paths, &root.join("missing.alfredworkflow")).is_err());
        std::fs::write(root.join("junk.alfredworkflow"), b"not a zip").unwrap();
        assert!(stage_package(&paths, &root.join("junk.alfredworkflow")).is_err());
        assert_eq!(
            std::fs::read_dir(&staging_root).unwrap().count(),
            0,
            "failed attempts leave nothing behind"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A valid `info.plist` with one extra key holding `depth` nested arrays.
    fn deep_info_plist(depth: usize) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>name</key><string>Deep</string><key>bundleid</key><string>test.deep</string><key>objects</key><array/><key>x</key>{}{}</dict></plist>"#,
            "<array>".repeat(depth),
            "</array>".repeat(depth)
        )
    }

    fn deep_package(path: &Path, depth: usize) {
        use std::io::Write;
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("info.plist", options).unwrap();
        z.write_all(deep_info_plist(depth).as_bytes()).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn a_package_with_a_deeply_nested_info_plist_is_refused_not_a_stack_overflow() {
        let root = std::env::temp_dir().join(format!("oa-deep-pkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let pkg = root.join("deep.alfredworkflow");
        deep_package(&pkg, 120_000);
        assert!(std::fs::metadata(&pkg).unwrap().len() < 10_000, "a small download");
        let paths = Paths::under(&root);
        // Test threads have a 2 MiB stack, like the Finder-open and drop paths.
        let err = match stage_package(&paths, &pkg) {
            Ok(_) => panic!("a package with a 120 000-level info.plist was staged"),
            Err(e) => format!("{e:#}"),
        };
        assert!(err.contains("nested"), "{err}");
        let staging_root = paths.data.join(STAGING_DIR);
        assert_eq!(
            std::fs::read_dir(&staging_root).unwrap().count(),
            0,
            "nothing is left behind"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn load_all_skips_a_deep_info_plist_and_keeps_the_rest() {
        let root = std::env::temp_dir().join(format!("oa-deep-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (bad, good) = (root.join("bad"), root.join("good"));
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(bad.join("info.plist"), deep_info_plist(120_000)).unwrap();
        std::fs::write(
            good.join("info.plist"),
            deep_info_plist(2).replace("<key>x</key><array><array></array></array>", ""),
        )
        .unwrap();
        let all = crate::workflow::load_all(std::slice::from_ref(&root));
        assert_eq!(all.len(), 1, "{:?}", all.iter().map(|w| &w.name).collect::<Vec<_>>());
        assert_eq!(all[0].name, "Deep");
        assert!(Workflow::load(&bad).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
