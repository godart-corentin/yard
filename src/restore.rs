//! Application images only. No backup command, migration, data volume or database operation
//! is reachable from this module. Data backups are listed as metadata, never executed.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::error::{Result, YardError};
use crate::project::Project;
use crate::state::{BackupAttempt, ProjectState, Release};

const MANUAL: &str = "Data backups cannot be restored by Yard. Manually inspect the backup and its destination, stop application writers, verify a separate database restore plan, and perform and validate the data restore outside Yard before restarting the application. See README.md (Manual data recovery).";

fn timestamp() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

fn journal_path(project: &Project) -> PathBuf {
    project.state_path.with_extension("restore.jsonl")
}

fn journal(project: &Project, target: Option<&str>, result: &str, cause: &str) -> Result<()> {
    let path = journal_path(project);
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| YardError::Config("invalid state path".into()))?,
    )?;
    // Do not follow a planted symlink to another file; retain previous attempts.
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&path)?;
    writeln!(
        file,
        "{}",
        json!({"at_unix": timestamp().as_secs(), "target": target,
        "result": result, "cause": cause})
    )?;
    file.sync_all()?;
    Ok(())
}

pub fn log(project: &Project) -> Result<()> {
    let path = journal_path(project);
    match fs::read_to_string(&path) {
        Ok(contents) => print!("{contents}"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("No restore attempts recorded")
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub fn points(project: &Project) -> Result<()> {
    let contents = match fs::read_to_string(&project.state_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            println!("No restore points recorded for {}", project.name);
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let value: Value = serde_json::from_str(&contents).map_err(|error| {
        YardError::Config(format!("state file is unreadable, not absent: {error}"))
    })?;
    if !value.is_object() {
        return Err(YardError::Config(
            "state file is unreadable, not absent: expected an object".into(),
        ));
    }
    println!(
        "Restore points for {} (application releases only):",
        project.name
    );
    for key in ["previous", "current", "pending"] {
        if let Some(record) = value.get(key).filter(|record| !record.is_null()) {
            match serde_json::from_value::<Release>(record.clone()) {
                Ok(release) => println!(
                    "  release:{} {key} revision={} at={} age={}s result={} destination={} ",
                    release.tag,
                    release.revision,
                    release.deployed_at_unix,
                    timestamp()
                        .as_secs()
                        .saturating_sub(release.deployed_at_unix),
                    release.status,
                    release.tag
                ),
                Err(_) => println!("  {key}: unreadable release record (not absent)"),
            }
        }
    }
    for (key, label) in [
        ("last_backup", "backup:local"),
        ("last_offsite", "backup:offsite"),
    ] {
        match value.get(key).filter(|record| !record.is_null()) {
            Some(record) => match serde_json::from_value::<BackupAttempt>(record.clone()) {
                Ok(attempt) => println!(
                    "  {label} at={} age={}s result={} destination={} (data: manual recovery only)",
                    attempt.started_at_unix,
                    timestamp()
                        .as_secs()
                        .saturating_sub(attempt.started_at_unix),
                    attempt.result.label(),
                    attempt.destination.as_deref().unwrap_or("unknown")
                ),
                Err(_) => println!("  {label}: unreadable backup record (not absent)"),
            },
            None => println!("  {label}: no backup recorded"),
        }
    }
    Ok(())
}

pub fn run(project: &Project, target: Option<&str>, yes: bool) -> Result<()> {
    // A writable journal is required even for refusals; the start entry survives a crash.
    journal(project, target, "started", "requested")?;
    let result = attempt(project, target, yes);
    let cause = match &result {
        Ok(()) => "verified application activation".to_owned(),
        Err(YardError::Config(reason)) if reason.starts_with("activation failed (") => {
            "activation and application recovery failed; pending release retained".into()
        }
        Err(YardError::Config(reason)) if reason.starts_with("application activated but state") => {
            "application activated; state save failed; inspect runtime and snapshot".into()
        }
        Err(YardError::Config(reason)) => reason.clone(),
        Err(_) => "restore failed; inspect CLI error and pending release with yard status".into(),
    };
    journal(
        project,
        target,
        if result.is_ok() {
            "success"
        } else {
            "refused_or_failed"
        },
        &cause,
    ).map_err(|error| YardError::Config(format!(
        "restore outcome could not be journaled ({error}); operation result: {cause}; inspect yard status and the journal"
    )))?;
    result
}

fn attempt(project: &Project, target: Option<&str>, yes: bool) -> Result<()> {
    let target = target
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            YardError::Config(
                "name an application revision (yard restore <project> <revision> --yes)".into(),
            )
        })?;
    if target.starts_with("backup:") {
        return Err(YardError::Config(MANUAL.into()));
    }
    if !yes {
        return Err(YardError::Config(
            "confirmation required: rerun with --yes after checking the target".into(),
        ));
    }
    project.ensure_clean()?;
    project.check_tag_writable()?;
    let mut state = ProjectState::load(&project.state_path)?;
    let current = state
        .current
        .clone()
        .or_else(|| state.previous.clone())
        .ok_or_else(|| {
            YardError::Config(
                "no active release recorded; inspect yard status before restoring".into(),
            )
        })?;
    let releases: Vec<_> = [state.current.as_ref(), state.previous.as_ref()]
        .into_iter()
        .flatten()
        .collect();
    let available = releases
        .iter()
        .map(|release| format!("release:{}", release.tag))
        .collect::<Vec<_>>()
        .join(", ");
    let known = if let Some(tag) = target.strip_prefix("release:") {
        Some(
            releases
                .iter()
                .copied()
                .find(|item| item.tag == tag)
                .ok_or_else(|| {
                    YardError::Config(format!(
                        "no recorded release tagged {tag}; available: {available}"
                    ))
                })?,
        )
    } else {
        let matches: Vec<_> = releases
            .iter()
            .copied()
            .filter(|item| item.revision == target)
            .collect();
        if matches.len() > 1 && matches.iter().any(|item| item.tag != matches[0].tag) {
            return Err(YardError::Config("revision matches multiple recorded releases; select release:<tag> from yard restore-points".into()));
        }
        matches.first().copied()
    };
    let mut release = if let Some(known) = known {
        known.clone()
    } else {
        let revision = project
            .resolve_revision(target)
            .map_err(|error| match error {
                YardError::CommandFailed { .. } => {
                    YardError::Config(format!("unknown revision {target}; available: {available}"))
                }
                other => other,
            })?;
        [state.current.as_ref(), state.previous.as_ref()]
            .into_iter()
            .flatten()
            .find(|item| item.revision == revision)
            .cloned()
            .unwrap_or_else(|| Release::new(revision.clone(), Project::tag_for_revision(&revision)))
    };
    if release.services.is_empty() {
        release.services = project.release_services(&release.tag)?;
    }
    let mut current = current;
    if current.services.is_empty() {
        current.services = project.release_services(&current.tag)?;
    }
    // Recorded state cannot authorize Compose services. Check both activation and
    // failure-recovery paths before snapshots or runtime changes.
    validate_application_release(project, &release)?;
    validate_application_release(project, &current)?;
    if project.current_tag_from_env()?.as_deref() != Some(current.tag.as_str())
        && state.pending.is_none()
    {
        return Err(YardError::Config(
            "Compose tag differs from recorded active release; inspect yard status".into(),
        ));
    }
    for service in &release.services {
        if !project.image_ref_exists(&service.image)? {
            return Err(YardError::ImageMissing(service.image.clone()));
        }
    }
    // Both files are copied before the first write. No application data is read or written.
    let snapshot = snapshot_files(project)?;
    println!(
        "Application restore {}: {} -> {} (snapshot {})",
        project.name,
        current.tag,
        release.tag,
        snapshot.display()
    );
    let mut pending = release.clone();
    pending.status = "activating".into();
    state.pending = Some(pending);
    state.save(&project.state_path)?;
    let activation = (|| -> Result<()> {
        project.persist_tag(&release.tag)?;
        project.activate(&release)
    })();
    if let Err(error) = activation {
        eprintln!(
            "Application activation failed: {error}; attempting to reactivate {}",
            current.tag
        );
        let recovery = (|| -> Result<()> {
            project.persist_tag(&current.tag)?;
            project.activate(&current)?;
            state.pending = None;
            if state.current.is_none() {
                current.status = "active".into();
                state.current = Some(current.clone());
                state.previous = None;
            }
            state.save(&project.state_path)
        })();
        return match recovery {
            Ok(()) => Err(error),
            Err(restore_error) => Err(YardError::Config(format!(
                "activation failed ({error}); recovery failed ({restore_error}); pending marker retained; inspect yard status and snapshot {}",
                snapshot.display()))),
        };
    }
    current.status = "superseded".into();
    release.status = "active".into();
    if state.current.is_none() {
        state.previous = None;
    } else if current.revision != release.revision || current.tag != release.tag {
        state.previous = Some(current);
    }
    state.current = Some(release);
    state.pending = None;
    state.save(&project.state_path).map_err(|error| YardError::Config(format!(
        "application activated but state could not be saved ({error}); inspect runtime and snapshot {} before retrying",
        snapshot.display())))?;
    Ok(())
}

fn validate_application_release(project: &Project, release: &Release) -> Result<()> {
    let allowed = &project.config.compose.services;
    let migration = project.config.deployment.migration_service.as_deref();
    let names: Vec<_> = release
        .services
        .iter()
        .map(|service| service.name.as_str())
        .collect();
    if names.len() != allowed.len()
        || names
            .iter()
            .any(|name| migration == Some(*name) || !allowed.iter().any(|allowed| allowed == name))
        || allowed
            .iter()
            .any(|name| names.iter().filter(|recorded| **recorded == name).count() != 1)
    {
        return Err(YardError::Config(format!(
            "release:{} services [{}] do not match authorized application services [{}] (migration service: {}); refusing to activate data, migration or unconfigured services. {MANUAL}",
            release.tag,
            names.join(", "),
            allowed.join(", "),
            migration.unwrap_or("none")
        )));
    }
    Ok(())
}

fn snapshot_files(project: &Project) -> Result<PathBuf> {
    let parent = project
        .state_path
        .parent()
        .ok_or_else(|| YardError::Config("invalid state path".into()))?;
    fs::create_dir_all(parent)?;
    if fs::symlink_metadata(&project.state_path)?
        .file_type()
        .is_symlink()
    {
        return Err(YardError::Config(format!(
            "refusing symbolic link at {}",
            project.state_path.display()
        )));
    }
    let state = fs::read(&project.state_path)?;
    let env = fs::read(project.config.compose_env_path())?;
    for _ in 0..8 {
        let stem = format!("{}.restore-{}", project.name, timestamp().as_nanos());
        let path = parent.join(format!("{stem}.json"));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        file.write_all(&state)?;
        file.sync_all()?;
        let env_path = parent.join(format!("{stem}.env"));
        let mut env_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&env_path)?;
        env_file.write_all(&env)?;
        env_file.sync_all()?;
        fs::File::open(parent)?.sync_all()?;
        return Ok(path);
    }
    Err(YardError::Config(
        "could not allocate a unique restore snapshot".into(),
    ))
}
