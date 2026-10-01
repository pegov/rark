mod cli;
mod clipboard;
mod diff;
mod history;
mod project;
mod snapshot;

use std::io::{self, BufRead, Write};

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::clipboard::copy_to_clipboard;
use crate::diff::{diff, render_change};
use crate::history::{latest_btw, open_history, save_run, show_history};
use crate::project::project_root;
use crate::snapshot::Snapshot;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::History { path, limit }) => {
            let root = project_root(&path)?;
            return show_history(&open_history()?, &root, limit);
        }
        Some(Command::Btw { path, copy }) => {
            let root = project_root(&path)?;
            let message = latest_btw(&open_history()?, &root)?;
            io::stdout().write_all(message.as_bytes())?;
            if copy {
                copy_to_clipboard(&message)?;
            }
            return Ok(());
        }
        None => {}
    }
    let root = project_root(&cli.path)?;

    println!("Taking checkpoint of {}", root.display());
    let before = Snapshot::capture(&root)?;
    println!("done ({} files).", before.files.len());

    println!("Press Enter to diff against the current state...");
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
