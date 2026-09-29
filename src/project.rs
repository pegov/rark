use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub(crate) fn project_root(path: &Path) -> Result<PathBuf> {
    let path =
        fs::canonicalize(path).with_context(|| format!("canonicalize {}", path.display()))?;
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    Ok(path
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(&path)
        .to_path_buf())
}
