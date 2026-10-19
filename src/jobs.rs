//! Job metadata store of the ops executor (see docs/web-jobs.md, "Job storage").
//!
//! `<state>/jobs/<id>.json` is rewritten atomically; `<id>.log` is append-only and owned by the
//! supervisor of the job; `<id>.pid` (executor private) records the process group of a live job
//! so a restarted executor can tell a vanished job from one that is still running.
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic_file;
use crate::error::{Result, YardError};

pub const RETENTION: Duration = Duration::from_secs(90 * 24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Deploy,
    Backup,
    Restore,
    ImagesPrune,
    HostCollect,
}

impl Action {
    pub fn is_global(self) -> bool {
        matches!(self, Self::ImagesPrune | Self::HostCollect)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl State {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

/// The wire `Job` of the contract.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub action: Action,
    pub project: Option<String>,
    pub revision: Option<String>,
    pub state: State,
    pub actor: String,
    pub created_at_unix: u64,
    pub started_at_unix: Option<u64>,
    pub finished_at_unix: Option<u64>,
    pub exit_code: Option<i32>,
    pub output_bytes: u64,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    v: u8,
    #[serde(flatten)]
    job: Job,
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn jobs_dir(state: &Path) -> PathBuf {
    state.join("jobs")
}

/// Create the directory `0755` whatever the umask, so the read-only state mount can list it.
pub fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn civil(days: u64) -> (u64, u64, u64) {
    // Howard Hinnant's days-to-civil algorithm (proleptic Gregorian, UTC).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

pub fn id_prefix(unix: u64) -> String {
    let (year, month, day) = civil(unix / 86_400);
    let rest = unix % 86_400;
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

pub fn new_id(unix: u64) -> Result<String> {
    let mut nonce = [0_u8; 4];
    fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let suffix: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("{}-{suffix}", id_prefix(unix)))
}

pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 25
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'Z'
        && bytes[16] == b'-'
        && bytes[17..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

pub fn json_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

pub fn log_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.log"))
}

pub fn pid_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.pid"))
}

pub fn write(dir: &Path, job: &Job) -> Result<()> {
    let stored = Stored {
        v: 1,
        job: job.clone(),
    };
    let bytes = serde_json::to_vec(&stored)?;
    atomic_file::write_with_mode(&json_path(dir, &job.id), &bytes, 0o644)
}

pub fn read(dir: &Path, id: &str) -> Option<Job> {
    if !valid_id(id) {
        return None;
    }
    let bytes = fs::read(json_path(dir, id)).ok()?;
    serde_json::from_slice::<Stored>(&bytes)
        .ok()
        .map(|stored| stored.job)
}

/// Create the log before the process exists. `create_new` also detects an id collision.
pub fn create_log(dir: &Path, id: &str) -> Result<fs::File> {
    let path = log_path(dir, id);
    let file = OpenOptions::new()
        .create_new(true)
        .append(true)
        .mode(0o644)
        .open(&path)?;
    // The service umask would otherwise strip the group/other read bits.
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    Ok(file)
}

pub fn log_len(dir: &Path, id: &str) -> u64 {
    fs::metadata(log_path(dir, id))
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

pub fn write_pgid(dir: &Path, id: &str, pgid: i32) -> Result<()> {
    atomic_file::write(&pid_path(dir, id), pgid.to_string().as_bytes())
}

fn read_pgid(dir: &Path, id: &str) -> Option<i32> {
    fs::read_to_string(pid_path(dir, id))
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|pgid| *pgid > 1)
}

pub fn remove_pid(dir: &Path, id: &str) {
    let _ = fs::remove_file(pid_path(dir, id));
}

pub fn group_alive(pgid: i32) -> bool {
    // EPERM still means the group exists.
    unsafe { libc::kill(-pgid, 0) == 0 || *libc::__errno_location() == libc::EPERM }
}

fn ids(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".json")?;
            valid_id(id).then(|| id.to_owned())
        })
        .collect();
    found.sort();
    found
}

/// A job that was `queued`/`running` when the previous executor died.
pub struct Unfinished {
    pub job: Job,
    /// Present when the process group still exists: the job is adopted, not failed.
    pub pgid: Option<i32>,
}

/// Mark every unfinished job whose process group is gone as failed; report the survivors.
pub fn sweep_orphans(dir: &Path) -> Result<Vec<Unfinished>> {
    let mut alive = Vec::new();
    for id in ids(dir) {
        let Some(mut job) = read(dir, &id) else {
            continue;
        };
        if job.state.is_terminal() {
            remove_pid(dir, &id);
            continue;
        }
        if let Some(pgid) = read_pgid(dir, &id).filter(|pgid| group_alive(*pgid)) {
            alive.push(Unfinished {
                job,
                pgid: Some(pgid),
            });
            continue;
        }
        job.state = State::Failed;
        job.error = Some("Executor restarted".into());
        job.finished_at_unix = Some(now_unix());
        job.output_bytes = log_len(dir, &id);
        write(dir, &job)?;
        remove_pid(dir, &id);
    }
    Ok(alive)
}

/// Delete terminal jobs finished before `now - max_age`. Unfinished jobs are never removed.
pub fn retain(dir: &Path, now: u64, max_age: Duration) -> Result<usize> {
    let mut removed = 0;
    for id in ids(dir) {
        let Some(job) = read(dir, &id) else { continue };
        let Some(finished) = job.finished_at_unix.filter(|_| job.state.is_terminal()) else {
            continue;
        };
        if now.saturating_sub(finished) > max_age.as_secs() {
            // Remove the metadata first: a half-deleted job must never look live.
            fs::remove_file(json_path(dir, &id))
                .map_err(|error| YardError::Config(format!("cannot remove job {id}: {error}")))?;
            let _ = fs::remove_file(log_path(dir, &id));
            remove_pid(dir, &id);
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("jobs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        ensure_dir(&dir).unwrap();
        dir
    }

    fn job(id: &str, state: State, finished: Option<u64>) -> Job {
        Job {
            id: id.into(),
            action: Action::Backup,
            project: Some("demo".into()),
            revision: None,
            state,
            actor: "tester".into(),
            created_at_unix: 1,
            started_at_unix: None,
            finished_at_unix: finished,
            exit_code: None,
            output_bytes: 0,
            error: None,
        }
    }

    #[test]
    fn ids_are_utc_sortable_and_strictly_validated() {
        assert_eq!(id_prefix(0), "19700101T000000Z");
        assert_eq!(id_prefix(1_792_420_000), "20261019T142640Z");
        assert_eq!(id_prefix(951_782_400), "20000229T000000Z");
        let id = new_id(1_792_420_000).unwrap();
        assert!(valid_id(&id), "{id}");
        assert!(valid_id("20261019T142501Z-3fa9c01b"));
        for bad in [
            "",
            "../20261019T142501Z-3fa9c01b",
            "20261019T142501Z-3FA9C01B",
            "20261019T142501Z-3fa9c01",
            "20261019T142501Z-3fa9c01b.json",
        ] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn metadata_is_world_readable_and_wire_job_has_no_version_field_inside() {
        let dir = temp("mode");
        let item = job("20261019T142501Z-3fa9c01b", State::Queued, None);
        write(&dir, &item).unwrap();
        let path = json_path(&dir, &item.id);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let raw: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["v"], 1);
        assert_eq!(read(&dir, &item.id).unwrap().id, item.id);
        assert!(serde_json::to_value(&item).unwrap().get("v").is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sweep_fails_vanished_jobs_and_keeps_live_groups() {
        let dir = temp("sweep");
        for (id, state) in [
            ("20261019T000001Z-00000001", State::Running),
            ("20261019T000002Z-00000002", State::Queued),
            ("20261019T000003Z-00000003", State::Succeeded),
            ("20261019T000004Z-00000004", State::Running),
        ] {
            write(&dir, &job(id, state, None)).unwrap();
        }
        fs::write(log_path(&dir, "20261019T000001Z-00000001"), "partial\n").unwrap();
        // A group that cannot exist (pid above pid_max) is "gone".
        write_pgid(&dir, "20261019T000001Z-00000001", 4_000_000).unwrap();
        // This test's own group is alive.
        let own = unsafe { libc::getpgrp() };
        write_pgid(&dir, "20261019T000004Z-00000004", own).unwrap();
        let alive = sweep_orphans(&dir).unwrap();
        assert_eq!(alive.len(), 1);
        assert_eq!(alive[0].job.id, "20261019T000004Z-00000004");
        let failed = read(&dir, "20261019T000001Z-00000001").unwrap();
        assert_eq!(failed.state, State::Failed);
        assert_eq!(failed.error.as_deref(), Some("Executor restarted"));
        assert!(failed.finished_at_unix.is_some());
        assert_eq!(failed.output_bytes, 8);
        assert_eq!(
            read(&dir, "20261019T000002Z-00000002").unwrap().state,
            State::Failed
        );
        assert_eq!(
            read(&dir, "20261019T000003Z-00000003").unwrap().state,
            State::Succeeded
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retention_deletes_only_old_terminal_jobs() {
        let dir = temp("retain");
        let now = 1_000_000_000;
        let old = now - RETENTION.as_secs() - 1;
        let fresh = now - RETENTION.as_secs() + 60;
        for (id, state, finished) in [
            ("20260101T000000Z-0000000a", State::Failed, Some(old)),
            ("20260101T000000Z-0000000b", State::Succeeded, Some(fresh)),
            ("20260101T000000Z-0000000c", State::Running, None),
        ] {
            write(&dir, &job(id, state, finished)).unwrap();
            fs::write(log_path(&dir, id), "x").unwrap();
        }
        assert_eq!(retain(&dir, now, RETENTION).unwrap(), 1);
        assert!(!json_path(&dir, "20260101T000000Z-0000000a").exists());
        assert!(!log_path(&dir, "20260101T000000Z-0000000a").exists());
        assert!(json_path(&dir, "20260101T000000Z-0000000b").exists());
        assert!(log_path(&dir, "20260101T000000Z-0000000c").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
