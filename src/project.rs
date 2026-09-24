use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::command;
use crate::config::ProjectConfig;
use crate::envfile;
use crate::error::{Result, YardError};
use crate::state::{BackupAttempt, BackupResult, ProjectState, Release, ReleaseService};

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub config: ProjectConfig,
    pub state_path: PathBuf,
}

impl Project {
    pub fn load(name: &str, projects_dir: &Path, state_dir: &Path) -> Result<Self> {
        if name == "host" {
            return Err(YardError::Config(
                "project name 'host' is reserved for the host snapshot".into(),
            ));
        }
        let path = projects_dir.join(format!("{name}.toml"));
        if !path.is_file() {
            return Err(YardError::ProjectNotFound(name.to_owned()));
        }
        let config = ProjectConfig::load(&path)?;
        Ok(Self {
            name: name.to_owned(),
            config,
            state_path: state_dir.join(format!("{name}.json")),
        })
    }

    pub fn list(projects_dir: &Path) -> Result<Vec<String>> {
        if !projects_dir.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(projects_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                if stem == "host" {
                    continue;
                }
                names.push(stem.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn ensure_clean(&self) -> Result<()> {
        let output = self.git_checked(&["status", "--porcelain", "--untracked-files=no"])?;
        if !output.trim().is_empty() {
            return Err(YardError::Config(format!(
                "{} has tracked local changes; refusing to deploy",
                self.config.repo.display()
            )));
        }
        Ok(())
    }

    pub fn head_revision(&self) -> Result<String> {
        self.git_checked(&["rev-parse", "HEAD"])
    }

    pub fn current_branch(&self) -> Result<String> {
        self.git_checked(&["branch", "--show-current"])
    }

    pub fn resolve_revision(&self, revision: &str) -> Result<String> {
        self.git_checked(&["rev-parse", &format!("{revision}^{{commit}}")])
    }

    pub fn switch_branch(&self) -> Result<()> {
        self.git_checked(&["switch", &self.config.branch])?;
        Ok(())
    }

    pub fn update_branch(&self) -> Result<()> {
        self.git_checked(&["fetch", &self.config.remote, &self.config.branch])?;
        self.git_checked(&[
            "merge",
            "--ff-only",
            &format!("{}/{}", self.config.remote, self.config.branch),
        ])?;
        Ok(())
    }

    pub fn tag_for_revision(revision: &str) -> String {
        revision.chars().take(12).collect()
    }

    pub fn image_ref_exists(&self, image: &str) -> Result<bool> {
        let args = vec!["image".to_owned(), "inspect".to_owned(), image.to_owned()];
        match command::checked("docker", &args, None, &[]) {
            Ok(_) => Ok(true),
            Err(YardError::CommandFailed { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn release_services(&self, tag: &str) -> Result<Vec<ReleaseService>> {
        let mut args = self.compose_args();
        args.extend([
            "config".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
        ]);
        let output = command::checked(
            "docker",
            &args,
            Some(&self.config.compose.directory),
            &self.tag_env(tag),
        )?;
        let config: serde_json::Value = serde_json::from_str(&output)?;
        self.config
            .compose
            .services
            .iter()
            .map(|name| {
                let image = config["services"][name]["image"].as_str().ok_or_else(|| {
                    YardError::Config(format!(
                        "Compose service {name} requires an image reference"
                    ))
                })?;
                if !image.ends_with(&format!(":{tag}")) {
                    return Err(YardError::Config(format!(
                        "Compose service {name} image {image} must be tagged with {tag}"
                    )));
                }
                Ok(ReleaseService {
                    name: name.clone(),
                    image: image.to_owned(),
                })
            })
            .collect()
    }

    pub fn current_tag_from_env(&self) -> Result<Option<String>> {
        let path = self.config.compose_env_path();
        envfile::get(&path, &self.config.image.tag_env)
    }

    pub fn persist_tag(&self, tag: &str) -> Result<()> {
        let path = self.config.compose_env_path();
        envfile::set(&path, &self.config.image.tag_env, tag)
    }

    pub fn check_tag_writable(&self) -> Result<()> {
        envfile::check_writable(&self.config.compose_env_path())
    }

    pub fn compose_build(&self, tag: &str, service: &str) -> Result<()> {
        let mut args = self.compose_args();
        args.push("build".to_owned());
        args.push(service.to_owned());
        command::checked(
            "docker",
            &args,
            Some(&self.config.compose.directory),
            &self.tag_env(tag),
        )?;
        Ok(())
    }

    pub fn compose_migrate(&self, tag: &str) -> Result<()> {
        let Some(service) = &self.config.deployment.migration_service else {
            return Ok(());
        };
        let mut args = self.compose_args();
        args.extend(["run".to_owned(), "--rm".to_owned(), service.clone()]);
        command::checked(
            "docker",
            &args,
            Some(&self.config.compose.directory),
            &self.tag_env(tag),
        )?;
        Ok(())
    }

    pub fn compose_up(&self, tag: &str, service: &str) -> Result<()> {
        let mut args = self.compose_args();
        args.extend([
            "up".to_owned(),
            "-d".to_owned(),
            "--no-build".to_owned(),
            "--no-deps".to_owned(),
        ]);
        args.push(service.to_owned());
        command::checked(
            "docker",
            &args,
            Some(&self.config.compose.directory),
            &self.tag_env(tag),
        )?;
        Ok(())
    }

    pub fn compose_ps(&self) -> Result<String> {
        let mut args = self.compose_args();
        args.extend([
            "ps".to_owned(),
            "--all".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
        ]);
        command::checked("docker", &args, Some(&self.config.compose.directory), &[])
    }

    pub fn runtime_report(&self, release: &Release) -> Result<(bool, String)> {
        let output = self.compose_ps()?;
        let containers: Vec<serde_json::Value> = if output.trim().is_empty() {
            Vec::new()
        } else if let Ok(array) = serde_json::from_str::<Vec<serde_json::Value>>(&output) {
            array
        } else {
            output
                .lines()
                .map(serde_json::from_str)
                .collect::<std::result::Result<_, _>>()?
        };
        let mut matched = true;
        let mut details = Vec::new();
        for service in &release.services {
            let observed = containers
                .iter()
                .find(|item| item["Service"] == service.name);
            let actual = observed.map_or("missing", |item| {
                item["Image"].as_str().unwrap_or("unknown")
            });
            let state = observed.map_or("missing", |item| {
                item["State"].as_str().unwrap_or("unknown")
            });
            if actual != service.image || state != "running" {
                matched = false;
            }
            details.push(format!(
                "{}: {} ({state}, expected {})",
                service.name, actual, service.image
            ));
        }
        Ok((matched, details.join("; ")))
    }

    pub fn activate(&self, release: &Release) -> Result<()> {
        for service in &release.services {
            self.compose_up(&release.tag, &service.name)
                .map_err(|source| YardError::Service {
                    service: service.name.clone(),
                    source: Box::new(source),
                })?;
        }
        crate::health::wait(&self.config.deployment).map_err(|source| YardError::Service {
            service: self.config.compose.services[0].clone(),
            source: Box::new(source),
        })?;
        let (matched, report) = self.runtime_report(release)?;
        if !matched {
            return Err(YardError::Config(format!(
                "release does not match Docker Compose: {report}"
            )));
        }
        Ok(())
    }

    pub fn compose_logs(&self, tail: u32, follow: bool) -> Result<()> {
        let mut args = self.compose_args();
        args.extend(["logs".to_owned(), "--tail".to_owned(), tail.to_string()]);
        if follow {
            args.push("--follow".to_owned());
        }
        args.extend(self.config.compose.services.iter().cloned());
        command::inherit("docker", &args, Some(&self.config.compose.directory), &[])
    }

    pub fn run_backup(&self, state: &mut ProjectState) -> Result<()> {
        let Some(backup) = &self.config.backup else {
            return Ok(());
        };
        // Clear the outcome of the previous run: a failed local attempt has no new off-site copy.
        state.last_offsite = None;
        let (started_at_unix, duration_ms, result) = self.backup_command(&backup.command);
        state.last_backup = Some(BackupAttempt {
            started_at_unix,
            duration_ms,
            result: if result.is_ok() {
                BackupResult::Success
            } else {
                BackupResult::Failure
            },
            destination: backup
                .directory
                .as_ref()
                .map(|path| path.display().to_string()),
        });
        state.save(&self.state_path)?;
        result?;

        if let Some(offsite_command) = &backup.offsite_command {
            let (started_at_unix, duration_ms, result) = self.backup_command(offsite_command);
            state.last_offsite = Some(BackupAttempt {
                started_at_unix,
                duration_ms,
                result: if result.is_ok() {
                    BackupResult::Success
                } else {
                    BackupResult::Failure
                },
                destination: backup.offsite_destination.clone(),
            });
            state.save(&self.state_path)?;
            result.map_err(|error| {
                YardError::Config(format!(
                    "off-site copy failed (local backup succeeded): {error}"
                ))
            })?;
        }
        Ok(())
    }

    fn backup_command(&self, command_line: &[String]) -> (u64, u128, Result<()>) {
        let started_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let start = Instant::now();
        let result = match command_line.split_first() {
            Some((program, args)) => command::inherit(program, args, Some(&self.config.repo), &[]),
            None => Err(YardError::Config("backup command is empty".into())),
        };
        (started_at_unix, start.elapsed().as_millis(), result)
    }

    fn git_checked(&self, args: &[&str]) -> Result<String> {
        command::checked(
            "git",
            &args
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
            Some(&self.config.repo),
            &[],
        )
    }

    fn tag_env(&self, tag: &str) -> Vec<(String, String)> {
        vec![(self.config.image.tag_env.clone(), tag.to_owned())]
    }

    pub(crate) fn compose_command_args(&self) -> Vec<String> {
        let mut args = vec!["compose".to_owned()];
        let env_file = self.config.compose_env_path();
        args.extend([
            "--env-file".to_owned(),
            env_file.to_string_lossy().into_owned(),
        ]);
        args.extend([
            "-f".to_owned(),
            self.config
                .compose_file_path()
                .to_string_lossy()
                .into_owned(),
        ]);
        args
    }

    fn compose_args(&self) -> Vec<String> {
        self.compose_command_args()
    }
}
