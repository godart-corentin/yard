use std::thread;
use std::time::Duration;

use reqwest::blocking::Client;
use tracing::{debug, warn};

use crate::config::DeploymentConfig;
use crate::error::{Result, YardError};

pub fn wait(config: &DeploymentConfig) -> Result<()> {
    let Some(url) = &config.health_url else {
        return Ok(());
    };

    let client = Client::builder().timeout(Duration::from_secs(5)).build()?;

    for attempt in 1..=config.health_attempts {
        match client.get(url).send() {
            Ok(response) if response.status().is_success() => {
                debug!(url, attempt, status = %response.status(), "health check passed");
                return Ok(());
            }
            Ok(response) => {
                warn!(url, attempt, status = %response.status(), "health check returned non-success");
            }
            Err(error) => {
                warn!(url, attempt, %error, "health check request failed");
            }
        }

        if attempt < config.health_attempts {
            thread::sleep(Duration::from_secs(config.health_interval_seconds));
        }
    }

    Err(YardError::HealthCheck(url.clone()))
}
