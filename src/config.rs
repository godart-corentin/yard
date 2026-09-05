use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Result, YardError};

fn default_remote() -> String {
    "origin".to_owned()
}

fn default_health_attempts() -> u32 {
    30
}

fn default_health_interval_seconds() -> u64 {
    2
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    pub repo: PathBuf,
    pub branch: String,

    #[serde(default = "default_remote")]
    pub remote: String,

    pub compose: ComposeConfig,
    pub image: ImageConfig,

    #[serde(default)]
    pub deployment: DeploymentConfig,

    pub backup: Option<BackupConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComposeConfig {
    pub directory: PathBuf,
    pub file: String,
    pub env_file: String,
    pub service: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageConfig {
    pub name: String,
    pub tag_env: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentConfig {
    pub migration_service: Option<String>,
    pub health_url: Option<String>,

    #[serde(default = "default_health_attempts")]
    pub health_attempts: u32,

    #[serde(default = "default_health_interval_seconds")]
    pub health_interval_seconds: u64,
}

impl Default for DeploymentConfig {
    fn default() -> Self {
        Self {
            migration_service: None,
            health_url: None,
            health_attempts: default_health_attempts(),
            health_interval_seconds: default_health_interval_seconds(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BackupConfig {
    pub command: Vec<String>,
}

impl ProjectConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)?;
        let config: Self = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.branch.trim().is_empty() {
            return Err(YardError::Config("branch must not be empty".into()));
        }
        if self.remote.trim().is_empty() {
            return Err(YardError::Config("remote must not be empty".into()));
        }
        if self.compose.file.trim().is_empty() {
            return Err(YardError::Config("compose.file must not be empty".into()));
        }
        if self.compose.service.trim().is_empty() {
            return Err(YardError::Config("compose.service must not be empty".into()));
        }
        if self.compose.env_file.trim().is_empty() {
            return Err(YardError::Config("compose.env_file must not be empty".into()));
        }
        if self.image.name.trim().is_empty() {
            return Err(YardError::Config("image.name must not be empty".into()));
        }
        if !valid_env_name(&self.image.tag_env) {
            return Err(YardError::Config(format!(
                "image.tag_env is not a valid environment variable name: {}",
                self.image.tag_env
            )));
        }
        if self.deployment.health_attempts == 0 {
            return Err(YardError::Config(
                "deployment.health_attempts must be greater than zero".into(),
            ));
        }
        if self.deployment.health_interval_seconds == 0 {
            return Err(YardError::Config(
                "deployment.health_interval_seconds must be greater than zero".into(),
            ));
        }
        if let Some(backup) = &self.backup {
            if backup.command.is_empty() || backup.command[0].trim().is_empty() {
                return Err(YardError::Config(
                    "backup.command must contain an executable".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn compose_file_path(&self) -> PathBuf {
        self.compose.directory.join(&self.compose.file)
    }

    pub fn compose_env_path(&self) -> PathBuf {
        self.compose.directory.join(&self.compose.env_file)
    }
}

fn valid_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::valid_env_name;

    #[test]
    fn validates_environment_variable_names() {
        assert!(valid_env_name("APP_IMAGE_TAG"));
        assert!(valid_env_name("_TAG2"));
        assert!(!valid_env_name("2TAG"));
        assert!(!valid_env_name("APP-TAG"));
        assert!(!valid_env_name(""));
    }
}
