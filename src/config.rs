use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{de, Deserialize, Deserializer};

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

    #[serde(default)]
    pub service_health: BTreeMap<String, ServiceProbe>,

    pub backup: Option<BackupConfig>,
}

#[derive(Debug, Clone)]
pub struct ComposeConfig {
    pub directory: PathBuf,
    pub file: String,
    pub env_file: String,
    pub services: Vec<String>,
}

impl<'de> Deserialize<'de> for ComposeConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            directory: PathBuf,
            file: String,
            env_file: String,
            service: Option<String>,
            services: Option<Vec<String>>,
        }
        let raw = Raw::deserialize(deserializer)?;
        let services = match (raw.service, raw.services) {
            (Some(_), Some(_)) => {
                return Err(de::Error::custom(
                    "compose.service and compose.services are mutually exclusive",
                ))
            }
            (Some(service), None) => vec![service],
            (None, Some(services)) => services,
            (None, None) => {
                return Err(de::Error::custom(
                    "compose.service or compose.services is required",
                ))
            }
        };
        Ok(Self {
            directory: raw.directory,
            file: raw.file,
            env_file: raw.env_file,
            services,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageConfig {
    pub name: String,
    pub tag_env: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ServiceProbe {
    Http { url: String, timeout_ms: u64 },
    Heartbeat { path: PathBuf, max_age_seconds: u64 },
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
    pub directory: Option<PathBuf>,
    pub extension: Option<String>,
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
        if self.compose.services.is_empty() {
            return Err(YardError::Config(
                "compose.services must not be empty".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for service in &self.compose.services {
            if service.trim().is_empty() {
                return Err(YardError::Config(
                    "compose.services contains an empty name".into(),
                ));
            }
            if !valid_service_name(service) {
                return Err(YardError::Config(format!(
                    "compose.services contains invalid service name: {service}"
                )));
            }
            if !seen.insert(service) {
                return Err(YardError::Config(format!(
                    "compose.services contains duplicate service: {service}"
                )));
            }
        }
        if self.compose.env_file.trim().is_empty() {
            return Err(YardError::Config(
                "compose.env_file must not be empty".into(),
            ));
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
        for (name, probe) in &self.service_health {
            if !self.compose.services.contains(name) {
                return Err(YardError::Config(format!(
                    "service_health.{name} is not a configured Compose service"
                )));
            }
            match probe {
                ServiceProbe::Http { url, timeout_ms } => {
                    let valid = reqwest::Url::parse(url).ok().is_some_and(|parsed| {
                        matches!(parsed.scheme(), "http" | "https")
                            && parsed.host().is_some()
                            && parsed.username().is_empty()
                            && parsed.password().is_none()
                    });
                    if !valid || *timeout_ms == 0 {
                        return Err(YardError::Config(format!(
                            "service_health.{name} requires an HTTP(S) URL without credentials and a positive timeout_ms"
                        )));
                    }
                }
                ServiceProbe::Heartbeat {
                    path,
                    max_age_seconds,
                } => {
                    if !path.is_absolute()
                        || path
                            .components()
                            .any(|part| matches!(part, std::path::Component::ParentDir))
                        || *max_age_seconds == 0
                    {
                        return Err(YardError::Config(format!(
                            "service_health.{name} requires an absolute path without '..' and a positive max_age_seconds"
                        )));
                    }
                }
            }
        }
        if let Some(backup) = &self.backup {
            if backup.command.is_empty() || backup.command[0].trim().is_empty() {
                return Err(YardError::Config(
                    "backup.command must contain an executable".into(),
                ));
            }
            if backup
                .extension
                .as_deref()
                .is_some_and(|extension| extension.trim().is_empty())
            {
                return Err(YardError::Config(
                    "backup.extension must not be empty when configured".into(),
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

fn valid_service_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_alphanumeric())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::{valid_env_name, ProjectConfig};

    #[test]
    fn validates_environment_variable_names() {
        assert!(valid_env_name("APP_IMAGE_TAG"));
        assert!(valid_env_name("_TAG2"));
        assert!(!valid_env_name("2TAG"));
        assert!(!valid_env_name("APP-TAG"));
        assert!(!valid_env_name(""));
    }

    #[test]
    fn service_probes_are_optional_but_must_target_configured_services() {
        let base = "repo = '/srv/app'\nbranch = 'main'\n[compose]\ndirectory = '/srv/app'\nfile = 'compose.yml'\nenv_file = '.env'\nservices = ['api', 'worker']\n[image]\nname = 'app'\ntag_env = 'APP_TAG'\n";
        let legacy: ProjectConfig = toml::from_str(base).unwrap();
        legacy.validate().unwrap();
        assert!(legacy.service_health.is_empty());
        let configured = format!("{base}\n[service_health.api]\ntype = 'http'\nurl = 'http://127.0.0.1:8080/health'\ntimeout_ms = 800\n[service_health.worker]\ntype = 'heartbeat'\npath = '/run/yard/heartbeat'\nmax_age_seconds = 30\n");
        let parsed: ProjectConfig = toml::from_str(&configured).unwrap();
        parsed.validate().unwrap();
        assert_eq!(parsed.service_health.len(), 2);
        for bad in [
            format!("{base}\n[service_health.other]\ntype = 'heartbeat'\npath = '/run/beat'\nmax_age_seconds = 30\n"),
            format!("{base}\n[service_health.worker]\ntype = 'heartbeat'\npath = '../beat'\nmax_age_seconds = 30\n"),
            format!("{base}\n[service_health.api]\ntype = 'http'\nurl = 'file:///etc/passwd'\ntimeout_ms = 800\n"),
        ] {
            let parsed: ProjectConfig = toml::from_str(&bad).unwrap();
            assert!(parsed.validate().is_err());
        }
    }
}
