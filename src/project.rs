use std::fs;
use std::path::{Path, PathBuf};

use crate::command;
use crate::config::ProjectConfig;
use crate::envfile;
use crate::error::{Result, YardError};

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub config: ProjectConfig,
    pub state_path: PathBuf,
}

impl Project {
    pub fn load(name: &str, projects_dir: &Path, state_dir: &Path) -> Result<Self> {
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

    pub fn image_ref(&self, tag: &str) -> String {
        format!("{}:{tag}", self.config.image.name)
    }

    pub fn image_exists(&self, tag: &str) -> Result<bool> {
        let args = vec![
            "image".to_owned(),
            "inspect".to_owned(),
            self.image_ref(tag),
        ];
        match command::checked("docker", &args, None, &[]) {
            Ok(_) => Ok(true),
            Err(YardError::CommandFailed { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn current_tag_from_env(&self) -> Result<Option<String>> {
        let path = self.config.compose_env_path();
        envfile::get(&path, &self.config.image.tag_env)
    }

    pub fn persist_tag(&self, tag: &str) -> Result<()> {
        let path = self.config.compose_env_path();
        envfile::set(&path, &self.config.image.tag_env, tag)
    }

    pub fn compose_build(&self, tag: &str) -> Result<()> {
        let mut args = self.compose_args();
        args.extend(["build".to_owned(), self.config.compose.service.clone()]);
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

    pub fn compose_up(&self, tag: &str) -> Result<()> {
        let mut args = self.compose_args();
        args.extend([
            "up".to_owned(),
            "-d".to_owned(),
            "--no-build".to_owned(),
            "--no-deps".to_owned(),
            self.config.compose.service.clone(),
        ]);
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
        args.push("ps".to_owned());
        command::checked("docker", &args, Some(&self.config.compose.directory), &[])
    }

    pub fn compose_logs(&self, tail: u32, follow: bool) -> Result<()> {
        let mut args = self.compose_args();
        args.extend(["logs".to_owned(), "--tail".to_owned(), tail.to_string()]);
        if follow {
            args.push("--follow".to_owned());
        }
        args.push(self.config.compose.service.clone());
        command::inherit("docker", &args, Some(&self.config.compose.directory), &[])
    }

    pub fn run_backup(&self) -> Result<()> {
        let Some(backup) = &self.config.backup else {
            return Ok(());
        };
        let (program, args) = backup
            .command
            .split_first()
            .ok_or_else(|| YardError::Config("backup.command is empty".into()))?;
        command::inherit(program, args, Some(&self.config.repo), &[])
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

    fn compose_args(&self) -> Vec<String> {
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
}
