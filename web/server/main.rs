use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone)]
struct Config {
    host: String,
    port: u16,
    projects_dir: PathBuf,
    state_dir: PathBuf,
    static_dir: PathBuf,
    check_timeout: Duration,
    cache_duration: Duration,
    host_max_age_seconds: u64,
    read_socket: PathBuf,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            host: env_value("YARD_WEB_HOST", "0.0.0.0"),
            port: env_number("YARD_WEB_PORT", 8088)?,
            projects_dir: PathBuf::from(env_value("YARD_PROJECTS_DIR", "/etc/yard/projects")),
            state_dir: PathBuf::from(env_value("YARD_STATE_DIR", "/var/lib/yard")),
            static_dir: PathBuf::from(env_value("YARD_WEB_STATIC", "/opt/yard/static")),
            check_timeout: Duration::from_secs_f64(env_number(
                "YARD_WEB_CHECK_TIMEOUT_SECONDS",
                5.0,
            )?),
            cache_duration: Duration::from_secs_f64(env_number("YARD_WEB_CACHE_SECONDS", 15.0)?),
            host_max_age_seconds: env_number("YARD_WEB_HOST_MAX_AGE_SECONDS", 300)?,
            read_socket: PathBuf::from(env_value(
                "YARD_WEB_READ_SOCKET",
                "/run/yard-web-read/read.sock",
            )),
        })
    }
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn env_number<T>(name: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr + Copy,
    T::Err: std::fmt::Display,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| format!("invalid {name}: {error}")),
        Err(_) => Ok(default),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectFile {
    deployment: Option<Deployment>,
    service_health: Option<toml::Value>,
    compose: Option<toml::Value>,
    backup: Option<WebBackup>,
}

#[derive(Debug, Deserialize)]
struct WebBackup {
    offsite_command: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BackupAttempt {
    started_at_unix: u64,
    duration_ms: u128,
    result: BackupResult,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum BackupResult {
    Success,
    Failure,
}

// Keep unreadable records visible without exposing their raw contents through the API.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
enum BackupRecord {
    Attempt(BackupAttempt),
    Invalid(InvalidBackup),
}

#[derive(Clone, Debug, Serialize)]
struct InvalidBackup {
    result: &'static str,
}

#[derive(Debug, Deserialize)]
struct Deployment {
    health_url: Option<String>,
}

#[derive(Clone)]
struct Project {
    name: String,
    health_url: Option<String>,
    has_service_probes: bool,
    service_names: Vec<String>,
    url_monitor: bool,
    state_status: &'static str,
    release: ReleaseView,
    previous_release: ReleaseView,
    pending_release: ReleaseView,
    last_backup: Option<BackupRecord>,
    last_offsite: Option<BackupRecord>,
    offsite_configured: Option<bool>,
    config_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ProjectStatus {
    #[serde(serialize_with = "serialize_label")]
    name: String,
    #[serde(serialize_with = "serialize_labels")]
    application_services: Vec<String>,
    url_monitor: bool,
    // A monitor URL may contain tokens in its path or query; keep it server-side.
    #[serde(skip_serializing)]
    health_url: Option<String>,
    services: Vec<ServiceHealth>,
    #[serde(skip)]
    uses_service_probes: bool,
    state_status: &'static str,
    release: ReleaseView,
    previous_release: ReleaseView,
    pending_release: ReleaseView,
    restore_points: Vec<RestorePoint>,
    cli: Option<Diagnostic>,
    last_backup: Option<BackupRecord>,
    last_offsite: Option<BackupRecord>,
    offsite_configured: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_error: Option<String>,
    checked_at: String,
    latency_ms: Option<u128>,
    http_status: Option<u16>,
    status: String,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct StatusPayload {
    status: String,
    checked_at: String,
    projects: Vec<ProjectStatus>,
    host: HostStatus,
}

#[derive(Clone, Debug, Serialize)]
struct ReleaseView {
    record_status: &'static str,
    tag: Option<String>,
    revision: Option<String>,
    deployed_at_unix: Option<u64>,
    status: Option<String>,
    services: Vec<ReleaseServiceView>,
}

impl ReleaseView {
    fn empty(record_status: &'static str) -> Self {
        Self {
            record_status,
            tag: None,
            revision: None,
            deployed_at_unix: None,
            status: None,
            services: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct ReleaseServiceView {
    name: String,
    image: String,
}

#[derive(Deserialize)]
struct RecordedRelease {
    revision: String,
    tag: String,
    deployed_at_unix: u64,
    #[serde(default)]
    services: Vec<RecordedService>,
    #[serde(default = "active_status")]
    status: String,
}
fn active_status() -> String {
    "active".into()
}
#[derive(Deserialize)]
struct RecordedService {
    name: String,
    image: String,
}

#[derive(Clone, Debug, Serialize)]
struct RestorePoint {
    position: &'static str,
    id: String,
    deployed_at_unix: u64,
    status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Diagnostic {
    name: String,
    verdict: String,
    reason: Option<String>,
    branch: Option<String>,
    head: Option<String>,
    env_tag: Option<String>,
    runtime: Option<Vec<RuntimeView>>,
    drift: Vec<String>,
    repo_bytes: Option<u64>,
    measured_at_unix: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeView {
    service: String,
    state: String,
    health: String,
    image: String,
}

// Deserialize only the public, versioned snapshot fields. Never proxy arbitrary JSON from disk.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostMetric<T> {
    value: Option<T>,
    status: HostLevel,
    message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HostLevel {
    Normal,
    Warning,
    Critical,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostThresholds {
    disk_warn_percent: u8,
    disk_crit_percent: u8,
    mem_pressure_percent: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostMemory {
    used_bytes: u64,
    total_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostDisk {
    #[serde(deserialize_with = "safe_mount")]
    mount: String,
    used_bytes: u64,
    total_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostDocker {
    images: String,
    containers: String,
    volumes: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostContainer {
    project: String,
    service: String,
    state: String,
    status: HostLevel,
}

// Explicit allowlist: never forward arbitrary snapshot or manifest fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct ServiceHealth {
    project: String,
    service: String,
    status: ServiceLevel,
    kind: Option<String>,
    latency_ms: Option<u64>,
    age_seconds: Option<u64>,
    #[serde(default, skip_serializing)]
    heartbeat_at_unix: Option<u64>,
    #[serde(default, skip_serializing)]
    max_age_seconds: Option<u64>,
    #[serde(default, skip_serializing)]
    crit_multiplier: Option<u64>,
    message: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ServiceLevel {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HostSnapshot {
    version: u8,
    collected_at_unix: u64,
    thresholds: HostThresholds,
    cpu: HostMetric<f64>,
    load: HostMetric<[f64; 3]>,
    memory: HostMetric<HostMemory>,
    disks: Vec<HostMetric<HostDisk>>,
    docker: HostMetric<HostDocker>,
    containers: Vec<HostContainer>,
    containers_status: HostLevel,
    containers_message: Option<String>,
    #[serde(default)]
    services: Vec<ServiceHealth>,
}

#[derive(Clone, Debug, Serialize)]
struct HostStatus {
    status: &'static str,
    age_seconds: Option<u64>,
    snapshot: Option<HostSnapshot>,
    message: &'static str,
}

fn load_host(state_dir: &Path, now: u64, max_age: u64) -> HostStatus {
    let unavailable = |age_seconds, message| HostStatus {
        status: "unknown",
        age_seconds,
        snapshot: None,
        message,
    };
    let Ok(contents) = fs::read_to_string(state_dir.join("host.json")) else {
        return unavailable(None, "Host snapshot unavailable");
    };
    let Ok(mut snapshot) = serde_json::from_str::<HostSnapshot>(&contents) else {
        return unavailable(None, "Host snapshot invalid");
    };
    if snapshot.version != 1 {
        return unavailable(None, "Host snapshot version unknown");
    }
    let Some(age) = now.checked_sub(snapshot.collected_at_unix) else {
        return unavailable(None, "Host snapshot timestamp invalid");
    };
    if age > max_age {
        return unavailable(Some(age), "Host snapshot stale");
    }
    // The on-disk snapshot is not trusted browser text.
    snapshot.cpu.message = None;
    snapshot.load.message = None;
    snapshot.memory.message = None;
    snapshot.docker.message = None;
    for disk in &mut snapshot.disks {
        disk.message = None;
    }
    if let Some(docker) = &mut snapshot.docker.value {
        docker.images = safe_usage(&docker.images);
        docker.containers = safe_usage(&docker.containers);
        docker.volumes = safe_usage(&docker.volumes);
    }
    snapshot.containers_message = snapshot.containers_message.filter(|message| {
        matches!(
            message.as_str(),
            "No containers for a configured project" | "Containers unavailable"
        )
    });
    for container in &mut snapshot.containers {
        container.project = safe_label(&container.project);
        container.service = safe_label(&container.service);
        if !matches!(
            container.state.as_str(),
            "running" | "exited" | "paused" | "restarting" | "created" | "dead"
        ) {
            container.state = "unknown".into();
        }
    }
    for service in &mut snapshot.services {
        service.project = safe_label(&service.project);
        service.service = safe_label(&service.service);
        service.kind = service
            .kind
            .take()
            .filter(|kind| matches!(kind.as_str(), "http" | "heartbeat"));
        service.message = service.message.take().filter(|message| {
            matches!(
                message.as_str(),
                "No service probe configured"
                    | "Container unavailable"
                    | "Container stopped"
                    | "HTTP non-success response"
                    | "HTTP request failed or timed out"
                    | "Heartbeat missing, unreadable or in the future"
            )
        });
        if service.kind.as_deref() != Some("heartbeat") {
            continue;
        }
        match (
            service.heartbeat_at_unix,
            service.max_age_seconds,
            service.crit_multiplier,
        ) {
            (Some(stamp), Some(max), Some(multiplier)) if max > 0 && multiplier >= 2 => {
                if let Some(age) = now.checked_sub(stamp) {
                    service.age_seconds = Some(age);
                    service.status = if age > max.saturating_mul(multiplier) {
                        ServiceLevel::Unhealthy
                    } else if age > max {
                        ServiceLevel::Degraded
                    } else {
                        ServiceLevel::Healthy
                    };
                } else {
                    service.status = ServiceLevel::Unknown;
                    service.age_seconds = None;
                }
            }
            _ if service.status == ServiceLevel::Healthy => service.status = ServiceLevel::Unknown,
            _ => {}
        }
    }
    HostStatus {
        status: "available",
        age_seconds: Some(age),
        snapshot: Some(snapshot),
        message: "",
    }
}

fn safe_mount<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let mount = String::deserialize(deserializer)?;
    Ok(if mount == "/" {
        "/".into()
    } else {
        "(mount hidden)".into()
    })
}

fn safe_label(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        value.into()
    } else {
        "(redacted)".into()
    }
}

fn serialize_label<S: serde::Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&safe_label(value))
}

fn serialize_labels<S: serde::Serializer>(
    values: &[String],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    values
        .iter()
        .map(|value| safe_label(value))
        .collect::<Vec<_>>()
        .serialize(serializer)
}

fn safe_usage(value: &str) -> String {
    let size = value.trim_end_matches(|b: char| b.is_ascii_alphabetic());
    let unit = &value[size.len()..];
    if value.len() <= 24
        && !size.is_empty()
        && matches!(unit, "B" | "kB" | "KB" | "MB" | "GB" | "TB" | "PB")
        && size.bytes().filter(|b| *b == b'.').count() <= 1
        && size.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && !size.starts_with('.')
        && !size.ends_with('.')
    {
        value.to_owned()
    } else {
        "(redacted)".into()
    }
}

fn safe_image(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 160
        && !value.starts_with('/')
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':'))
    {
        value.into()
    } else {
        "(redacted)".into()
    }
}

fn release_view(value: Option<&Value>, missing: &'static str) -> ReleaseView {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return ReleaseView::empty(missing);
    };
    match serde_json::from_value::<RecordedRelease>(value.clone()) {
        Ok(release) => ReleaseView {
            record_status: "recorded",
            tag: Some(safe_label(&release.tag)),
            revision: Some(safe_label(&release.revision)),
            deployed_at_unix: Some(release.deployed_at_unix),
            status: Some(
                if matches!(
                    release.status.as_str(),
                    "active" | "superseded" | "activating"
                ) {
                    release.status
                } else {
                    "(redacted)".into()
                },
            ),
            services: release
                .services
                .into_iter()
                .map(|service| ReleaseServiceView {
                    name: safe_label(&service.name),
                    image: safe_image(&service.image),
                })
                .collect(),
        },
        Err(_) => ReleaseView::empty("invalid"),
    }
}

fn restore_points(
    current: &ReleaseView,
    previous: &ReleaseView,
    pending: &ReleaseView,
) -> Vec<RestorePoint> {
    [
        ("previous", previous),
        ("current", current),
        ("pending", pending),
    ]
    .into_iter()
    .filter_map(|(position, record)| {
        (record.record_status == "recorded" && record.tag.as_deref() != Some("(redacted)"))
            .then(|| {
                Some(RestorePoint {
                    position,
                    id: format!("release:{}", record.tag.as_deref()?),
                    deployed_at_unix: record.deployed_at_unix?,
                    status: record.status.clone()?,
                })
            })
            .flatten()
    })
    .collect()
}

fn safe_http_url(value: &str) -> bool {
    reqwest::Url::parse(value).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn monitor_url(contents: &str) -> Result<Option<String>, ()> {
    let value: toml::Value = toml::from_str(contents).map_err(|_| ())?;
    let root = value.as_table().ok_or(())?;
    if root.len() != 1 || !root.contains_key("deployment") {
        return Ok(None);
    }
    let deployment = root
        .get("deployment")
        .and_then(toml::Value::as_table)
        .ok_or(())?;
    if deployment.len() != 1 {
        return Err(());
    }
    let url = deployment
        .get("health_url")
        .and_then(toml::Value::as_str)
        .ok_or(())?;
    if !safe_http_url(url) {
        return Err(());
    }
    Ok(Some(url.to_owned()))
}

struct Cache {
    value: Option<StatusPayload>,
    valid_until: Instant,
}

struct App {
    config: Config,
    client: Client,
    cache: Mutex<Cache>,
}

impl App {
    fn new(config: Config) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(config.check_timeout)
            .user_agent("yard-web/1")
            .build()
            .map_err(|error| format!("cannot create HTTP client: {error}"))?;
        Ok(Self {
            config,
            client,
            cache: Mutex::new(Cache {
                value: None,
                valid_until: Instant::now(),
            }),
        })
    }

    fn status(&self) -> StatusPayload {
        if let Some(mut value) = self.cached_status() {
            value.host = self.host_status();
            // Live CLI verdicts and recorded release views must refer to the same state.
            for project in &mut value.projects {
                let (state_status, release, previous, pending, backup, offsite) =
                    load_project_state(&self.config.state_dir, &project.name);
                project.state_status = state_status;
                project.restore_points = restore_points(&release, &previous, &pending);
                project.release = release;
                project.previous_release = previous;
                project.pending_release = pending;
                project.last_backup = backup;
                project.last_offsite = offsite;
            }
            apply_service_snapshot(&mut value);
            self.apply_cli(&mut value);
            return value;
        }

        let projects = load_projects(&self.config.projects_dir, &self.config.state_dir);
        let checked = check_projects(projects, &self.client);
        let mut payload = StatusPayload {
            status: overall_status(&checked).to_owned(),
            checked_at: utc_now(),
            projects: checked,
            host: self.host_status(),
        };
        apply_service_snapshot(&mut payload);

        let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        cache.valid_until = Instant::now() + self.config.cache_duration;
        cache.value = Some(payload.clone());
        drop(cache);
        self.apply_cli(&mut payload);
        payload
    }

    fn apply_cli(&self, payload: &mut StatusPayload) {
        let diagnostics = load_diagnostics(&self.config.read_socket);
        for project in &mut payload.projects {
            project.cli = diagnostics
                .as_ref()
                .and_then(|items| items.iter().find(|item| item.name == project.name))
                .cloned();
            project.status = project_live_status(project).into();
            if !project.url_monitor {
                project.error = match project.cli.as_ref().and_then(|item| item.reason.as_deref()) {
                    Some("state_unreadable") => Some("State unreadable".into()),
                    Some("manifest_unreadable") => {
                        Some("Project manifest unreadable or invalid".into())
                    }
                    _ if project.cli.is_none() => Some("CLI live diagnostics unavailable".into()),
                    _ => None,
                };
            } else if project.cli.is_none() {
                project.error = Some("CLI live diagnostics unavailable".into());
            }
        }
        payload.status = overall_status(&payload.projects).to_owned();
    }

    fn cached_status(&self) -> Option<StatusPayload> {
        let cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        if Instant::now() < cache.valid_until {
            cache.value.clone()
        } else {
            None
        }
    }

    fn host_status(&self) -> HostStatus {
        load_host(
            &self.config.state_dir,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            self.config.host_max_age_seconds,
        )
    }
}

// CLI "alert" covers drift, pending releases and missing measurements as well as
// failures. Only an explicit live health failure is Down; a host container
// snapshot is never used to decide the project badge.
fn project_live_status(project: &ProjectStatus) -> &'static str {
    let cli = project.cli.as_ref();
    if project.url_monitor {
        if project.status == "down" {
            return "down"; // Web HTTP sample failed, even if CLI is unavailable.
        }
        if cli.is_some_and(|item| {
            item.verdict == "alert" && item.reason.as_deref() == Some("url_monitor")
        }) {
            return "down"; // The separate CLI URL sample failed.
        }
        if cli.is_some_and(|item| item.verdict == "alert") {
            return if project.status == "operational" {
                "degraded" // A non-health CLI alert alongside a successful Web sample.
            } else {
                "unknown" // No health sample and no explicit health failure.
            };
        }
        return if project.status == "operational" {
            "operational"
        } else {
            "unknown"
        };
    }
    // The host service snapshot may describe individual probes, but without a
    // live CLI diagnosis it cannot establish the Compose project's badge.
    if cli.is_none() {
        return "unknown";
    }
    if cli.is_some_and(|item| {
        item.runtime.as_ref().is_some_and(|rows| {
            rows.iter()
                .any(|row| row.state == "stopped" || row.health == "unhealthy")
        })
    }) || project.uses_service_probes && project.status == "unhealthy"
    {
        return "down";
    }
    if project.uses_service_probes && project.status == "degraded" {
        return "degraded";
    }
    let measured = cli
        .is_some_and(|item| item.runtime.as_ref().is_some_and(|rows| !rows.is_empty()))
        || project.uses_service_probes && project.status == "healthy";
    if !measured {
        return "unknown";
    }
    match cli.map(|item| item.verdict.as_str()) {
        Some("alert") => "degraded",
        Some("ok") => "operational",
        _ => "unknown",
    }
}

fn load_diagnostics(socket: &Path) -> Option<Vec<Diagnostic>> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(13)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    stream.write_all(b"{\"op\":\"diagnostics\"}\n").ok()?;
    let mut bytes = Vec::new();
    stream.take(1_048_577).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 1_048_576 {
        return None;
    }
    let reply: Value = serde_json::from_slice(&bytes).ok()?;
    let parsed =
        serde_json::from_value::<Vec<Diagnostic>>(reply.get("diagnostics")?.clone()).ok()?;
    parsed
        .into_iter()
        .map(|mut item| {
            if !matches!(item.verdict.as_str(), "ok" | "alert") {
                return None;
            }
            item.name = safe_label(&item.name);
            item.branch = item.branch.as_deref().map(safe_label);
            item.head = item.head.as_deref().map(safe_label);
            item.env_tag = item.env_tag.as_deref().map(safe_label);
            item.drift = item.drift.iter().map(|value| safe_label(value)).collect();
            item.reason = item.reason.filter(|reason| {
                matches!(
                    reason.as_str(),
                    "url_monitor" | "state_unreadable" | "manifest_unreadable"
                )
            });
            for row in item.runtime.iter_mut().flatten() {
                row.service = safe_label(&row.service);
                if !matches!(row.state.as_str(), "running" | "completed" | "stopped") {
                    return None;
                }
                if !matches!(
                    row.health.as_str(),
                    "healthy" | "unhealthy" | "starting" | "unknown"
                ) {
                    return None;
                }
                if !matches!(row.image.as_str(), "recorded" | "unrecorded" | "unknown") {
                    return None;
                }
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
            if item.measured_at_unix > now + 5 || now.saturating_sub(item.measured_at_unix) > 30 {
                return None;
            }
            Some(item)
        })
        .collect()
}

fn load_projects(projects_dir: &Path, state_dir: &Path) -> Vec<Project> {
    let Ok(entries) = fs::read_dir(projects_dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("toml"))
        .collect();
    paths.sort();

    paths
        .into_iter()
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_owned();
            if name == "host" {
                return None;
            }
            let (
                state_status,
                release,
                previous_release,
                pending_release,
                last_backup,
                last_offsite,
            ) = load_project_state(state_dir, &name);
            let parsed = fs::read_to_string(&path)
                .map_err(|_| ())
                .and_then(|contents| {
                    let monitor = monitor_url(&contents)?;
                    let config = toml::from_str::<ProjectFile>(&contents).map_err(|_| ())?;
                    if config.compose.is_none()
                        && contents.contains("[deployment]")
                        && monitor.is_none()
                    {
                        return Err(());
                    }
                    Ok((config, monitor))
                });

            Some(match parsed {
                Ok((config, monitor)) => Project {
                    name,
                    url_monitor: monitor.is_some(),
                    has_service_probes: config.service_health.as_ref().is_some_and(|probes| {
                        probes.as_table().is_some_and(|table| !table.is_empty())
                    }),
                    service_names: config
                        .compose
                        .as_ref()
                        .map(|compose| {
                            compose
                                .get("services")
                                .and_then(toml::Value::as_array)
                                .map(|services| {
                                    services
                                        .iter()
                                        .filter_map(toml::Value::as_str)
                                        .map(str::to_owned)
                                        .collect()
                                })
                                .or_else(|| {
                                    compose
                                        .get("service")
                                        .and_then(toml::Value::as_str)
                                        .map(|name| vec![name.to_owned()])
                                })
                                .unwrap_or_default()
                        })
                        .unwrap_or_default(),
                    health_url: monitor.or_else(|| {
                        config
                            .deployment
                            .and_then(|deployment| deployment.health_url)
                            .map(|url| url.trim().to_owned())
                            .filter(|url| safe_http_url(url))
                    }),
                    state_status,
                    release,
                    previous_release,
                    pending_release,
                    last_backup,
                    last_offsite,
                    offsite_configured: Some(
                        config
                            .backup
                            .and_then(|backup| backup.offsite_command)
                            .is_some(),
                    ),
                    config_error: None,
                },
                Err(_) => Project {
                    name,
                    url_monitor: false,
                    health_url: None,
                    has_service_probes: false,
                    service_names: vec![],
                    state_status,
                    release,
                    previous_release,
                    pending_release,
                    last_backup,
                    last_offsite,
                    offsite_configured: None,
                    config_error: Some("Project manifest unreadable or invalid".into()),
                },
            })
        })
        .collect()
}

fn load_project_state(
    state_dir: &Path,
    project: &str,
) -> (
    &'static str,
    ReleaseView,
    ReleaseView,
    ReleaseView,
    Option<BackupRecord>,
    Option<BackupRecord>,
) {
    // host.json is reserved for the CLI's host snapshot, never project state.
    if project == "host" {
        return (
            "missing",
            ReleaseView::empty("missing"),
            ReleaseView::empty("missing"),
            ReleaseView::empty("missing"),
            None,
            None,
        );
    }
    let (status, state) = match fs::read_to_string(state_dir.join(format!("{project}.json"))) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ("missing", None),
        Err(_) => ("invalid", None),
        Ok(contents) => match serde_json::from_str::<Value>(&contents) {
            Ok(value) if value.is_object() => ("recorded", Some(value)),
            _ => ("invalid", None),
        },
    };
    let backup = |name| {
        state
            .as_ref()
            .and_then(|value| value.get(name))
            .filter(|value| !value.is_null())
            .map(
                |value| match serde_json::from_value::<BackupAttempt>(value.clone()) {
                    Ok(attempt) => BackupRecord::Attempt(attempt),
                    Err(_) => BackupRecord::Invalid(InvalidBackup { result: "invalid" }),
                },
            )
    };
    let field = |name| {
        release_view(
            state.as_ref().and_then(|value| value.get(name)),
            if status == "invalid" {
                "unavailable"
            } else {
                "missing"
            },
        )
    };
    (
        status,
        field("current"),
        field("previous"),
        field("pending"),
        backup("last_backup"),
        backup("last_offsite"),
    )
}

fn check_projects(projects: Vec<Project>, client: &Client) -> Vec<ProjectStatus> {
    if projects.is_empty() {
        return Vec::new();
    }

    let len = projects.len();
    let workers = len.min(8);
    let queue = Mutex::new(projects.into_iter().enumerate().collect::<VecDeque<_>>());
    let results = Mutex::new(vec![None; len]);

    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let next = queue
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .pop_front();
                let Some((index, project)) = next else {
                    break;
                };
                let status = check_project(project, client);
                results.lock().unwrap_or_else(|error| error.into_inner())[index] = Some(status);
            });
        }
    });

    results
        .into_inner()
        .unwrap_or_else(|error| error.into_inner())
        .into_iter()
        .flatten()
        .collect()
}

fn check_project(project: Project, client: &Client) -> ProjectStatus {
    let mut result = ProjectStatus {
        name: project.name.clone(),
        application_services: project.service_names.clone(),
        url_monitor: project.url_monitor,
        health_url: project.health_url,
        services: project
            .service_names
            .into_iter()
            .filter(|_| project.has_service_probes)
            .map(|service| ServiceHealth {
                project: project.name.clone(),
                service,
                status: ServiceLevel::Unknown,
                kind: None,
                latency_ms: None,
                age_seconds: None,
                heartbeat_at_unix: None,
                max_age_seconds: None,
                crit_multiplier: None,
                message: Some("Service snapshot unavailable".to_owned()),
            })
            .collect(),
        uses_service_probes: project.has_service_probes,
        state_status: project.state_status,
        restore_points: restore_points(
            &project.release,
            &project.previous_release,
            &project.pending_release,
        ),
        release: project.release,
        previous_release: project.previous_release,
        pending_release: project.pending_release,
        cli: None,
        last_backup: project.last_backup,
        last_offsite: project.last_offsite,
        offsite_configured: project.offsite_configured,
        config_error: project.config_error.clone(),
        checked_at: utc_now(),
        latency_ms: None,
        http_status: None,
        status: "unknown".to_owned(),
        error: None,
    };

    if let Some(error) = project.config_error {
        result.error = Some(error);
        return result;
    }

    if !project.url_monitor {
        result.error = Some("CLI live diagnostics unavailable".to_owned());
        return result;
    }

    if project.has_service_probes {
        result.error = Some("Service snapshot unavailable".to_owned());
        return result;
    }

    let Some(health_url) = result.health_url.as_deref() else {
        result.error = Some("No deployment.health_url configured".to_owned());
        return result;
    };

    let valid_url = reqwest::Url::parse(health_url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some());
    if valid_url.is_none() {
        result.error = Some("Health URL must use http or https".to_owned());
        return result;
    }

    let started = Instant::now();
    match client.get(health_url).send() {
        Ok(mut response) => {
            let status = response.status().as_u16();
            let mut body = Vec::with_capacity(1024);
            let _ = response.by_ref().take(1024).read_to_end(&mut body);
            result.latency_ms = Some(started.elapsed().as_millis());
            result.http_status = Some(status);
            if (200..300).contains(&status) {
                result.status = "operational".to_owned();
            } else {
                result.status = "down".to_owned();
                result.error = Some(format!("HTTP {status}"));
            }
        }
        Err(error) => {
            result.status = "down".to_owned();
            let _ = error;
            result.error = Some("HTTP request failed or timed out".to_owned());
        }
    }
    result
}

fn service_project_status(services: &[ServiceHealth]) -> &'static str {
    if services.is_empty() {
        return "unknown";
    }
    let healthy = services
        .iter()
        .filter(|s| s.status == ServiceLevel::Healthy)
        .count();
    let unhealthy = services
        .iter()
        .filter(|s| s.status == ServiceLevel::Unhealthy)
        .count();
    if healthy == services.len() {
        "healthy"
    } else if unhealthy == services.len() {
        "unhealthy"
    } else if unhealthy > 0 || services.iter().any(|s| s.status == ServiceLevel::Degraded) {
        "degraded"
    } else {
        "unknown"
    }
}

fn apply_service_snapshot(payload: &mut StatusPayload) {
    let fresh = if payload.host.status == "available" {
        payload
            .host
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.services.as_slice())
    } else {
        None
    };
    for project in &mut payload.projects {
        if !project.uses_service_probes {
            project.services.clear();
            continue;
        }
        let measured: Vec<_> = fresh
            .unwrap_or(&[])
            .iter()
            .filter(|service| service.project == project.name)
            .cloned()
            .collect();
        let available = !measured.is_empty();
        if available {
            project.services = measured;
        } else {
            for service in &mut project.services {
                service.status = ServiceLevel::Unknown;
                service.latency_ms = None;
                service.age_seconds = None;
                service.message = Some("Service snapshot unavailable".to_owned());
            }
        }
        project.status = service_project_status(&project.services).to_owned();
        project.error = if !available {
            Some("Service snapshot unavailable".to_owned())
        } else {
            None
        };
    }
    payload.status = overall_status(&payload.projects).to_owned();
}

fn overall_status(projects: &[ProjectStatus]) -> &'static str {
    if projects.is_empty() {
        return "unknown";
    }
    let operational = projects
        .iter()
        .filter(|project| matches!(project.status.as_str(), "operational" | "healthy"))
        .count();
    let down = projects
        .iter()
        .filter(|project| matches!(project.status.as_str(), "down" | "unhealthy"))
        .count();
    if operational == projects.len() {
        "operational"
    } else if operational == 0 && down > 0 {
        "down"
    } else if down > 0 || projects.iter().any(|project| project.status == "degraded") {
        "degraded"
    } else {
        "unknown"
    }
}

fn utc_now() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let base = format_unix_utc(elapsed.as_secs());
    format!(
        "{}.{:06}Z",
        base.trim_end_matches('Z'),
        elapsed.subsec_micros()
    )
}

fn format_unix_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    // Gregorian civil date conversion by Howard Hinnant, with the Unix epoch offset.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

// A second, deliberately small allowlist in Web. The executor independently validates
// the same operation and project against the real Yard manifest before spawning Yard.
fn read_request(target: &str, config: &Config) -> Option<Value> {
    let url = reqwest::Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != "/api/read" || !target.starts_with("/api/read?") {
        return None;
    }
    let mut params = std::collections::BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if !matches!(
            key.as_ref(),
            "op" | "project" | "service" | "tail" | "since"
        ) || params
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return None;
        }
    }
    let op = params.remove("op")?;
    if op == "images" {
        return params
            .is_empty()
            .then(|| serde_json::json!({"op": "images"}));
    }
    if !matches!(op.as_str(), "logs" | "restore-points" | "restore-log") {
        return None;
    }
    let project = params.remove("project")?;
    if project.is_empty()
        || project.len() > 64
        || !project
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    let loaded = load_projects(&config.projects_dir, &config.state_dir);
    let selected = loaded.iter().find(|item| item.name == project)?;
    if selected.service_names.is_empty() || selected.config_error.is_some() {
        return None;
    }
    if op != "logs" {
        return params
            .is_empty()
            .then(|| serde_json::json!({"op": op, "project": project}));
    }
    let service = params.remove("service");
    if service
        .as_deref()
        .is_some_and(|name| !selected.service_names.iter().any(|valid| valid == name))
    {
        return None;
    }
    let tail = params
        .remove("tail")
        .map(|value| value.parse::<u32>())
        .transpose()
        .ok()?;
    if tail.is_some_and(|count| count == 0 || count > 1000) {
        return None;
    }
    let since = params.remove("since");
    if since.as_deref().is_some_and(|value| {
        value.is_empty()
            || value.len() > 40
            || !value.bytes().all(|byte| {
                byte.is_ascii_digit()
                    || matches!(
                        byte,
                        b'T' | b'Z' | b'+' | b'-' | b':' | b'.' | b's' | b'm' | b'h' | b'd'
                    )
            })
    }) {
        return None;
    }
    params.is_empty().then(|| serde_json::json!({"op": op, "project": project, "service": service, "tail": tail, "since": since}))
}

fn read_result(socket: &Path, request: &Value) -> Result<Value, u16> {
    let mut stream = UnixStream::connect(socket).map_err(|_| 503u16)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(13)))
        .map_err(|_| 503u16)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| 503u16)?;
    serde_json::to_writer(&mut stream, request).map_err(|_| 503u16)?;
    stream.write_all(b"\n").map_err(|_| 503u16)?;
    let mut bytes = Vec::new();
    stream
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| 503u16)?;
    if bytes.len() > 1_048_576 {
        return Err(502);
    }
    let reply: Value = serde_json::from_slice(&bytes).map_err(|_| 502u16)?;
    if let Some(output) = reply.get("output").and_then(Value::as_str) {
        return (output.len() <= 131_072)
            .then(|| serde_json::json!({"output": output}))
            .ok_or(502);
    }
    match reply.get("error").and_then(Value::as_str) {
        Some("Read operation refused") => Err(400),
        Some(_) => Err(502),
        None => Err(502),
    }
}

fn serve_read(stream: &mut TcpStream, target: &str, app: &App) -> Result<(), String> {
    let request = read_request(target, &app.config);
    let result = request
        .ok_or(400)
        .and_then(|request| read_result(&app.config.read_socket, &request));
    let (status, body) = match result {
        Ok(value) => (200, value),
        Err(400) => (400, serde_json::json!({"error": "Read operation refused"})),
        Err(503) => (
            503,
            serde_json::json!({"error": "Read executor unavailable"}),
        ),
        Err(_) => (
            502,
            serde_json::json!({"error": "Read operation failed or output limit exceeded"}),
        ),
    };
    let bytes = serde_json::to_vec(&body).map_err(|error| error.to_string())?;
    write_response(
        stream,
        status,
        "application/json; charset=utf-8",
        &bytes,
        "no-store",
    )
}

fn serve(app: Arc<App>) -> Result<(), String> {
    let address = format!("{}:{}", app.config.host, app.config.port);
    let listener = TcpListener::bind(&address)
        .map_err(|error| format!("cannot listen on {address}: {error}"))?;
    println!("Yard Web listening on {address}");

    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                let app = Arc::clone(&app);
                thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, &app) {
                        eprintln!("yard-web request failed: {error}");
                    }
                });
            }
            Err(error) => eprintln!("yard-web accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(mut stream: TcpStream, app: &App) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;

    let mut reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|error| error.to_string())?;
    if request_line.len() > 8_192 {
        return write_response(
            &mut stream,
            414,
            "text/plain; charset=utf-8",
            b"URI too long\n",
            "no-store",
        );
    }

    let mut header_bytes = request_line.len();
    loop {
        let mut header = String::new();
        let read = reader
            .read_line(&mut header)
            .map_err(|error| error.to_string())?;
        if read == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        header_bytes += read;
        if header_bytes > 32_768 {
            return write_response(
                &mut stream,
                431,
                "text/plain; charset=utf-8",
                b"Request headers too large\n",
                "no-store",
            );
        }
    }

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or("/");
    if method != "GET" {
        return write_response(
            &mut stream,
            405,
            "text/plain; charset=utf-8",
            b"Method not allowed\n",
            "no-store",
        );
    }

    match path {
        "/api/read" => serve_read(&mut stream, target, app),
        "/healthz" => write_response(
            &mut stream,
            200,
            "application/json; charset=utf-8",
            b"{\"status\":\"ok\"}\n",
            "no-store",
        ),
        "/api/status" => {
            let mut body = serde_json::to_vec(&app.status()).map_err(|error| error.to_string())?;
            body.push(b'\n');
            write_response(
                &mut stream,
                200,
                "application/json; charset=utf-8",
                &body,
                "no-store",
            )
        }
        "/" => serve_static(&mut stream, &app.config.static_dir, "index.html"),
        "/index.html" => serve_static(&mut stream, &app.config.static_dir, "index.html"),
        "/styles.css" => serve_static(&mut stream, &app.config.static_dir, "styles.css"),
        "/app.js" => serve_static(&mut stream, &app.config.static_dir, "app.js"),
        "/favicon.svg" => serve_static(&mut stream, &app.config.static_dir, "favicon.svg"),
        _ => write_response(
            &mut stream,
            404,
            "text/plain; charset=utf-8",
            b"Not found\n",
            "no-store",
        ),
    }
}

fn serve_static(stream: &mut TcpStream, directory: &Path, filename: &str) -> Result<(), String> {
    let path = directory.join(filename);
    let body = match fs::read(&path) {
        Ok(body) => body,
        Err(_) => {
            return write_response(
                stream,
                404,
                "text/plain; charset=utf-8",
                b"Not found\n",
                "no-store",
            )
        }
    };
    let content_type = match Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
    {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    write_response(stream, 200, content_type, &body, "no-cache")
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    cache_control: &str,
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        404 => "Not Found",
        405 => "Method Not Allowed",
        414 => "URI Too Long",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: {cache_control}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| error.to_string())
}

fn healthcheck(config: &Config) -> Result<(), String> {
    let address = format!("127.0.0.1:{}", config.port);
    let mut stream = TcpStream::connect_timeout(
        &address
            .parse()
            .map_err(|error| format!("invalid address: {error}"))?,
        Duration::from_secs(2),
    )
    .map_err(|error| format!("cannot reach yard-web: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    if response.starts_with("HTTP/1.1 200 ") {
        Ok(())
    } else {
        Err("yard-web health check returned a non-success response".to_owned())
    }
}

fn run() -> Result<(), String> {
    let config = Config::from_env()?;
    if env::args().nth(1).as_deref() == Some("--healthcheck") {
        return healthcheck(&config);
    }
    serve(Arc::new(App::new(config)?))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("yard-web: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_HOST: &str = r#"{"version":1,"collected_at_unix":1000,"thresholds":{"disk_warn_percent":80,"disk_crit_percent":90,"mem_pressure_percent":85},"cpu":{"value":40.0,"status":"normal","message":null},"load":{"value":[1,2,3],"status":"normal","message":null},"memory":{"value":{"used_bytes":50,"total_bytes":100},"status":"warning","message":null},"disks":[],"docker":{"value":null,"status":"unknown","message":"Docker unavailable"},"containers":[],"containers_status":"unknown","containers_message":null,"secret":"DO_NOT_EXPOSE"}"#;

    fn project(status: &str) -> ProjectStatus {
        ProjectStatus {
            name: "test".to_owned(),
            application_services: vec![],
            url_monitor: false,
            health_url: None,
            services: vec![],
            uses_service_probes: false,
            state_status: "missing",
            release: ReleaseView::empty("missing"),
            previous_release: ReleaseView::empty("missing"),
            pending_release: ReleaseView::empty("missing"),
            restore_points: vec![],
            cli: None,
            last_backup: None,
            last_offsite: None,
            offsite_configured: Some(false),
            config_error: None,
            checked_at: "2026-01-01T00:00:00Z".to_owned(),
            latency_ms: None,
            http_status: None,
            status: status.to_owned(),
            error: None,
        }
    }

    fn diagnostic(verdict: &str, runtime: Option<Vec<RuntimeView>>) -> Diagnostic {
        Diagnostic {
            name: "test".into(),
            verdict: verdict.into(),
            reason: None,
            branch: None,
            head: None,
            env_tag: None,
            runtime,
            drift: vec![],
            repo_bytes: None,
            measured_at_unix: 0,
        }
    }

    #[test]
    fn alert_without_health_failure_is_not_down() {
        let mut measured = project("unknown");
        measured.cli = Some(diagnostic(
            "alert",
            Some(vec![RuntimeView {
                service: "api".into(),
                state: "running".into(),
                health: "healthy".into(),
                image: "recorded".into(),
            }]),
        ));
        assert_eq!(project_live_status(&measured), "degraded");
        measured.cli.as_mut().unwrap().runtime = None;
        assert_eq!(project_live_status(&measured), "unknown");
        assert_eq!(overall_status(&[project("unknown")]), "unknown");
    }

    #[test]
    fn live_failure_is_down_but_missing_cli_is_unknown() {
        let mut measured = project("unknown");
        measured.cli = Some(diagnostic(
            "alert",
            Some(vec![RuntimeView {
                service: "api".into(),
                state: "stopped".into(),
                health: "unknown".into(),
                image: "unknown".into(),
            }]),
        ));
        assert_eq!(project_live_status(&measured), "down");
        measured.cli = None;
        assert_eq!(project_live_status(&measured), "unknown");
        measured.uses_service_probes = true;
        for status in ["healthy", "degraded", "unhealthy"] {
            measured.status = status.into();
            assert_eq!(project_live_status(&measured), "unknown");
        }
        measured.uses_service_probes = false;
        measured.url_monitor = true;
        measured.status = "down".into(); // Independent Web HTTP failure.
        assert_eq!(project_live_status(&measured), "down");
        measured.status = "operational".into();
        measured.cli = Some(diagnostic("alert", None));
        measured.cli.as_mut().unwrap().reason = Some("url_monitor".into());
        assert_eq!(project_live_status(&measured), "down"); // CLI sample failed.
    }

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            env::temp_dir().join(format!("yard-web-{label}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(format_unix_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix_utc(1_704_067_199), "2023-12-31T23:59:59Z");
    }

    #[test]
    fn loads_health_url_and_current_release() {
        let root = temp_dir("load");
        let projects = root.join("projects");
        let state = root.join("state");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(
            projects.join("hello.toml"),
            "[deployment]\nhealth_url = \"https://example.test/health\"\n",
        )
        .unwrap();
        fs::write(
            state.join("hello.json"),
            r#"{"current":{"revision":"abcdef1234567890","tag":"abcdef123456","deployed_at_unix":123},"previous":null}"#,
        )
        .unwrap();

        let loaded = load_projects(&projects, &state);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "hello");
        assert_eq!(
            loaded[0].health_url.as_deref(),
            Some("https://example.test/health")
        );
        assert_eq!(loaded[0].release.tag.as_deref(), Some("abcdef123456"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_boundary_hides_url_tokens_paths_arbitrary_state_and_snapshot_text() {
        let root = temp_dir("redaction");
        let projects = root.join("projects");
        let state = root.join("state");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(
            projects.join("demo.toml"),
            "[deployment]\nhealth_url = 'https://example.test/secret/TOKEN?key=TOKEN'\n",
        )
        .unwrap();
        fs::write(state.join("demo.json"), r#"{"current":{"revision":"abc","tag":"/private/TOKEN","deployed_at_unix":42,"services":[{"name":"api","image":"/private/TOKEN"}],"secret":"TOKEN"},"previous":{"revision":"def","tag":"safe","deployed_at_unix":40},"last_backup":{"started_at_unix":40,"duration_ms":1,"result":"success","destination":"/private/TOKEN"},"unknown":"TOKEN"}"#).unwrap();
        let mut checked = check_project(load_projects(&projects, &state).remove(0), &Client::new());
        let json = serde_json::to_value(&checked).unwrap();
        assert!(json.get("health_url").is_none());
        assert_eq!(json["state_status"], "recorded");
        assert_eq!(json["release"]["tag"], "(redacted)");
        assert_eq!(json["release"]["services"][0]["image"], "(redacted)");
        assert_eq!(json["previous_release"]["tag"], "safe");
        assert_eq!(json["restore_points"].as_array().unwrap().len(), 1);
        assert!(!json.to_string().contains("TOKEN"));
        checked.name = "/private/TOKEN".into();
        checked.application_services = vec!["/private/TOKEN".into()];
        let exposed = serde_json::to_value(&checked).unwrap();
        assert_eq!(exposed["name"], "(redacted)");
        assert_eq!(exposed["application_services"][0], "(redacted)");
        let mut hostile: Value = serde_json::from_str(VALID_HOST).unwrap();
        hostile["disks"] = serde_json::json!([{"status":"normal","value":{"mount":"/private/TOKEN","used_bytes":1,"total_bytes":2},"message":"TOKEN"}]);
        hostile["containers"] = serde_json::json!([{"project":"demo","service":"/private/TOKEN","state":"running","status":"normal"}]);
        hostile["containers_message"] = serde_json::json!("TOKEN");
        hostile["docker"] = serde_json::json!({"value":{"images":"TOKEN/secret","containers":"2GB","volumes":"0B"},"status":"normal","message":"TOKEN"});
        fs::write(state.join("host.json"), hostile.to_string()).unwrap();
        let host = serde_json::to_value(load_host(&state, 1001, 300)).unwrap();
        assert_eq!(
            host["snapshot"]["disks"][0]["value"]["mount"],
            "(mount hidden)"
        );
        assert_eq!(host["snapshot"]["docker"]["value"]["images"], "(redacted)");
        assert!(!host.to_string().contains("TOKEN"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_and_malformed_release_records_are_distinct_and_not_restore_targets() {
        let root = temp_dir("invalid-release");
        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        assert_eq!(load_project_state(&state, "demo").0, "missing");
        fs::write(state.join("demo.json"), "{broken").unwrap();
        let (status, current, _, _, _, _) = load_project_state(&state, "demo");
        assert_eq!(status, "invalid");
        assert_eq!(current.record_status, "unavailable");
        fs::write(state.join("demo.json"), r#"{"current":{"tag":"partial"},"previous":{"revision":"def","tag":"prior","deployed_at_unix":42},"pending":null}"#).unwrap();
        let (status, current, previous, pending, _, _) = load_project_state(&state, "demo");
        assert_eq!(status, "recorded");
        assert_eq!(current.record_status, "invalid");
        assert_eq!(pending.record_status, "missing");
        assert_eq!(restore_points(&current, &previous, &pending).len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn live_socket_separates_alerts_failures_and_missing_measurements() {
        use std::os::unix::net::UnixListener;
        let dir = temp_dir("socket-verdict");
        let socket = dir.join("read.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            assert_eq!(request, "{\"op\":\"diagnostics\"}\n");
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let reply = serde_json::json!({"diagnostics":[
                {"name":"monitor","verdict":"alert","reason":"url_monitor","branch":null,"head":null,"env_tag":null,"runtime":null,"drift":[],"repo_bytes":null,"measured_at_unix":now},
                {"name":"demo","verdict":"ok","reason":null,"branch":"main","head":"abcd","env_tag":null,"runtime":[],"drift":[],"repo_bytes":0,"measured_at_unix":now},
                {"name":"reviewdesk","verdict":"alert","reason":null,"branch":"main","head":"abcd","env_tag":null,"runtime":[{"service":"api","state":"running","health":"healthy","image":"recorded"}],"drift":["api"],"repo_bytes":0,"measured_at_unix":now}
            ]});
            stream.write_all(format!("{reply}\n").as_bytes()).unwrap();
        });
        let app = App::new(Config {
            host: "127.0.0.1".into(),
            port: 0,
            projects_dir: dir.clone(),
            state_dir: dir.clone(),
            static_dir: dir.clone(),
            check_timeout: Duration::from_secs(1),
            cache_duration: Duration::from_secs(1),
            host_max_age_seconds: 300,
            read_socket: socket,
        })
        .unwrap();
        let mut payload = StatusPayload {
            status: "operational".into(),
            checked_at: utc_now(),
            host: app.host_status(),
            projects: vec![
                ProjectStatus {
                    name: "monitor".into(),
                    url_monitor: true,
                    ..project("down")
                },
                ProjectStatus {
                    name: "demo".into(),
                    ..project("down")
                },
                ProjectStatus {
                    name: "reviewdesk".into(),
                    ..project("unknown")
                },
            ],
        };
        app.apply_cli(&mut payload);
        server.join().unwrap();
        assert_eq!(payload.projects[0].status, "down");
        assert_eq!(payload.projects[1].status, "unknown");
        assert_eq!(payload.projects[2].status, "degraded");
        assert_eq!(payload.status, "down");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn backup_payload_reads_only_recorded_fields_and_offsite_configuration() {
        let root = temp_dir("backups");
        let projects = root.join("projects");
        let state = root.join("state");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(
            projects.join("demo.toml"),
            "[backup]\noffsite_command = [\"copy\"]\n",
        )
        .unwrap();
        fs::write(state.join("demo.json"), r#"{"current":null,"last_backup":{"started_at_unix":123,"duration_ms":7,"result":"success","destination":"/archive","secret":"not-public"},"last_offsite":{"started_at_unix":124,"duration_ms":8,"result":"failure","destination":"remote:test"}}"#).unwrap();
        let checked = check_project(load_projects(&projects, &state).remove(0), &Client::new());
        let payload = serde_json::to_value(checked).unwrap();
        assert_eq!(payload["last_backup"]["result"], "success");
        assert_eq!(payload["last_offsite"]["result"], "failure");
        assert!(payload["last_offsite"].get("destination").is_none());
        assert_eq!(payload["offsite_configured"], true);
        assert!(payload["last_backup"].get("secret").is_none());
        fs::write(
            state.join("demo.json"),
            "{\"current\":null,\"previous\":null}",
        )
        .unwrap();
        let old = serde_json::to_value(check_project(
            load_projects(&projects, &state).remove(0),
            &Client::new(),
        ))
        .unwrap();
        assert!(old["last_backup"].is_null());
        assert!(old["last_offsite"].is_null());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_backup_records_are_not_reported_as_missing() {
        let root = temp_dir("invalid-backups");
        let projects = root.join("projects");
        let state = root.join("state");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(
            projects.join("demo.toml"),
            "[backup]\noffsite_command = [\"copy\"]\n",
        )
        .unwrap();

        for malformed in [
            serde_json::json!({"result": "unknown-state", "started_at_unix": 123, "duration_ms": 7}),
            serde_json::json!({"result": "success", "started_at_unix": 123, "duration_ms": 12.5}),
            serde_json::json!({"result": "success", "started_at_unix": "yesterday", "duration_ms": 7}),
        ] {
            fs::write(
                state.join("demo.json"),
                serde_json::to_vec(
                    &serde_json::json!({"last_backup": malformed, "last_offsite": malformed}),
                )
                .unwrap(),
            )
            .unwrap();
            let checked = check_project(load_projects(&projects, &state).remove(0), &Client::new());
            let payload = serde_json::to_value(checked).unwrap();
            assert_eq!(payload["last_backup"]["result"], "invalid");
            assert_eq!(payload["last_offsite"]["result"], "invalid");
            assert_eq!(payload["offsite_configured"], true);
            assert!(payload["last_backup"].get("started_at_unix").is_none());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exposes_pending_release_alongside_active_release() {
        let root = temp_dir("pending");
        let projects = root.join("projects");
        let state = root.join("state");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(projects.join("demo.toml"), "[deployment]\n").unwrap();
        fs::write(state.join("demo.json"), r#"{"current":{"revision":"abcd1234","tag":"old","deployed_at_unix":100},"pending":{"revision":"abcd1235","tag":"new","deployed_at_unix":101,"status":"activating","services":[{"name":"api","image":"api:new"}]}}"#).unwrap();
        let project = load_projects(&projects, &state).remove(0);
        let checked = check_project(project, &Client::new());
        let payload = serde_json::to_value(checked).unwrap();
        assert_eq!(payload["release"]["tag"], "old");
        assert_eq!(
            payload["pending_release"]["services"][0]["image"],
            "api:new"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_without_health_url_is_unknown() {
        let client = Client::builder().build().unwrap();
        let checked = check_project(
            Project {
                name: "hello".to_owned(),
                health_url: None,
                url_monitor: false,
                has_service_probes: false,
                service_names: vec![],
                state_status: "missing",
                release: ReleaseView::empty("missing"),
                previous_release: ReleaseView::empty("missing"),
                pending_release: ReleaseView::empty("missing"),
                last_backup: None,
                last_offsite: None,
                offsite_configured: Some(false),
                config_error: None,
            },
            &client,
        );
        assert_eq!(checked.status, "unknown");
        assert!(checked
            .error
            .unwrap()
            .contains("CLI live diagnostics unavailable"));
    }

    #[test]
    fn project_without_probes_has_no_service_rows_even_with_a_snapshot() {
        let client = Client::builder().build().unwrap();
        let checked = check_project(
            Project {
                name: "demo".to_owned(),
                health_url: None,
                url_monitor: false,
                has_service_probes: false,
                service_names: vec!["api".to_owned()],
                state_status: "missing",
                release: ReleaseView::empty("missing"),
                previous_release: ReleaseView::empty("missing"),
                pending_release: ReleaseView::empty("missing"),
                last_backup: None,
                last_offsite: None,
                offsite_configured: Some(false),
                config_error: None,
            },
            &client,
        );
        assert!(checked.services.is_empty());

        let dir = temp_dir("no-probes");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut snapshot: Value = serde_json::from_str(VALID_HOST).unwrap();
        snapshot["collected_at_unix"] = serde_json::json!(now);
        snapshot["services"] = serde_json::json!([{
            "project": "demo", "service": "api", "status": "unknown",
            "kind": null, "latency_ms": null, "age_seconds": null,
            "message": "No service probe configured"
        }]);
        fs::write(
            dir.join("host.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        let mut payload = StatusPayload {
            status: "operational".into(),
            checked_at: String::new(),
            projects: vec![ProjectStatus {
                name: "demo".into(),
                ..project("operational")
            }],
            host: load_host(&dir, now, 300),
        };
        apply_service_snapshot(&mut payload);
        assert!(payload.projects[0].services.is_empty());
        assert_eq!(payload.projects[0].status, "operational");
        payload.host = load_host(&dir, now + 301, 300);
        apply_service_snapshot(&mut payload);
        assert!(payload.projects[0].services.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn config_errors_preserve_the_existing_payload_field() {
        let client = Client::builder().build().unwrap();
        let checked = check_project(
            Project {
                name: "broken".to_owned(),
                health_url: None,
                url_monitor: false,
                has_service_probes: false,
                service_names: vec![],
                state_status: "missing",
                release: ReleaseView::empty("missing"),
                previous_release: ReleaseView::empty("missing"),
                pending_release: ReleaseView::empty("missing"),
                last_backup: None,
                last_offsite: None,
                offsite_configured: None,
                config_error: Some("invalid TOML".to_owned()),
            },
            &client,
        );
        let payload = serde_json::to_value(checked).unwrap();
        assert_eq!(payload["config_error"], "invalid TOML");
        assert_eq!(payload["error"], "invalid TOML");
        assert_eq!(payload["status"], "unknown");
    }

    #[test]
    fn successful_health_check_is_operational() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let checked = check_project(
            Project {
                name: "hello".to_owned(),
                health_url: Some(format!("http://{address}/health")),
                url_monitor: true,
                has_service_probes: false,
                service_names: vec![],
                state_status: "missing",
                release: ReleaseView::empty("missing"),
                previous_release: ReleaseView::empty("missing"),
                pending_release: ReleaseView::empty("missing"),
                last_backup: None,
                last_offsite: None,
                offsite_configured: Some(false),
                config_error: None,
            },
            &client,
        );
        server.join().unwrap();
        assert_eq!(checked.status, "operational");
        assert_eq!(checked.http_status, Some(200));
        assert!(checked.latency_ms.is_some());
    }

    #[test]
    fn overall_status_degrades_when_one_project_is_down() {
        assert_eq!(
            overall_status(&[project("operational"), project("down")]),
            "degraded"
        );
    }

    #[test]
    fn missing_malformed_and_unknown_host_snapshots_are_unavailable() {
        let dir = temp_dir("host-invalid");
        assert_eq!(load_host(&dir, 1000, 300).status, "unknown");
        fs::write(dir.join("host.json"), "{").unwrap();
        assert_eq!(load_host(&dir, 1000, 300).status, "unknown");
        let mut unknown: Value = serde_json::from_str(VALID_HOST).unwrap();
        unknown["version"] = serde_json::json!(99);
        fs::write(dir.join("host.json"), serde_json::to_vec(&unknown).unwrap()).unwrap();
        let loaded = load_host(&dir, 1000, 300);
        assert_eq!(loaded.status, "unknown");
        assert_eq!(loaded.message, "Host snapshot version unknown");
        assert!(loaded.snapshot.is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn never_deployed_project_remains_unknown_in_web_payload() {
        let dir = temp_dir("host-empty-compose");
        let mut snapshot: Value = serde_json::from_str(VALID_HOST).unwrap();
        snapshot["containers_message"] =
            serde_json::json!("No containers for a configured project");
        fs::write(
            dir.join("host.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        let host = load_host(&dir, 1000, 300);
        assert_eq!(host.status, "available");
        let payload = serde_json::to_value(&host).unwrap();
        assert_eq!(payload["snapshot"]["containers_status"], "unknown");
        assert_eq!(
            payload["snapshot"]["containers_message"],
            "No containers for a configured project"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn host_snapshot_cannot_be_loaded_as_a_project_release() {
        let dir = temp_dir("host-reserved");
        fs::write(
            dir.join("host.json"),
            r#"{"current":{"tag":"wrong"},"pending":{"tag":"also-wrong"}}"#,
        )
        .unwrap();
        let (status, current, previous, pending, local, offsite) = load_project_state(&dir, "host");
        assert_eq!(status, "missing");
        assert_eq!(current.record_status, "missing");
        assert_eq!(previous.record_status, "missing");
        assert_eq!(pending.record_status, "missing");
        assert!(local.is_none() && offsite.is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reserved_host_manifest_is_not_a_project() {
        let dir = temp_dir("host-manifest");
        fs::write(dir.join("host.toml"), "[deployment]\n").unwrap();
        assert!(load_projects(&dir, &dir).is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cached_project_health_does_not_cache_host_snapshot() {
        let dir = temp_dir("host-cache");
        let config = Config {
            host: "127.0.0.1".into(),
            port: 0,
            projects_dir: dir.clone(),
            state_dir: dir.clone(),
            static_dir: dir.clone(),
            check_timeout: Duration::from_secs(1),
            cache_duration: Duration::from_secs(60),
            host_max_age_seconds: 300,
            read_socket: dir.join("missing.sock"),
        };
        let app = App::new(config).unwrap();
        assert_eq!(app.status().host.status, "unknown");
        let mut snapshot: serde_json::Value = serde_json::from_str(r#"{"version":1,"collected_at_unix":1000,"thresholds":{"disk_warn_percent":80,"disk_crit_percent":90,"mem_pressure_percent":85},"cpu":{"value":40.0,"status":"normal","message":null},"load":{"value":[1,2,3],"status":"normal","message":null},"memory":{"value":{"used_bytes":50,"total_bytes":100},"status":"warning","message":null},"disks":[],"docker":{"value":null,"status":"unknown","message":"Docker unavailable"},"containers":[],"containers_status":"unknown","containers_message":null}"#).unwrap();
        snapshot["collected_at_unix"] = serde_json::json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs());
        fs::write(
            dir.join("host.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        assert_eq!(app.status().host.status, "available");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn host_snapshot_is_fresh_then_stale_and_does_not_expose_extra_fields() {
        let dir = temp_dir("host-fresh");
        fs::write(dir.join("host.json"), VALID_HOST).unwrap();
        let fresh = load_host(&dir, 1300, 300);
        assert_eq!(fresh.status, "available");
        assert_eq!(fresh.age_seconds, Some(300));
        let payload = serde_json::to_string(&fresh).unwrap();
        assert!(payload.contains("warning"));
        assert!(!payload.contains("DO_NOT_EXPOSE"));
        let stale = load_host(&dir, 1301, 300);
        assert_eq!(stale.status, "unknown");
        assert_eq!(stale.age_seconds, Some(301));
        assert!(stale.snapshot.is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn heartbeat_age_changes_project_health_without_a_new_cli_snapshot() {
        let dir = temp_dir("service-age");
        let mut snapshot: Value = serde_json::from_str(VALID_HOST).unwrap();
        snapshot["services"] = serde_json::json!([{
            "project": "demo", "service": "worker", "status": "healthy", "kind": "heartbeat",
            "latency_ms": null, "age_seconds": 2, "heartbeat_at_unix": 1000,
            "max_age_seconds": 30, "crit_multiplier": 3, "message": null,
            "secret": "DO_NOT_EXPOSE"
        }]);
        fs::write(
            dir.join("host.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        for (now, expected, age) in [
            (1005, ServiceLevel::Healthy, 5),
            (1031, ServiceLevel::Degraded, 31),
            (1091, ServiceLevel::Unhealthy, 91),
        ] {
            let host = load_host(&dir, now, 300);
            let service = &host.snapshot.as_ref().unwrap().services[0];
            assert_eq!(service.status, expected);
            assert_eq!(service.age_seconds, Some(age));
            let mut payload = StatusPayload {
                status: "unknown".into(),
                checked_at: String::new(),
                projects: vec![ProjectStatus {
                    uses_service_probes: true,
                    ..project("unknown")
                }],
                host,
            };
            payload.projects[0].name = "demo".into();
            apply_service_snapshot(&mut payload);
            assert_eq!(payload.projects[0].services[0].status, expected);
            let json = serde_json::to_string(&payload).unwrap();
            assert!(!json.contains("DO_NOT_EXPOSE"));
            assert!(!json.contains("heartbeat_at_unix"));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_http_response_preserves_all_host_metrics() {
        let dir = temp_dir("host-api-metrics");
        let mut snapshot: Value = serde_json::from_str(VALID_HOST).unwrap();
        snapshot["collected_at_unix"] = serde_json::json!(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs());
        snapshot["load"]["value"] = serde_json::json!([1.25, 2.5, 3.75]);
        snapshot["docker"] = serde_json::json!({
            "value": {"images": "2GB", "containers": "1MB", "volumes": "3GB"},
            "status": "normal", "message": null
        });
        fs::write(dir.join("demo.toml"), "[compose]\nservices = ['api', 'worker']\n[service_health.api]\ntype = 'http'\nurl = 'http://localhost/health'\ntimeout_ms = 1000\n[service_health.worker]\ntype = 'heartbeat'\npath = '/run/beat'\nmax_age_seconds = 30\n").unwrap();
        let now = snapshot["collected_at_unix"].as_u64().unwrap();
        snapshot["services"] = serde_json::json!([
            {"project": "demo", "service": "api", "status": "healthy", "kind": "http", "latency_ms": 42, "age_seconds": null, "message": null},
            {"project": "demo", "service": "worker", "status": "degraded", "kind": "heartbeat", "latency_ms": null, "age_seconds": 40, "heartbeat_at_unix": now - 40, "max_age_seconds": 30, "crit_multiplier": 2, "message": null}
        ]);
        fs::write(
            dir.join("host.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();

        let app = App::new(Config {
            host: "127.0.0.1".into(),
            port: 0,
            projects_dir: dir.clone(),
            state_dir: dir.clone(),
            static_dir: dir.clone(),
            check_timeout: Duration::from_secs(1),
            cache_duration: Duration::from_secs(1),
            host_max_age_seconds: 300,
            read_socket: dir.join("missing.sock"),
        })
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream, &app).unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .write_all(b"GET /api/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        server.join().unwrap();

        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("HTTP/1.1 200 OK"), "{headers}");
        let payload: Value = serde_json::from_str(body).unwrap();
        assert_eq!(payload["host"]["status"], "available");
        let exposed = payload["host"]["snapshot"].as_object().unwrap();
        for key in ["cpu", "load", "memory", "disks", "docker", "containers"] {
            assert!(
                exposed.contains_key(key),
                "missing host snapshot key: {key}"
            );
        }
        assert_eq!(exposed["load"]["value"], snapshot["load"]["value"]);
        assert_eq!(exposed["docker"]["value"], snapshot["docker"]["value"]);
        assert_eq!(payload["projects"][0]["status"], "unknown");
        assert_eq!(payload["projects"][0]["services"][0]["latency_ms"], 42);
        assert!(
            payload["projects"][0]["services"][1]["age_seconds"]
                .as_u64()
                .unwrap()
                >= 40
        );
        assert_eq!(payload["projects"][0]["services"][1]["status"], "degraded");
        assert_eq!(payload["status"], "unknown");
        assert!(!exposed.contains_key("secret"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_api_rejects_mutations_and_invalid_parameters_without_contacting_executor() {
        let dir = temp_dir("read-route");
        fs::write(dir.join("demo.toml"), "[compose]\nservice = 'api'\n").unwrap();
        let app = App::new(Config {
            host: "127.0.0.1".into(),
            port: 0,
            projects_dir: dir.clone(),
            state_dir: dir.clone(),
            static_dir: dir.clone(),
            check_timeout: Duration::from_secs(1),
            cache_duration: Duration::from_secs(1),
            host_max_age_seconds: 300,
            read_socket: dir.join("missing.sock"),
        })
        .unwrap();
        for path in [
            "/api/read?op=deploy&project=demo",
            "/api/read?op=images&prune=true",
            "/api/read?op=restore-points&project=..%2Fdemo",
            "/api/read?op=logs&project=demo&tail=999999",
            "/api/read?op=logs&project=demo&since=2h%3Bwhoami",
            "/api/read?op=restore-log&project=demo&project=demo",
            "/api/read?op=images&binary=sh",
        ] {
            assert!(
                read_api_http(&app, path).starts_with("HTTP/1.1 400"),
                "{path}"
            );
        }
        assert!(read_api_http(&app, "/api/read?op=images").starts_with("HTTP/1.1 503"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_identifies_monitors_and_configured_services_without_a_deployment() {
        let dir = temp_dir("read-inventory");
        fs::write(
            dir.join("demo.toml"),
            "[compose]\nservices = ['api', 'worker']\n",
        )
        .unwrap();
        fs::write(
            dir.join("monitor.toml"),
            "[deployment]\nhealth_url = 'http://127.0.0.1:1/health'\n",
        )
        .unwrap();
        let projects = load_projects(&dir, &dir);
        let demo = serde_json::to_value(check_project(
            projects.iter().find(|p| p.name == "demo").unwrap().clone(),
            &Client::new(),
        ))
        .unwrap();
        let monitor = serde_json::to_value(check_project(
            projects.into_iter().find(|p| p.name == "monitor").unwrap(),
            &Client::builder()
                .timeout(Duration::from_millis(50))
                .build()
                .unwrap(),
        ))
        .unwrap();
        assert_eq!(
            demo["application_services"],
            serde_json::json!(["api", "worker"])
        );
        assert_eq!(demo["url_monitor"], false);
        assert_eq!(monitor["url_monitor"], true);
        assert_eq!(monitor["application_services"], serde_json::json!([]));
        assert!(
            monitor["latency_ms"].is_null(),
            "failed HTTP checks have no response latency"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_api_forwards_only_inventory_and_sanitizes_executor_output() {
        use std::os::unix::net::UnixListener;
        let dir = temp_dir("read-success");
        let socket = dir.join("read.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let app = App::new(Config {
            host: "127.0.0.1".into(),
            port: 0,
            projects_dir: dir.clone(),
            state_dir: dir.clone(),
            static_dir: dir.clone(),
            check_timeout: Duration::from_secs(1),
            cache_duration: Duration::from_secs(1),
            host_max_age_seconds: 300,
            read_socket: socket.clone(),
        })
        .unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap(),
                serde_json::json!({"op":"images"})
            );
            (&stream).write_all(b"{\"output\":\"Project: demo\\n  keep image:abc\\n\",\"secret\":\"private\"}\n").unwrap();
        });
        let response = read_api_http(&app, "/api/read?op=images");
        server.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("Project: demo"));
        assert!(!response.contains("private"));
        fs::remove_dir_all(dir).unwrap();
    }

    fn read_api_http(app: &App, path: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::scope(|scope| {
            let server = scope.spawn(|| {
                let (stream, _) = listener.accept().unwrap();
                handle_connection(stream, app).unwrap();
            });
            let mut client = TcpStream::connect(address).unwrap();
            client
                .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
                .unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            server.join().unwrap();
            response
        })
    }
}
