use std::env;
use std::ffi::CString;
use std::fs;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::command;
use crate::project::Project;

pub const SNAPSHOT_FILE: &str = "host.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Normal,
    Warning,
    Critical,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct Metric<T: Serialize> {
    pub value: Option<T>,
    pub status: Level,
    pub message: Option<&'static str>,
}

impl<T: Serialize> Metric<T> {
    fn known(value: T, status: Level) -> Self {
        Self {
            value: Some(value),
            status,
            message: None,
        }
    }

    fn unknown(message: &'static str) -> Self {
        Self {
            value: None,
            status: Level::Unknown,
            message: Some(message),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Thresholds {
    pub disk_warn_percent: u8,
    pub disk_crit_percent: u8,
    pub mem_pressure_percent: u8,
}

impl Thresholds {
    fn from_env() -> Self {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let mut read = |name: &str, default: u8| -> u8 {
            lookup(name)
                .and_then(|v| v.parse::<u8>().ok())
                .filter(|v| *v <= 100)
                .unwrap_or(default)
        };
        let warn = read("YARD_DISK_WARN_PERCENT", 80);
        let crit = read("YARD_DISK_CRIT_PERCENT", 90).max(warn);
        Self {
            disk_warn_percent: warn,
            disk_crit_percent: crit,
            mem_pressure_percent: read("YARD_MEM_PRESSURE_PERCENT", 85),
        }
    }
}

fn percent_state(percent: f64, warn: u8, crit: Option<u8>) -> Level {
    if crit.is_some_and(|crit| percent > f64::from(crit)) {
        Level::Critical
    } else if percent > f64::from(warn) {
        Level::Warning
    } else {
        Level::Normal
    }
}

#[derive(Debug, Serialize)]
pub struct Memory {
    pub used_bytes: u64,
    pub total_bytes: u64,
}
#[derive(Debug, Serialize)]
pub struct Disk {
    pub mount: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}
#[derive(Debug, Serialize)]
pub struct DockerUsage {
    pub images: String,
    pub containers: String,
    pub volumes: String,
}
#[derive(Debug, Serialize)]
pub struct Container {
    pub project: String,
    pub service: String,
    pub state: String,
    pub status: Level,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub version: u8,
    pub collected_at_unix: u64,
    pub thresholds: Thresholds,
    pub cpu: Metric<f64>,
    pub load: Metric<[f64; 3]>,
    pub memory: Metric<Memory>,
    pub disks: Vec<Metric<Disk>>,
    pub docker: Metric<DockerUsage>,
    pub containers: Vec<Container>,
    pub containers_status: Level,
    pub containers_message: Option<&'static str>,
}

fn parse_cpu(text: &str) -> Option<(u64, u64)> {
    let mut fields = text.lines().next()?.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let counters: Vec<u64> = fields.map(str::parse).collect::<Result<_, _>>().ok()?;
    if counters.len() < 4 {
        return None;
    }
    let idle = counters[3].checked_add(*counters.get(4).unwrap_or(&0))?;
    let total = counters
        .iter()
        .try_fold(0_u64, |sum, n| sum.checked_add(*n))?;
    Some((idle, total))
}

fn cpu_percent(first: (u64, u64), second: (u64, u64)) -> Option<f64> {
    let total = second.1.checked_sub(first.1)?;
    let idle = second.0.checked_sub(first.0)?;
    if total == 0 || idle > total {
        return None;
    }
    Some(100.0 * (total - idle) as f64 / total as f64)
}

fn parse_memory(text: &str) -> Option<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next()?;
        if key == "MemTotal:" || key == "MemAvailable:" {
            let kb = parts.next()?.parse::<u64>().ok()?;
            if parts.next()? != "kB" {
                return None;
            }
            let bytes = kb.checked_mul(1024)?;
            if key == "MemTotal:" {
                total = Some(bytes);
            } else {
                available = Some(bytes);
            }
        }
    }
    let total = total?;
    let available = available?;
    if total == 0 {
        return None;
    }
    Some((total.checked_sub(available)?, total))
}

fn parse_load(text: &str) -> Option<[f64; 3]> {
    let mut fields = text.split_whitespace();
    let mut load = [0.0; 3];
    for value in &mut load {
        *value = fields.next()?.parse::<f64>().ok()?;
        if !value.is_finite() || *value < 0.0 {
            return None;
        }
    }
    Some(load)
}

fn disk_usage(mount: &Path) -> Option<(u64, u64)> {
    let path = CString::new(mount.as_os_str().as_bytes()).ok()?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: statvfs writes to the allocated struct; the CString remains alive for the call.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: successful statvfs initialized the struct.
    let stats = unsafe { stats.assume_init() };
    let block_size = stats.f_frsize;
    let total = stats.f_blocks.checked_mul(block_size)?;
    // Available to an unprivileged process, rather than free including root-reserved blocks.
    let available = stats.f_bavail.checked_mul(block_size)?;
    Some((total.checked_sub(available)?, total))
}

fn disk_mounts(text: &str) -> Vec<String> {
    let mut mounts = Vec::new();
    let mut devices = std::collections::HashSet::new();
    for line in text.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let mut fields = after.split_whitespace();
        let fs_type = fields.next().unwrap_or("");
        if !matches!(fs_type, "ext4" | "xfs" | "btrfs" | "zfs" | "vfat" | "f2fs") {
            continue;
        }
        let mut info = before.split_whitespace();
        let device = info.nth(2).unwrap_or("");
        let Some(mount) = info.nth(1) else {
            continue;
        };
        if device.is_empty() || !devices.insert(device) {
            continue;
        }
        // mountinfo escapes whitespace and backslashes using octal sequences.
        let mount = mount
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        mounts.push(mount);
    }
    mounts
}

fn parse_docker_df(text: &str) -> Option<DockerUsage> {
    let mut images = None;
    let mut containers = None;
    let mut volumes = None;
    for line in text.lines() {
        let row: serde_json::Value = serde_json::from_str(line).ok()?;
        let kind = row.get("Type")?.as_str()?;
        let size = row.get("Size")?.as_str()?.to_owned();
        match kind {
            "Images" => images = Some(size),
            "Containers" => containers = Some(size),
            "Local Volumes" => volumes = Some(size),
            _ => {}
        }
    }
    Some(DockerUsage {
        images: images?,
        containers: containers?,
        volumes: volumes?,
    })
}

fn parse_containers(project: &str, text: &str) -> Option<Vec<Container>> {
    let rows: Vec<serde_json::Value> = if text.trim().is_empty() {
        vec![]
    } else if text.trim().starts_with('[') {
        serde_json::from_str(text).ok()?
    } else {
        text.lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .ok()?
    };
    rows.into_iter()
        .map(|row| {
            let service = row.get("Service")?.as_str()?.to_owned();
            let state = row.get("State")?.as_str()?.to_owned();
            let status = if state.eq_ignore_ascii_case("running") {
                Level::Normal
            } else {
                Level::Critical
            };
            Some(Container {
                project: project.to_owned(),
                service,
                state,
                status,
            })
        })
        .collect()
}

pub fn collect(projects_dir: &Path) -> Snapshot {
    let thresholds = Thresholds::from_env();
    let first = fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| parse_cpu(&s));
    thread::sleep(Duration::from_millis(100));
    let second = fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| parse_cpu(&s));
    let cpu = first
        .zip(second)
        .and_then(|(a, b)| cpu_percent(a, b))
        .map(|v| Metric::known(v, Level::Normal))
        .unwrap_or_else(|| Metric::unknown("CPU unavailable"));
    let load = fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| parse_load(&s))
        .map(|v| Metric::known(v, Level::Normal))
        .unwrap_or_else(|| Metric::unknown("Load unavailable"));
    let memory = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| parse_memory(&s))
        .map(|(used_bytes, total_bytes)| {
            Metric::known(
                Memory {
                    used_bytes,
                    total_bytes,
                },
                percent_state(
                    100.0 * used_bytes as f64 / total_bytes as f64,
                    thresholds.mem_pressure_percent,
                    None,
                ),
            )
        })
        .unwrap_or_else(|| Metric::unknown("Memory unavailable"));
    let disks = fs::read_to_string("/proc/self/mountinfo")
        .ok()
        .map(|s| {
            disk_mounts(&s)
                .into_iter()
                .map(|mount| {
                    disk_usage(Path::new(&mount))
                        .map(|(used_bytes, total_bytes)| {
                            let status = if total_bytes == 0 {
                                Level::Unknown
                            } else {
                                percent_state(
                                    100.0 * used_bytes as f64 / total_bytes as f64,
                                    thresholds.disk_warn_percent,
                                    Some(thresholds.disk_crit_percent),
                                )
                            };
                            Metric::known(
                                Disk {
                                    mount,
                                    used_bytes,
                                    total_bytes,
                                },
                                status,
                            )
                        })
                        .unwrap_or_else(|| Metric::unknown("Disk unavailable"))
                })
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![Metric::unknown("Disk unavailable")]);
    let docker = command::checked(
        "docker",
        &[
            "system".into(),
            "df".into(),
            "--format".into(),
            "{{json .}}".into(),
        ],
        None,
        &[],
    )
    .ok()
    .and_then(|s| parse_docker_df(&s))
    .map(|v| Metric::known(v, Level::Normal))
    .unwrap_or_else(|| Metric::unknown("Docker unavailable"));
    let mut containers = Vec::new();
    let mut containers_status = Level::Normal;
    let mut containers_message = None;
    match Project::list(projects_dir) {
        Ok(names) => {
            for name in names {
                match Project::load(&name, projects_dir, Path::new("/"))
                    .ok()
                    .and_then(|p| p.compose_ps().ok())
                    .and_then(|s| parse_containers(&name, &s))
                {
                    Some(found) if !found.is_empty() => {
                        if found.iter().any(|c| c.status == Level::Critical) {
                            containers_status = Level::Critical;
                        }
                        containers.extend(found);
                    }
                    Some(_) => {
                        containers_status = Level::Critical;
                        containers_message = Some("No containers");
                    }
                    None => {
                        if containers_status != Level::Critical {
                            containers_status = Level::Unknown;
                        }
                        containers_message = Some("Containers unavailable");
                    }
                }
            }
        }
        Err(_) => {
            containers_status = Level::Unknown;
            containers_message = Some("Containers unavailable");
        }
    }
    Snapshot {
        version: 1,
        collected_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        thresholds,
        cpu,
        load,
        memory,
        disks,
        docker,
        containers,
        containers_status,
        containers_message,
    }
}

pub fn save(snapshot: &Snapshot, state_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(state_dir)?;
    let path = state_dir.join(SNAPSHOT_FILE);
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(snapshot)?;
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)
}

fn size(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
}

pub fn render(snapshot: &Snapshot) -> String {
    let mut lines = vec!["HOST".to_owned()];
    let mut metric = |label: &str, value: String, level: Level| {
        lines.push(format!("  {label:<12} {value} [{level:?}]"));
    };
    metric(
        "CPU",
        snapshot
            .cpu
            .value
            .map(|v| format!("{v:.1}%"))
            .unwrap_or_else(|| "unavailable".into()),
        snapshot.cpu.status,
    );
    metric(
        "Load",
        snapshot
            .load
            .value
            .map(|v| format!("{:.2} / {:.2} / {:.2}", v[0], v[1], v[2]))
            .unwrap_or_else(|| "unavailable".into()),
        snapshot.load.status,
    );
    metric(
        "RAM",
        snapshot
            .memory
            .value
            .as_ref()
            .map(|v| format!("{} / {}", size(v.used_bytes), size(v.total_bytes)))
            .unwrap_or_else(|| "unavailable".into()),
        snapshot.memory.status,
    );
    for disk in &snapshot.disks {
        metric(
            "Disk",
            disk.value
                .as_ref()
                .map(|v| {
                    format!(
                        "{}: {} / {}",
                        v.mount,
                        size(v.used_bytes),
                        size(v.total_bytes)
                    )
                })
                .unwrap_or_else(|| "unavailable".into()),
            disk.status,
        );
    }
    metric(
        "Docker",
        snapshot
            .docker
            .value
            .as_ref()
            .map(|v| {
                format!(
                    "images {}, containers {}, volumes {}",
                    v.images, v.containers, v.volumes
                )
            })
            .unwrap_or_else(|| "unavailable".into()),
        snapshot.docker.status,
    );
    for container in &snapshot.containers {
        metric(
            "Container",
            format!(
                "{} / {}: {}",
                container.project, container.service, container.state
            ),
            container.status,
        );
    }
    if let Some(message) = snapshot.containers_message {
        metric("Containers", message.into(), snapshot.containers_status);
    }
    lines.join("\n")
}

pub fn run(projects_dir: &Path, state_dir: &Path) {
    let snapshot = collect(projects_dir);
    if save(&snapshot, state_dir).is_err() {
        eprintln!("yard: host snapshot unavailable");
    }
    println!("{}", render(&snapshot));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_counters_and_computes_busy_delta() {
        let first = parse_cpu("cpu  100 20 30 850 10 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        let second = parse_cpu("cpu  130 20 50 900 10 0 0 0 0 0\n").unwrap();
        assert!((cpu_percent(first, second).unwrap() - 50.0).abs() < 0.01);
        assert!(cpu_percent(second, first).is_none());
        assert!(parse_cpu("not cpu").is_none());
    }

    #[test]
    fn parses_available_memory_and_load() {
        assert_eq!(
            parse_memory("MemTotal: 1000 kB\nMemFree: 50 kB\nMemAvailable: 400 kB\n"),
            Some((600 * 1024, 1000 * 1024))
        );
        assert_eq!(parse_memory("MemTotal: 1000 kB\n"), None);
        assert_eq!(
            parse_load("0.12 1.20 2.30 1/200 42"),
            Some([0.12, 1.2, 2.3])
        );
        assert_eq!(parse_load("garbage"), None);
    }

    #[test]
    fn thresholds_are_strict_and_critical_wins() {
        let thresholds = Thresholds {
            disk_warn_percent: 80,
            disk_crit_percent: 90,
            mem_pressure_percent: 85,
        };
        assert_eq!(percent_state(80.0, 80, Some(90)), Level::Normal);
        assert_eq!(percent_state(80.1, 80, Some(90)), Level::Warning);
        assert_eq!(percent_state(90.0, 80, Some(90)), Level::Warning);
        assert_eq!(percent_state(90.1, 80, Some(90)), Level::Critical);
        assert_eq!(
            percent_state(86.0, thresholds.mem_pressure_percent, None),
            Level::Warning
        );
    }

    #[test]
    fn environment_threshold_overrides_are_validated_without_machine_state() {
        let thresholds = Thresholds::from_lookup(|key| match key {
            "YARD_DISK_WARN_PERCENT" => Some("65".into()),
            "YARD_DISK_CRIT_PERCENT" => Some("75".into()),
            "YARD_MEM_PRESSURE_PERCENT" => Some("120".into()),
            _ => None,
        });
        assert_eq!(thresholds.disk_warn_percent, 65);
        assert_eq!(thresholds.disk_crit_percent, 75);
        assert_eq!(thresholds.mem_pressure_percent, 85);
        assert_eq!(
            percent_state(
                76.0,
                thresholds.disk_warn_percent,
                Some(thresholds.disk_crit_percent)
            ),
            Level::Critical
        );
    }

    #[test]
    fn disk_mounts_exclude_duplicate_bind_mounts_and_pseudo_filesystems() {
        let mounts = "1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 8:1 /var/data /mnt/data rw - ext4 /dev/sda1 rw\n3 1 8:2 / /backup rw - xfs /dev/sdb1 rw\n4 1 0:1 / /proc rw - proc proc rw\n";
        assert_eq!(disk_mounts(mounts), vec!["/", "/backup"]);
    }
    #[test]
    fn renders_host_without_reading_the_machine() {
        let snapshot = Snapshot {
            version: 1,
            collected_at_unix: 42,
            thresholds: Thresholds {
                disk_warn_percent: 80,
                disk_crit_percent: 90,
                mem_pressure_percent: 85,
            },
            cpu: Metric::known(45.0, Level::Normal),
            load: Metric::known([1.0, 2.0, 3.0], Level::Normal),
            memory: Metric::known(
                Memory {
                    used_bytes: 1_073_741_824,
                    total_bytes: 2_147_483_648,
                },
                Level::Warning,
            ),
            disks: vec![Metric::known(
                Disk {
                    mount: "/".into(),
                    used_bytes: 1_073_741_824,
                    total_bytes: 2_147_483_648,
                },
                Level::Critical,
            )],
            docker: Metric::unknown("Docker unavailable"),
            containers: vec![],
            containers_status: Level::Unknown,
            containers_message: Some("Containers unavailable"),
        };
        let text = render(&snapshot);
        assert!(text.starts_with("HOST\n"));
        assert!(text.contains("CPU          45.0% [Normal]"));
        assert!(text.contains("RAM          1.0 GiB / 2.0 GiB [Warning]"));
        assert!(text.contains("Disk         /: 1.0 GiB / 2.0 GiB [Critical]"));
        assert!(text.contains("Docker       unavailable [Unknown]"));
    }

    #[test]
    fn parses_docker_system_df_and_stopped_containers() {
        let df = "{\"Type\":\"Images\",\"Size\":\"2.4GB\"}\n{\"Type\":\"Containers\",\"Size\":\"12MB\"}\n{\"Type\":\"Local Volumes\",\"Size\":\"4GB\"}";
        let usage = parse_docker_df(df).unwrap();
        assert_eq!(usage.images, "2.4GB");
        assert_eq!(usage.volumes, "4GB");
        assert_eq!(
            parse_containers("hello", "{\"Service\":\"api\",\"State\":\"exited\"}").unwrap()[0]
                .status,
            Level::Critical
        );
        assert!(parse_docker_df("not JSON").is_none());
    }
}
