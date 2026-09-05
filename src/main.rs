mod cli;
mod command;
mod config;
mod deploy;
mod envfile;
mod error;
mod health;
mod project;
mod rollback;
mod state;
mod status;

use std::env;
use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::error::Result;
use crate::project::Project;

const DEFAULT_PROJECTS_DIR: &str = "/etc/yard/projects";
const DEFAULT_STATE_DIR: &str = "/var/lib/yard";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("yard=info")),
        )
        .with_target(false)
        .without_time()
        .init();

    if let Err(error) = run() {
        eprintln!("yard: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let projects_dir = cli.projects_dir.unwrap_or_else(|| {
        env::var_os("YARD_PROJECTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_PROJECTS_DIR))
    });
    let state_dir = cli.state_dir.unwrap_or_else(|| {
        env::var_os("YARD_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR))
    });

    match cli.command {
        Command::List => {
            for name in Project::list(&projects_dir)? {
                println!("{name}");
            }
        }
        Command::Status { project } => {
            let project = Project::load(&project, &projects_dir, &state_dir)?;
            status::run(&project)?;
        }
        Command::Deploy { project } => {
            let project = Project::load(&project, &projects_dir, &state_dir)?;
            deploy::run(&project)?;
        }
        Command::Rollback { project, revision } => {
            let project = Project::load(&project, &projects_dir, &state_dir)?;
            rollback::run(&project, revision.as_deref())?;
        }
        Command::Logs {
            project,
            tail,
            no_follow,
        } => {
            let project = Project::load(&project, &projects_dir, &state_dir)?;
            project.compose_logs(tail, !no_follow)?;
        }
        Command::Backup { project } => {
            let project = Project::load(&project, &projects_dir, &state_dir)?;
            if project.config.backup.is_none() {
                return Err(crate::error::YardError::Config(format!(
                    "{} has no backup command configured",
                    project.name
                )));
            }
            project.run_backup()?;
        }
    }

    Ok(())
}
