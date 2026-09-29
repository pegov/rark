use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ignore::WalkBuilder;
use rusqlite::{Connection, OptionalExtension, params};
use similar::TextDiff;

const MAX_CONTENT: u64 = 1 * 1024 * 1024;

#[derive(Parser, Debug)]
#[command(about = "Checkpoint and diff project files")]
struct Cli {
    /// Project directory to checkpoint (defaults to the current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show previous diffs for a project
    History {
        /// Project directory (defaults to the current directory)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Maximum number of runs to show
        #[arg(
            long,
            value_name = "N",
            default_value_t = 5,
            value_parser = clap::value_parser!(i64).range(1..)
        )]
        limit: i64,
    },
    /// Print the latest saved diff as a message for an LLM agent
    Btw {
        /// Project directory (defaults to the current directory)
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

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

fn render_change(old: &Snapshot, new: &Snapshot, c: &Change) -> String {
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

fn database_path() -> Result<PathBuf> {
    let base = if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        PathBuf::from(dir)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share")
    } else if let Some(dir) = std::env::var_os("APPDATA") {
        PathBuf::from(dir)
    } else {
        bail!("set XDG_DATA_HOME, HOME, or APPDATA to locate the history database");
    };
    let dir = base.join("rark");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir.join("history.sqlite3"))
}

fn project_root(path: &Path) -> Result<PathBuf> {
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

fn open_history() -> Result<Connection> {
    let db = database_path()?;
    let conn = Connection::open(&db).with_context(|| format!("open {}", db.display()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY,
            root TEXT NOT NULL UNIQUE
        );
        CREATE TABLE IF NOT EXISTS runs (
            id INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL REFERENCES projects(id),
            recorded_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS runs_by_project ON runs(project_id, id);
        CREATE TABLE IF NOT EXISTS changes (
            run_id INTEGER NOT NULL REFERENCES runs(id),
            path TEXT NOT NULL,
            kind TEXT NOT NULL,
            diff TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS changes_by_run ON changes(run_id);",
    )?;
    Ok(conn)
}

fn save_run(
    conn: &mut Connection,
    root: &Path,
    changes: &[Change],
    before: &Snapshot,
    after: &Snapshot,
) -> Result<()> {
    let root = root.to_str().context("project path is not UTF-8")?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let tx = conn.transaction()?;
    tx.execute("INSERT OR IGNORE INTO projects(root) VALUES (?1)", [root])?;
    let project_id: i64 =
        tx.query_row("SELECT id FROM projects WHERE root = ?1", [root], |row| {
            row.get(0)
        })?;
    tx.execute(
        "INSERT INTO runs(project_id, recorded_at) VALUES (?1, ?2)",
        params![project_id, now],
    )?;
    let run_id = tx.last_insert_rowid();
    for change in changes {
        let (kind, path) = match change {
            Change::Added(path) => ("added", path),
            Change::Modified(path) => ("modified", path),
            Change::Deleted(path) => ("deleted", path),
        };
        let path = path.to_str().context("changed path is not UTF-8")?;
        tx.execute(
            "INSERT INTO changes(run_id, path, kind, diff) VALUES (?1, ?2, ?3, ?4)",
            params![run_id, path, kind, render_change(before, after, change)],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn show_history(conn: &Connection, root: &Path, limit: i64) -> Result<()> {
    let root = root.to_str().context("project path is not UTF-8")?;
    let mut runs = conn.prepare(
        "SELECT runs.id, runs.recorded_at FROM runs
         JOIN projects ON projects.id = runs.project_id
         WHERE projects.root = ?1 ORDER BY runs.id DESC LIMIT ?2",
    )?;
    let mut rows = runs.query(params![root, limit])?;
    let mut found = false;
    while let Some(row) = rows.next()? {
        found = true;
        let run_id: i64 = row.get(0)?;
        let recorded_at: i64 = row.get(1)?;
        println!("Run {run_id} (Unix timestamp {recorded_at}):");
        let mut changes =
            conn.prepare("SELECT path, kind, diff FROM changes WHERE run_id = ?1 ORDER BY rowid")?;
        let mut entries = changes.query([run_id])?;
        while let Some(entry) = entries.next()? {
            let path: String = entry.get(0)?;
            let kind: String = entry.get(1)?;
            let diff: String = entry.get(2)?;
            println!("{kind}: {path}");
            print!("{diff}");
        }
    }
    if !found {
        println!("No history for {root}.");
    }
    Ok(())
}

fn latest_btw(conn: &Connection, root: &Path) -> Result<String> {
    let root = root.to_str().context("project path is not UTF-8")?;
    let run_id: i64 = conn
        .query_row(
            "SELECT runs.id FROM runs
             JOIN projects ON projects.id = runs.project_id
             WHERE projects.root = ?1 ORDER BY runs.id DESC LIMIT 1",
            [root],
            |row| row.get(0),
        )
        .optional()?
        .with_context(|| format!("No saved runs for {root}."))?;

    let mut message = String::from("btw, i changed this:\n<diff>\n");
    let mut changes =
        conn.prepare("SELECT path, kind, diff FROM changes WHERE run_id = ?1 ORDER BY rowid")?;
    let mut entries = changes.query([run_id])?;
    while let Some(entry) = entries.next()? {
        let path: String = entry.get(0)?;
        let kind: String = entry.get(1)?;
        let diff: String = entry.get(2)?;
        message.push_str(&format!("{kind}: {path}\n{diff}"));
        if !diff.ends_with('\n') {
            message.push('\n');
        }
    }
    message.push_str("</diff>\n");
    Ok(message)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::History { path, limit }) => {
            let root = project_root(&path)?;
            return show_history(&open_history()?, &root, limit);
        }
        Some(Command::Btw { path }) => {
            let root = project_root(&path)?;
            let message = latest_btw(&open_history()?, &root)?;
            io::stdout().write_all(message.as_bytes())?;
            return Ok(());
        }
        None => {}
    }
    let root = project_root(&cli.path)?;

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

    let mut conn = open_history()?;
    save_run(&mut conn, &root, &changes, &before, &after)?;
    println!("{} change(s):\n", changes.len());
    for c in &changes {
        println!("{}", c.path().display());
        print!("{}", render_change(&before, &after, c));
    }

    Ok(())
}
