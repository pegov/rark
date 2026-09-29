use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use ignore::WalkBuilder;

const MAX_CONTENT: u64 = 1 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileMeta {
    size: u64,
    mtime: SystemTime,
}

pub(crate) struct FileEntry {
    pub(crate) meta: FileMeta,
    pub(crate) content: Option<Vec<u8>>,
}

pub(crate) struct Snapshot {
    pub(crate) files: HashMap<PathBuf, FileEntry>,
}

impl Snapshot {
    pub(crate) fn capture(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root).with_context(|| format!("canonicalize {:?}", root))?;

        let mut files = HashMap::new();

        let walker = WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .follow_links(false)
            .build();

        for entry in walker {
            let entry = entry?;
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(&root)
                .with_context(|| format!("strip prefix for {:?}", path))?
                .to_path_buf();

            let meta = entry.metadata()?;
            let size = meta.len();
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

            let meta = FileMeta { size, mtime };
            files.insert(
                rel,
                FileEntry {
                    meta,
                    content: read_text(path, size),
                },
            );
        }

        Ok(Snapshot { files })
    }
}

fn read_text(path: &Path, size: u64) -> Option<Vec<u8>> {
    if size > MAX_CONTENT {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    (!bytes.contains(&0)).then_some(bytes)
}
