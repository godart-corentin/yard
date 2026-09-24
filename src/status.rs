use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::error::{Result, YardError};
use crate::host;
use crate::monitor::Monitor;
use crate::project::Project;
use crate::service_health::{Health, ServiceHealth};
use crate::state::{BackupAttempt, ProjectState, Release};

#[derive(Debug, Deserialize)]
struct ComposeService {
    #[serde(rename = "Service", default)]
    service: String,
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "State", default)]
    state: String,
    #[serde(rename = "Health", default)]
    health: String,
    #[serde(rename = "Image", default)]
    image: String,
    #[serde(rename = "ExitCode", default)]
    exit_code: Option<i32>,
}

pub fn run(project: &Project, projects_dir: &Path, state_dir: &Path) -> Result<()> {
    let state = load_state(project)?;
    let head = project.head_revision()?;
    let branch = project.current_branch()?;
    let env_tag = project.current_tag_from_env().ok().flatten();
    let snapshot = host::collect(projects_dir);
    let compose_output = project.compose_ps();
    let services = compose_output
        .as_ref()
        .ok()
        .and_then(|output| parse_compose_services(output).ok());

    println!("Project: {}", project.name);
    println!(
        "Branch:  {}",
        if branch.is_empty() {
            "(detached)"
        } else {
            &branch
        }
    );
    println!("HEAD:    {}", Project::tag_for_revision(&head));

    println!();
    println!("Release");
    print_release("current", state.current.as_ref());
    print_release("previous", state.previous.as_ref());
    print_release("pending", state.pending.as_ref());
    println!("  {:<9} {}", "env", env_tag.as_deref().unwrap_or("not set"));
    if let Some(pending) = &state.pending {
        println!(
            "  WARNING: {} release not yet active; rollback to recover",
            pending.status
        );
    }
    if let Some(current) = &state.current {
        if !current.services.is_empty() {
            let (matched, details) = Project::runtime_report_from_output(
                current,
                compose_output
                    .as_ref()
                    .map_err(|error| YardError::Config(format!("Compose unavailable: {error}")))?,
            )?;
            println!(
                "  Docker: {} — {details}",
                if matched && state.pending.is_none() {
                    "matched"
                } else {
                    "MISMATCH"
                }
            );
        }
    }

    let latest = state.pending.as_ref().or(state.current.as_ref());
    match latest {
        Some(release) => println!(
            "  Last deployment: {} ({})",
            format_deployment_timestamp(release.deployed_at_unix),
            release.status,
        ),
        None => println!("  Last deployment: none recorded"),
    }

    println!();
    println!("Backup");
    match &state.last_backup {
        Some(attempt) => print_backup("Local", attempt),
        None => println!("  No backup recorded"),
    }
    if project
        .config
        .backup
        .as_ref()
        .and_then(|backup| backup.offsite_command.as_ref())
        .is_none()
    {
        println!("  Off-site: not configured");
    } else if let Some(attempt) = &state.last_offsite {
        print_backup("Off-site", attempt);
    } else {
        println!("  Off-site: no copy recorded for the last local backup");
    }

    println!();
    println!("Docker Compose");
    if let Some(services) = &services {
        if services.is_empty() {
            println!("  (no containers)");
        } else {
            let service_width = services
                .iter()
                .map(ComposeService::display_name)
                .map(str::len)
                .max()
                .unwrap_or(7)
                .max(7);
            let status_width = services
                .iter()
                .map(ComposeService::display_status)
                .map(str::len)
                .max()
                .unwrap_or(6)
                .max(6);

            for service in services {
                println!(
                    "  {:<service_width$}  {:<status_width$}  {}",
                    service.display_name(),
                    service.display_status(),
                    service.display_image(),
                );
            }
        }
    } else {
        println!("  unavailable [Unknown]");
    }
    for service in services.as_deref().unwrap_or(&[]) {
        if let Some(reason) = drift(project, &state, service) {
            println!("  DRIFT: {reason}");
        }
    }
    println!(
        "Project service health (measured {} UTC)",
        format_timestamp(snapshot.collected_at_unix)
    );
    for name in &project.config.compose.services {
        if let Some(health) = snapshot
            .services
            .iter()
            .find(|item| item.project == project.name && item.service == *name)
        {
            println!(
                "  {}: {:?} — measured {} (heartbeat age: {}){}",
                name,
                health.status,
                format_age(UNIX_EPOCH + Duration::from_secs(snapshot.collected_at_unix)),
                health
                    .age_seconds
                    .map(|age| format!("{age} s"))
                    .unwrap_or_else(|| "n/a".into()),
                health
                    .message
                    .map(|message| format!(" — {message}"))
                    .unwrap_or_default()
            );
        } else {
            println!("  {name}: Unknown — measurement unavailable");
        }
    }
    println!(
        "Disk (repo): {}",
        project_disk(&project.config.repo)
            .map(|bytes| format!("{bytes} bytes"))
            .unwrap_or_else(|_| "unavailable".into())
    );
    println!();
    host::publish(&snapshot, state_dir);
    Ok(())
}

fn print_backup(label: &str, attempt: &BackupAttempt) {
    let destination = attempt
        .destination
        .as_deref()
        .unwrap_or("unknown destination");
    println!(
        "  {label}: {} — {} — {destination} ({} ms) [started {} UTC]",
        attempt.result.label(),
        format_age(UNIX_EPOCH + Duration::from_secs(attempt.started_at_unix)),
        attempt.duration_ms,
        format_timestamp(attempt.started_at_unix),
    );
}

fn print_release(label: &str, release: Option<&Release>) {
    match release {
        Some(release) => {
            println!(
                "  {:<9} {}  {} ({})",
                label,
                Project::tag_for_revision(&release.revision),
                release.tag,
                release.status
            );
            for service in &release.services {
                println!("    {}  {}", service.name, service.image);
            }
        }
        None => println!("  {label:<9} none"),
    }
}

fn parse_compose_services(output: &str) -> Result<Vec<ComposeService>> {
    let output = output.trim();
    if output.is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(services) = serde_json::from_str::<Vec<ComposeService>>(output) {
        return Ok(services);
    }

    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str::<ComposeService>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

impl ComposeService {
    fn display_name(&self) -> &str {
        if self.service.is_empty() {
            &self.name
        } else {
            &self.service
        }
    }

    fn display_status(&self) -> &str {
        if self.health.is_empty() {
            &self.state
        } else {
            &self.health
        }
    }

    fn display_image(&self) -> &str {
        if self.image.is_empty() {
            "-"
        } else {
            &self.image
        }
    }
}

fn format_age(modified: SystemTime) -> String {
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return "in future".to_owned();
    };
    let seconds = age.as_secs();

    match seconds {
        0..=9 => "just now".to_owned(),
        10..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

fn format_deployment_timestamp(unix: u64) -> String {
    let timestamp = format_timestamp(unix);
    if UNIX_EPOCH
        .checked_add(Duration::from_secs(unix))
        .map_or(true, |stamp| format_age(stamp) == "in future")
    {
        format!("{timestamp} UTC (in future)")
    } else {
        format!("{timestamp} UTC")
    }
}

fn load_state(project: &Project) -> Result<ProjectState> {
    ProjectState::load(&project.state_path)
        .map_err(|error| YardError::Config(format!("{}: state unreadable: {error}", project.name)))
}

fn drift(project: &Project, state: &ProjectState, service: &ComposeService) -> Option<String> {
    if !project
        .config
        .compose
        .services
        .iter()
        .any(|name| name == service.display_name())
    {
        return None;
    }
    if !service.state.eq_ignore_ascii_case("running") {
        return None;
    }
    if service.image.is_empty() {
        return Some(format!(
            "{} has an unknown running image",
            service.display_name()
        ));
    }
    let releases = [
        state.current.as_ref(),
        state.previous.as_ref(),
        state.pending.as_ref(),
    ];
    if releases.into_iter().flatten().any(|release| {
        let recorded_image = release
            .services
            .iter()
            .any(|item| item.name == service.display_name() && item.image == service.image);
        // Older releases lack per-service images. A tag alone cannot
        // identify an image: only the configured repository is a safe
        // fallback, and other repositories remain visibly uncertain.
        recorded_image
            || (release.services.is_empty()
                && service.image == format!("{}:{}", project.config.image.name, release.tag))
    }) {
        return None;
    }
    if releases.into_iter().flatten().any(|release| {
        release.services.is_empty()
            && service.image.rsplit_once(':').map(|(_, tag)| tag) == Some(release.tag.as_str())
    }) {
        Some(format!(
            "{} runs {}: cannot verify image against legacy release (configured repository {})",
            service.display_name(),
            service.image,
            project.config.image.name
        ))
    } else if releases.iter().all(Option::is_none) {
        Some(format!(
            "{} runs {} without a recorded release",
            service.display_name(),
            service.image
        ))
    } else {
        Some(format!(
            "{} runs {} outside current/previous{} release",
            service.display_name(),
            service.image,
            if state.pending.is_some() {
                "/pending"
            } else {
                ""
            }
        ))
    }
}

fn expected_finished_migration(project: &Project, service: &ComposeService) -> bool {
    project.config.deployment.migration_service.as_deref() == Some(service.display_name())
        && service.state.eq_ignore_ascii_case("exited")
        && service.exit_code == Some(0)
}

// Local checkout only: Docker volumes and remote backups have no per-project size
// in the existing data. Do not follow symlinks outside the checkout.
fn project_disk(path: &Path) -> std::io::Result<u64> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(0);
    }
    let mut bytes = meta.blocks().saturating_mul(512);
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            bytes = bytes.saturating_add(project_disk(&entry?.path())?);
        }
    }
    Ok(bytes)
}

fn format_timestamp(unix: u64) -> String {
    let Ok(seconds) = libc::time_t::try_from(unix) else {
        return unix.to_string();
    };
    let mut result = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: gmtime_r initializes result when it returns non-null.
    let stamp = unsafe { libc::gmtime_r(&seconds, result.as_mut_ptr()) };
    if stamp.is_null() {
        return unix.to_string();
    }
    // SAFETY: gmtime_r succeeded and wrote a complete tm.
    let stamp = unsafe { result.assume_init() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        stamp.tm_year + 1900,
        stamp.tm_mon + 1,
        stamp.tm_mday,
        stamp.tm_hour,
        stamp.tm_min,
        stamp.tm_sec
    )
}

pub fn overview(projects_dir: &Path, state_dir: &Path) -> Result<()> {
    let names = Project::list(projects_dir)?;
    let snapshot = host::collect(projects_dir);
    if let Err(error) = host::save(&snapshot, state_dir) {
        eprintln!("yard: cannot save host snapshot: {error}");
    }
    if names.is_empty() {
        println!("No projects configured");
    }
    for name in names {
        let path = projects_dir.join(format!("{name}.toml"));
        match Monitor::load(&path) {
            Ok(Some(monitor)) => {
                let (healthy, result) = monitor.check();
                println!(
                    "{} {name}: {result}; {}",
                    if healthy { "OK" } else { "ALERT" },
                    monitor.url()
                );
                continue;
            }
            Err(error) => {
                println!("ALERT {name}: project not inspectable ({error})");
                continue;
            }
            Ok(None) => {}
        }
        let project = match Project::load(&name, projects_dir, state_dir) {
            Ok(project) => project,
            Err(error) => {
                println!("ALERT {name}: project not inspectable ({error})");
                continue;
            }
        };
        let state = match load_state(&project) {
            Ok(state) => state,
            Err(_) => {
                println!("ALERT {name}: state unreadable");
                continue;
            }
        };
        let services = project
            .compose_ps()
            .ok()
            .and_then(|output| parse_compose_services(&output).ok());
        let drifts: Vec<_> = services
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|service| drift(&project, &state, service))
            .collect();
        let health: Vec<&ServiceHealth> = snapshot
            .services
            .iter()
            .filter(|item| item.project == name)
            .collect();
        let problem = services.as_ref().map_or(true, |rows| {
            rows.is_empty()
                || rows.iter().any(|row| {
                    !row.state.eq_ignore_ascii_case("running")
                        && !expected_finished_migration(&project, row)
                })
                || project
                    .config
                    .compose
                    .services
                    .iter()
                    .any(|name| !rows.iter().any(|row| row.display_name() == name))
        }) || health.iter().any(|item| {
            matches!(item.status, Health::Degraded | Health::Unhealthy)
                || item.message == Some("Container unavailable")
        }) || project.config.service_health.keys().any(|name| {
            health
                .iter()
                .find(|item| item.service == *name)
                .map_or(true, |item| item.status == Health::Unknown)
        }) || !drifts.is_empty()
            || state.pending.is_some();
        let version = state
            .current
            .as_ref()
            .map(|release| release.tag.as_str())
            .unwrap_or("no deployment recorded");
        let previous = state
            .previous
            .as_ref()
            .map(|release| release.tag.as_str())
            .unwrap_or("none");
        let runtime = services
            .as_ref()
            .map(|rows| {
                format!(
                    "{} running / {} containers",
                    rows.iter()
                        .filter(|row| row.state.eq_ignore_ascii_case("running"))
                        .count(),
                    rows.len()
                )
            })
            .unwrap_or_else(|| "containers unavailable".into());
        let backup = state
            .last_backup
            .as_ref()
            .map(|attempt| {
                format!(
                    "{} @ {} UTC",
                    attempt.result.label(),
                    format_timestamp(attempt.started_at_unix)
                )
            })
            .unwrap_or_else(|| "none".into());
        let offsite = state
            .last_offsite
            .as_ref()
            .map(|attempt| {
                format!(
                    "{} @ {} UTC",
                    attempt.result.label(),
                    format_timestamp(attempt.started_at_unix)
                )
            })
            .unwrap_or_else(|| "none".into());
        let deployment = state
            .pending
            .as_ref()
            .or(state.current.as_ref())
            .map(|release| {
                format!(
                    "{} @ {}",
                    release.status,
                    format_deployment_timestamp(release.deployed_at_unix)
                )
            })
            .unwrap_or_else(|| "none recorded".into());
        let disk = project_disk(&project.config.repo)
            .map(|bytes| format!("{bytes} bytes"))
            .unwrap_or_else(|_| "unavailable".into());
        let probes = health
            .iter()
            .map(|item| format!("{}={:?}", item.service, item.status))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{} {name}: {version} (previous {previous}); deployment {deployment}; {runtime}; probes {probes}; backup {backup}, off-site {offsite}; repo {disk}; {}{}",
            if problem { "ALERT" } else { "OK" },
            if state.pending.is_some() {
                "pending release; "
            } else {
                ""
            },
            if drifts.is_empty() {
                String::new()
            } else {
                format!("DRIFT: {}", drifts.join("; "))
            }
        );
    }
    Ok(())
}
