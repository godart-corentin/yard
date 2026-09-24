use std::fs;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;

use crate::error::{Result, YardError};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Monitor {
    deployment: MonitorDeployment,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorDeployment {
    health_url: String,
}

impl Monitor {
    // A URL-only manifest is useful to Yard Web and has no Git or Compose state.
    // Accept only this exact shape so a damaged deployment manifest stays an alert.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let contents = fs::read_to_string(path)?;
        let value: toml::Value = toml::from_str(&contents)?;
        let Some(table) = value.as_table() else {
            return Ok(None);
        };
        if table.len() != 1 || !table.contains_key("deployment") {
            return Ok(None);
        }
        let monitor: Self = toml::from_str(&contents)?;
        let url = reqwest::Url::parse(&monitor.deployment.health_url).map_err(|_| {
            YardError::Config("deployment.health_url must be an HTTP(S) URL".into())
        })?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(YardError::Config(
                "deployment.health_url must be an HTTP(S) URL without credentials".into(),
            ));
        }
        Ok(Some(monitor))
    }

    pub fn url(&self) -> &str {
        &self.deployment.health_url
    }

    pub fn check(&self) -> (bool, String) {
        let response = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .and_then(|client| client.get(self.url()).send());
        match response {
            Ok(response) if response.status().is_success() => {
                (true, format!("HTTP {}", response.status().as_u16()))
            }
            Ok(response) => (false, format!("HTTP {}", response.status().as_u16())),
            Err(_) => (false, "HTTP request failed or timed out".into()),
        }
    }
}
