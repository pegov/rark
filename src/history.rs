use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::diff::{Change, render_change};
use crate::snapshot::Snapshot;

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

pub(crate) fn open_history() -> Result<Connection> {
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

pub(crate) fn save_run(
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

pub(crate) fn show_history(conn: &Connection, root: &Path, limit: i64) -> Result<()> {
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

pub(crate) fn latest_btw(conn: &Connection, root: &Path) -> Result<String> {
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

    let mut diff_text = String::new();
    let mut changes =
        conn.prepare("SELECT path, kind, diff FROM changes WHERE run_id = ?1 ORDER BY rowid")?;
    let mut entries = changes.query([run_id])?;
    while let Some(entry) = entries.next()? {
        let path: String = entry.get(0)?;
        let kind: String = entry.get(1)?;
        let diff: String = entry.get(2)?;
        diff_text.push_str(&format!("{kind}: {path}\n{diff}"));
        if !diff.ends_with('\n') {
            diff_text.push('\n');
        }
    }
    let template = if let Some(home) = std::env::var_os("HOME") {
        let template_path = PathBuf::from(home).join(".config/rark/btw.md");
        read_btw_template(&template_path)?
    } else {
        DEFAULT_BTW_TEMPLATE.to_string()
    };
    Ok(template.replace("{{DIFF}}", &diff_text))
}

const DEFAULT_BTW_TEMPLATE: &str = "btw, i changed this:\n<diff>\n{{DIFF}}</diff>\n";

fn read_btw_template(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(template) => Ok(template),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(DEFAULT_BTW_TEMPLATE.to_string())
        }
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
}
