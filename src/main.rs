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

struct Snapshot {
    files: HashMap<PathBuf, FileMeta>,
    contents: HashMap<PathBuf, Vec<u8>>,
}

impl Snapshot {
    fn capture(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root).with_context(|| format!("canonicalize {:?}", root))?;

        let mut files = HashMap::new();
        let mut contents = HashMap::new();

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
            let Some(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_file() {
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

            if let Some(bytes) = read_text(path, size) {
                contents.insert(rel.clone(), bytes);
            }
            files.insert(rel, FileMeta { size, mtime });
        }

        Ok(Snapshot { files, contents })
    }
}

fn read_text(path: &Path, size: u64) -> Option<Vec<u8>> {
    if size > MAX_CONTENT {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    Some(bytes)
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
            Some(om) if om != m => changes.push(Change::Modified(p.clone())),
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
    let (old_c, new_c): (Option<&Vec<u8>>, Option<&Vec<u8>>) = match c {
        Change::Added(_) => (None, new.contents.get(path)),
        Change::Deleted(_) => (old.contents.get(path), None),
        Change::Modified(_) => (old.contents.get(path), new.contents.get(path)),
    };

    let unreadable = match c {
        Change::Added(_) => new_c.is_none(),
        Change::Deleted(_) => old_c.is_none(),
        Change::Modified(_) => old_c.is_none() || new_c.is_none(),
    };

    if unreadable {
        let label = match c {
            Change::Added(_) => "added",
            Change::Deleted(_) => "deleted",
            Change::Modified(_) => "modified",
        };
        println!("[{} binary or too large — content not diffed]", label);
        println!();
        return;
    }

    let old_s = old_c
        .map(|v| String::from_utf8_lossy(v))
        .unwrap_or_default();
    let new_s = new_c
        .map(|v| String::from_utf8_lossy(v))
        .unwrap_or_default();

    if old_s == new_s {
        println!("[metadata changed, content identical]");
        println!();
        return;
    }

    let (old_label, new_label) = match c {
        Change::Added(_) => ("/dev/null".to_string(), format!("b/{}", path.display())),
        Change::Deleted(_) => (format!("a/{}", path.display()), "/dev/null".to_string()),
        Change::Modified(_) => (
            format!("a/{}", path.display()),
            format!("b/{}", path.display()),
        ),
    };

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
