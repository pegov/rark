use std::path::{Path, PathBuf};

use similar::TextDiff;

use crate::snapshot::Snapshot;

#[derive(Debug)]
pub(crate) enum Change {
    Added(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
}

impl Change {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Change::Added(p) | Change::Modified(p) | Change::Deleted(p) => p,
        }
    }
}

pub(crate) fn diff(old: &Snapshot, new: &Snapshot) -> Vec<Change> {
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

pub(crate) fn render_change(old: &Snapshot, new: &Snapshot, c: &Change) -> String {
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
        return format!("[{label} binary or too large — content not diffed]\n\n");
    };

    let old_s = String::from_utf8_lossy(old_c);
    let new_s = String::from_utf8_lossy(new_c);

    if old_s == new_s {
        return "[metadata changed, content identical]\n\n".to_string();
    }

    let text_diff = TextDiff::from_lines(old_s.as_ref(), new_s.as_ref());
    format!(
        "{}\n",
        text_diff
            .unified_diff()
            .context_radius(3)
            .header(&old_label, &new_label)
    )
}
