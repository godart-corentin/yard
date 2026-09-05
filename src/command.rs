use std::path::Path;
use std::process::{Command, Stdio};

use tracing::debug;

use crate::error::{Result, YardError};

pub fn checked(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> Result<String> {
    debug!(program, ?args, cwd = ?cwd, "running command");

    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (key, value) in env {
        command.env(key, value);
    }

    let output = command.output()?;
    if !output.status.success() {
        return Err(YardError::CommandFailed {
            program: format_command(program, args),
            status: output.status.code().unwrap_or(1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn inherit(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> Result<()> {
    debug!(program, ?args, cwd = ?cwd, "running interactive command");

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (key, value) in env {
        command.env(key, value);
    }

    let status = command.status()?;
    if !status.success() {
        return Err(YardError::CommandFailed {
            program: format_command(program, args),
            status: status.code().unwrap_or(1),
            stderr: String::new(),
        });
    }

    Ok(())
}

fn format_command(program: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(program.to_owned());
    parts.extend(args.iter().cloned());
    parts.join(" ")
}
