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

    /// Show Git, Yard and Compose state for a project.
    Status { project: String },

    /// Deploy the configured branch for a project.
    Deploy { project: String },

    /// Roll back to the previous deployment or to an already-built revision.
    Rollback {
        project: String,
        revision: Option<String>,
    },

    /// Follow application logs.
    Logs {
        project: String,

        /// Number of existing log lines to show first.
        #[arg(long, default_value_t = 200)]
        tail: u32,

        /// Print logs and exit instead of following them.
        #[arg(long)]
        no_follow: bool,
    },

    /// Run the configured project backup command.
    Backup { project: String },
}
