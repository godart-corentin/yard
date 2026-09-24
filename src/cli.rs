use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "yard", version, about = "Homelab deployment CLI")]
pub struct Cli {
    /// Override the project manifest directory.
    #[arg(long, global = true)]
    pub projects_dir: Option<PathBuf>,

    /// Override the deployment state directory.
    #[arg(long, global = true)]
    pub state_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List configured projects.
    List,

    /// Show Git, Yard and Compose state for a project, or summarize all projects.
    Status { project: Option<String> },

    /// Collect and display host metrics; refresh the Web snapshot.
    Host,

    /// Inventory Yard images, optionally pruning unused revisions.
    Images {
        /// Simulate removal of candidates (requires --yes to actually remove).
        #[arg(long)]
        prune: bool,
        /// Confirm the requested prune operation.
        #[arg(long, requires = "prune")]
        yes: bool,
    },

    /// Deploy the configured branch for a project.
    Deploy { project: String },

    /// Restore an explicitly named application revision (never data).
    Rollback {
        project: String,
        revision: Option<String>,
        #[arg(long)]
        yes: bool,
    },

    /// Restore an explicitly named application revision (never data).
    Restore {
        project: String,
        revision: Option<String>,
        #[arg(long)]
        yes: bool,
    },

    /// List recorded application releases and backup attempts.
    RestorePoints { project: String },

    /// Show the append-only restore attempt journal.
    RestoreLog { project: String },

    /// Show application logs (following them by default).
    Logs {
        project: String,

        /// Number of existing log lines to show first.
        #[arg(long, default_value_t = 200)]
        tail: u32,

        /// Show logs for only this configured Compose service.
        #[arg(long)]
        service: Option<String>,

        /// Show logs since this duration or timestamp (Docker Compose format).
        #[arg(long)]
        since: Option<String>,

        /// Follow new log lines (already the default; conflicts with --no-follow).
        #[arg(long, conflicts_with = "no_follow")]
        follow: bool,

        /// Print logs and exit instead of following them.
        #[arg(long)]
        no_follow: bool,
    },

    /// Run the configured project backup command.
    Backup { project: String },
}
