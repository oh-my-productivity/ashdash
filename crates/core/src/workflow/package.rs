//! Installing `.alfredworkflow` packages.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::Workflow;

/// Install a `.alfredworkflow` package (a zip archive) under `dest_root`.
pub fn import_package(package: &Path, dest_root: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_root)?;
    let tmp = dest_root.join(format!(".import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let file = std::fs::File::open(package).with_context(|| format!("opening {}", package.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("not a valid workflow archive")?;
    // `extract` rejects entries that would escape the target directory.
    zip.extract(&tmp).context("extracting workflow")?;
    let wf = Workflow::load(&tmp).context("archive has no readable info.plist")?;
    let base: String = if wf.bundle_id.is_empty() {
        wf.name.clone()
    } else {
        wf.bundle_id.clone()
    };
    let safe: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        bail!("workflow has neither a bundle id nor a name");
    }
    let dest = dest_root.join(format!("user.workflow.{safe}"));
    if dest.exists() {
        std::fs::remove_dir_all(&dest)?;
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}
