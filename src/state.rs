use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic_file;
use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub revision: String,
    pub tag: String,
    pub deployed_at_unix: u64,
    #[serde(default)]
    pub services: Vec<ReleaseService>,
    #[serde(default = "active_status")]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseService {
    pub name: String,
    pub image: String,
}

fn active_status() -> String {
    "active".to_owned()
}

impl Release {
    pub fn new(revision: String, tag: String) -> Self {
        let deployed_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            revision,
            tag,
            deployed_at_unix,
            services: Vec::new(),
            status: active_status(),
        }
    }

    pub fn with_services(mut self, services: Vec<ReleaseService>) -> Self {
        self.services = services;
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectState {
    pub current: Option<Release>,
    pub previous: Option<Release>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Release>,
}

impl ProjectState {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&contents)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_file::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(self)?).as_bytes(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn preexisting_temp_file_is_not_followed_and_state_is_private() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/state-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{}.json", std::process::id()));
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, "stale").unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();
        ProjectState::default().save(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(&tmp).unwrap(), "stale");
        fs::remove_file(tmp).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn existing_state_mode_is_preserved() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/state-mode-test");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        fs::write(&path, "{}\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        ProjectState::default().save(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        fs::remove_dir_all(root).unwrap();
    }
}
