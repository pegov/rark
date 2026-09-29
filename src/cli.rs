use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(about = "Checkpoint and diff project files")]
pub(crate) struct Cli {
    /// Project directory to checkpoint (defaults to the current directory)
    #[arg(default_value = ".")]
    pub(crate) path: PathBuf,

    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
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
