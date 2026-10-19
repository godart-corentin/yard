use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Project names are file stems; one that starts with `-` could be taken for an option by
/// any tool it is later passed to, so the CLI never accepts it (`yard deploy -- -x` included).
fn project_name(value: &str) -> Result<String, String> {
    if value.is_empty() || value.starts_with('-') {
        return Err("a project name cannot be empty or start with '-'".into());
    }
    Ok(value.to_owned())
}

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
    /// Serve the read-only Web operations over a local Unix socket.
    #[command(hide = true)]
    ReadServer {
        #[arg(long)]
        socket: PathBuf,
    },
    /// Run Yard Web actions as jobs over a local Unix socket.
    #[command(hide = true)]
    OpsServer {
        #[arg(long)]
        socket: PathBuf,
    },
    /// List configured projects.
    List,

    /// Show Git, Yard and Compose state for a project, or summarize all projects.
    Status {
        #[arg(value_parser = project_name)]
        project: Option<String>,
    },

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
    Deploy {
        #[arg(value_parser = project_name)]
        project: String,
    },

    /// Restore an explicitly named application revision (never data).
    Rollback {
        #[arg(value_parser = project_name)]
        project: String,
        revision: Option<String>,
        #[arg(long)]
        yes: bool,
    },

    /// Restore an explicitly named application revision (never data).
    Restore {
        #[arg(value_parser = project_name)]
        project: String,
        revision: Option<String>,
        #[arg(long)]
        yes: bool,
    },

    /// List recorded application releases and backup attempts.
    RestorePoints {
        #[arg(value_parser = project_name)]
        project: String,
    },

    /// Show the append-only restore attempt journal.
    RestoreLog {
        #[arg(value_parser = project_name)]
        project: String,
    },

    /// Show application logs (following them by default).
    Logs {
        #[arg(value_parser = project_name)]
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
    Backup {
        #[arg(value_parser = project_name)]
        project: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("yard").chain(args.iter().copied()))
    }

    #[test]
    fn double_dash_ends_options_before_a_project_name() {
        let Command::Deploy { project } = parse(&["deploy", "--", "demo"]).unwrap().command else {
            panic!("not deploy");
        };
        assert_eq!(project, "demo");
        let Command::Restore {
            project,
            revision,
            yes,
        } = parse(&["restore", "--yes", "--", "demo", "abc123"])
            .unwrap()
            .command
        else {
            panic!("not restore");
        };
        assert_eq!(
            (project.as_str(), revision.as_deref(), yes),
            ("demo", Some("abc123"), true)
        );
        let Command::Logs {
            project,
            no_follow,
            tail,
            ..
        } = parse(&["logs", "--no-follow", "--tail", "5", "--", "demo"])
            .unwrap()
            .command
        else {
            panic!("not logs");
        };
        assert_eq!((project.as_str(), no_follow, tail), ("demo", true, 5));
    }

    #[test]
    fn a_project_name_starting_with_a_dash_is_rejected_even_after_double_dash() {
        for args in [
            &["deploy", "--", "-demo"][..],
            &["backup", "--", "--yes"],
            &["status", "--", "-x"],
            &["restore-points", "--", "-x"],
            &["restore", "--yes", "--", "-x", "abc"],
            &["logs", "--", "-x"],
            &["deploy", "-demo"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }
}
