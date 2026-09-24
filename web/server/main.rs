use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
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
                4.0,
            )?),
            cache_duration: Duration::from_secs_f64(env_number("YARD_WEB_CACHE_SECONDS", 15.0)?),
            host_max_age_seconds: env_number("YARD_WEB_HOST_MAX_AGE_SECONDS", 300)?,
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
    release: Option<Value>,
    pending_release: Option<Value>,
    config_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ProjectStatus {
    name: String,
    health_url: Option<String>,
    services: Vec<ServiceHealth>,
    #[serde(skip)]
    uses_service_probes: bool,
    release: Option<Value>,
    pending_release: Option<Value>,
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
    for service in &mut snapshot.services {
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
            apply_service_snapshot(&mut value);
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
        payload
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
            let (release, pending_release) = load_releases(state_dir, &name);
            let parsed = fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|contents| {
                    toml::from_str::<ProjectFile>(&contents).map_err(|error| error.to_string())
                });

            Some(match parsed {
                Ok(config) => Project {
                    name,
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
                    health_url: config
                        .deployment
                        .and_then(|deployment| deployment.health_url)
                        .map(|url| url.trim().to_owned())
                        .filter(|url| !url.is_empty()),
                    release,
                    pending_release,
                    config_error: None,
                },
                Err(error) => Project {
                    name,
                    health_url: None,
                    has_service_probes: false,
                    service_names: vec![],
                    release,
                    pending_release,
                    config_error: Some(error),
                },
            })
        })
        .collect()
}

fn load_releases(state_dir: &Path, project: &str) -> (Option<Value>, Option<Value>) {
    // host.json is reserved for the CLI's host snapshot, never project state.
    if project == "host" {
        return (None, None);
    }
    let state = fs::read_to_string(state_dir.join(format!("{project}.json")))
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok());
    let field = |name| {
        state
            .as_ref()
            .and_then(|state| state.get(name))
            .filter(|value| value.is_object())
            .cloned()
    };
    (field("current"), field("pending"))
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
        health_url: project.health_url,
        services: project
            .service_names
            .into_iter()
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
        release: project.release,
        pending_release: project.pending_release,
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
            if (200..400).contains(&status) {
                result.status = "operational".to_owned();
            } else {
                result.status = "down".to_owned();
                result.error = Some(format!("HTTP {status}"));
            }
        }
        Err(error) => {
            result.latency_ms = Some(started.elapsed().as_millis());
            result.status = "down".to_owned();
            result.error = Some(error.to_string());
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
        if !project.uses_service_probes {
            continue;
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
    let path = parts
        .next()
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or("/");
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
            health_url: None,
            services: vec![],
            uses_service_probes: false,
            release: None,
            pending_release: None,
            config_error: None,
            checked_at: "2026-01-01T00:00:00Z".to_owned(),
            latency_ms: None,
            http_status: None,
            status: status.to_owned(),
            error: None,
        }
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
        assert_eq!(loaded[0].release.as_ref().unwrap()["tag"], "abcdef123456");
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
        fs::write(state.join("demo.json"), r#"{"current":{"tag":"old"},"pending":{"tag":"new","status":"activating","services":[{"name":"api","image":"api:new"}]}}"#).unwrap();
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
                has_service_probes: false,
                service_names: vec![],
                release: None,
                pending_release: None,
                config_error: None,
            },
            &client,
        );
        assert_eq!(checked.status, "unknown");
        assert!(checked.error.unwrap().contains("No deployment.health_url"));
    }

    #[test]
    fn config_errors_preserve_the_existing_payload_field() {
        let client = Client::builder().build().unwrap();
        let checked = check_project(
            Project {
                name: "broken".to_owned(),
                health_url: None,
                has_service_probes: false,
                service_names: vec![],
                release: None,
                pending_release: None,
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
                has_service_probes: false,
                service_names: vec![],
                release: None,
                pending_release: None,
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
        assert_eq!(load_releases(&dir, "host"), (None, None));
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
        assert_eq!(payload["projects"][0]["status"], "degraded");
        assert_eq!(payload["projects"][0]["services"][0]["latency_ms"], 42);
        assert!(
            payload["projects"][0]["services"][1]["age_seconds"]
                .as_u64()
                .unwrap()
                >= 40
        );
        assert_eq!(payload["projects"][0]["services"][1]["status"], "degraded");
        assert_eq!(payload["status"], "degraded");
        assert!(!exposed.contains_key("secret"));
        fs::remove_dir_all(dir).unwrap();
    }
}
