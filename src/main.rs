use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use similar::TextDiff;

const MAX_CONTENT: u64 = 1 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileMeta {
    size: u64,
    mtime: SystemTime,
}

struct FileEntry {
    meta: FileMeta,
    content: Option<Vec<u8>>,
}

struct Snapshot {
    files: HashMap<PathBuf, FileEntry>,
}

impl Snapshot {
    fn capture(root: &Path) -> Result<Self> {
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

#[derive(Debug)]
enum Change {
    Added(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
}

impl Change {
    fn path(&self) -> &Path {
        match self {
            Change::Added(p) | Change::Modified(p) | Change::Deleted(p) => p,
        }
    }
}

fn diff(old: &Snapshot, new: &Snapshot) -> Vec<Change> {
    let mut changes = Vec::new();

    for (p, m) in &new.files {
        match old.files.get(p) {
            None => changes.push(Change::Added(p.clone())),
            Some(om) if om.meta != m.meta => changes.push(Change::Modified(p.clone())),
            _ => {}
        }
    }
    for p in old.files.keys() {
        if !new.files.contains_key(p) {
            changes.push(Change::Deleted(p.clone()));
        }
    }
    changes.sort_by(|a, b| a.path().cmp(b.path()));
    changes
}

fn render_change(old: &Snapshot, new: &Snapshot, c: &Change) {
    let path = c.path();
    let old_c = old.files.get(path).and_then(|e| e.content.as_deref());
    let new_c = new.files.get(path).and_then(|e| e.content.as_deref());
    let a = format!("a/{}", path.display());
    let b = format!("b/{}", path.display());
    let (old_c, new_c, label, old_label, new_label) = match c {
        Change::Added(_) => (Some(&[][..]), new_c, "added", "/dev/null", b.as_str()),
        Change::Deleted(_) => (old_c, Some(&[][..]), "deleted", a.as_str(), "/dev/null"),
        Change::Modified(_) => (old_c, new_c, "modified", a.as_str(), b.as_str()),
    };

    let (Some(old_c), Some(new_c)) = (old_c, new_c) else {
        println!("[{label} binary or too large — content not diffed]");
        println!();
        return;
    };

    let old_s = String::from_utf8_lossy(old_c);
    let new_s = String::from_utf8_lossy(new_c);

    if old_s == new_s {
        println!("[metadata changed, content identical]");
        println!();
        return;
    }

    let text_diff = TextDiff::from_lines(old_s.as_ref(), new_s.as_ref());
    print!(
        "{}",
        text_diff
            .unified_diff()
            .context_radius(3)
            .header(&old_label, &new_label)
    );
    println!();
}

fn main() -> Result<()> {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = PathBuf::from(root);

    println!("Taking checkpoint of {}", root.display());
    io::stdout().flush()?;
    let before = Snapshot::capture(&root)?;
    println!("done ({} files).", before.files.len());

    print!("Press Enter to diff against the current state... ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;

    let after = Snapshot::capture(&root)?;
    let changes = diff(&before, &after);

    if changes.is_empty() {
        println!("No changes.");
        return Ok(());
    }

    println!("{} change(s):\n", changes.len());
    for c in &changes {
        render_change(&before, &after, c);
    }

    Ok(())
}
