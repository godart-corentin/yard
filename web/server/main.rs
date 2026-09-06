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
}

#[derive(Debug, Deserialize)]
struct Deployment {
    health_url: Option<String>,
}

#[derive(Clone)]
struct Project {
    name: String,
    health_url: Option<String>,
    release: Option<Value>,
    config_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ProjectStatus {
    name: String,
    health_url: Option<String>,
    release: Option<Value>,
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
        if let Some(value) = self.cached_status() {
            return value;
        }

        let projects = load_projects(&self.config.projects_dir, &self.config.state_dir);
        let checked = check_projects(projects, &self.client);
        let payload = StatusPayload {
            status: overall_status(&checked).to_owned(),
            checked_at: utc_now(),
            projects: checked,
        };

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
            let release = load_release(state_dir, &name);
            let parsed = fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|contents| {
                    toml::from_str::<ProjectFile>(&contents).map_err(|error| error.to_string())
                });

            Some(match parsed {
                Ok(config) => Project {
                    name,
                    health_url: config
                        .deployment
                        .and_then(|deployment| deployment.health_url)
                        .map(|url| url.trim().to_owned())
                        .filter(|url| !url.is_empty()),
                    release,
                    config_error: None,
                },
                Err(error) => Project {
                    name,
                    health_url: None,
                    release,
                    config_error: Some(error),
                },
            })
        })
        .collect()
}

fn load_release(state_dir: &Path, project: &str) -> Option<Value> {
    let contents = fs::read_to_string(state_dir.join(format!("{project}.json"))).ok()?;
    let state: Value = serde_json::from_str(&contents).ok()?;
    state
        .get("current")
        .filter(|value| value.is_object())
        .cloned()
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
        name: project.name,
        health_url: project.health_url,
        release: project.release,
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

fn overall_status(projects: &[ProjectStatus]) -> &'static str {
    if projects.is_empty() {
        return "unknown";
    }
    let operational = projects
        .iter()
        .filter(|project| project.status == "operational")
        .count();
    let down = projects
        .iter()
        .filter(|project| project.status == "down")
        .count();
    if operational == projects.len() {
        "operational"
    } else if operational == 0 && down > 0 {
        "down"
    } else if down > 0 {
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

    fn project(status: &str) -> ProjectStatus {
        ProjectStatus {
            name: "test".to_owned(),
            health_url: None,
            release: None,
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
    fn project_without_health_url_is_unknown() {
        let client = Client::builder().build().unwrap();
        let checked = check_project(
            Project {
                name: "hello".to_owned(),
                health_url: None,
                release: None,
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
                release: None,
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
                release: None,
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
}
