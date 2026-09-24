//! CLI-only service probes. The Web process only consumes the sanitized host snapshot.
use std::env;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::Serialize;

use crate::command;
use crate::config::ServiceProbe;
use crate::host::Container;
use crate::project::Project;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct ServiceHealth {
    pub project: String,
    pub service: String,
    pub status: Health,
    pub kind: Option<&'static str>,
    pub latency_ms: Option<u64>,
    pub age_seconds: Option<u64>,
    pub heartbeat_at_unix: Option<u64>,
    pub max_age_seconds: Option<u64>,
    pub crit_multiplier: Option<u64>,
    pub message: Option<&'static str>,
}

#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub http_warn_ms: u64,
    pub http_crit_ms: u64,
    pub heartbeat_crit_multiplier: u64,
}

impl Thresholds {
    pub fn from_env() -> Self {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let mut read = |name, default| {
            lookup(name)
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(default)
        };
        let warn = read("YARD_HTTP_WARN_MS", 500);
        Self {
            http_warn_ms: warn,
            http_crit_ms: read("YARD_HTTP_CRIT_MS", 2000).max(warn),
            heartbeat_crit_multiplier: read("YARD_HEARTBEAT_CRIT_MULTIPLIER", 2).max(2),
        }
    }
}

fn http_health(ms: u64, thresholds: Thresholds) -> Health {
    if ms > thresholds.http_crit_ms {
        Health::Unhealthy
    } else if ms > thresholds.http_warn_ms {
        Health::Degraded
    } else {
        Health::Healthy
    }
}

fn heartbeat_health(age: u64, max_age: u64, thresholds: Thresholds) -> Health {
    if age > max_age.saturating_mul(thresholds.heartbeat_crit_multiplier) {
        Health::Unhealthy
    } else if age > max_age {
        Health::Degraded
    } else {
        Health::Healthy
    }
}

pub fn collect(
    project: &Project,
    containers: Option<&[Container]>,
    thresholds: Thresholds,
) -> Vec<ServiceHealth> {
    project
        .config
        .compose
        .services
        .iter()
        .map(|name| {
            let probe = project.config.service_health.get(name);
            let mut result = ServiceHealth {
                project: project.name.clone(),
                service: name.clone(),
                status: Health::Unknown,
                kind: probe.map(|p| match p {
                    ServiceProbe::Http { .. } => "http",
                    ServiceProbe::Heartbeat { .. } => "heartbeat",
                }),
                latency_ms: None,
                age_seconds: None,
                message: None,
                heartbeat_at_unix: None,
                max_age_seconds: None,
                crit_multiplier: None,
            };
            let Some(probe) = probe else {
                result.message = Some("No service probe configured");
                return result;
            };
            if !containers.is_some_and(|rows| rows.iter().any(|c| c.service == *name)) {
                result.message = Some("Container unavailable");
                return result;
            }
            if containers.is_some_and(|rows| {
                rows.iter()
                    .any(|c| c.service == *name && !c.state.eq_ignore_ascii_case("running"))
            }) {
                result.status = Health::Unhealthy;
                result.message = Some("Container stopped");
                return result;
            }
            match probe {
                ServiceProbe::Http { url, timeout_ms } => {
                    let client = Client::builder()
                        .timeout(Duration::from_millis(*timeout_ms))
                        .redirect(reqwest::redirect::Policy::none())
                        .build();
                    let started = Instant::now();
                    match client.and_then(|client| client.get(url).send()) {
                        Ok(response) => {
                            result.latency_ms = Some(
                                started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                            );
                            if response.status().is_success() {
                                result.status = http_health(result.latency_ms.unwrap(), thresholds);
                            } else {
                                result.status = Health::Unhealthy;
                                result.message = Some("HTTP non-success response");
                            }
                        }
                        Err(_) => {
                            result.latency_ms = Some(
                                started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                            );
                            result.status = Health::Unhealthy;
                            result.message = Some("HTTP request failed or timed out");
                        }
                    }
                }
                ServiceProbe::Heartbeat {
                    path,
                    max_age_seconds,
                } => {
                    result.max_age_seconds = Some(*max_age_seconds);
                    result.crit_multiplier = Some(thresholds.heartbeat_crit_multiplier);
                    // stat runs inside the existing service container; no host mount or worker agent.
                    let mut args = project.compose_command_args();
                    args.extend([
                        "exec".into(),
                        "-T".into(),
                        name.clone(),
                        "stat".into(),
                        "-c".into(),
                        "%Y".into(),
                        "--".into(),
                        path.to_string_lossy().into_owned(),
                    ]);
                    let stamp = command::checked(
                        "docker",
                        &args,
                        Some(&project.config.compose.directory),
                        &[],
                    )
                    .ok()
                    .and_then(|text| text.parse::<u64>().ok());
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    match stamp.and_then(|stamp| now.checked_sub(stamp)) {
                        Some(age) => {
                            result.age_seconds = Some(age);
                            result.heartbeat_at_unix = stamp;
                            result.status = heartbeat_health(age, *max_age_seconds, thresholds);
                        }
                        None => {
                            result.message = Some("Heartbeat missing, unreadable or in the future")
                        }
                    }
                }
            }
            result
        })
        .collect()
}

pub fn render(services: &[ServiceHealth]) -> String {
    let mut lines = vec!["Service health".to_owned()];
    for service in services {
        let measured = match (service.latency_ms, service.age_seconds) {
            (Some(ms), _) => format!("{ms} ms"),
            (_, Some(age)) => format!("{age} s since heartbeat"),
            _ => service.message.unwrap_or("unavailable").to_owned(),
        };
        lines.push(format!(
            "  {} / {}: {measured} [{:?}]",
            service.project, service.service, service.status
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_change_both_probe_classifications() {
        let limits = Thresholds::from_lookup(|name| match name {
            "YARD_HTTP_WARN_MS" => Some("25".into()),
            "YARD_HTTP_CRIT_MS" => Some("50".into()),
            "YARD_HEARTBEAT_CRIT_MULTIPLIER" => Some("3".into()),
            _ => None,
        });
        assert_eq!(http_health(26, limits), Health::Degraded);
        assert_eq!(http_health(51, limits), Health::Unhealthy);
        assert_eq!(heartbeat_health(31, 30, limits), Health::Degraded);
        assert_eq!(heartbeat_health(91, 30, limits), Health::Unhealthy);
        assert_eq!(heartbeat_health(30, 30, limits), Health::Healthy);
    }
}
