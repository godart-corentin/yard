//! Job metadata store of the ops executor (see docs/web-jobs.md, "Job storage").
//!
//! `<state>/jobs/<id>.json` is rewritten atomically; `<id>.log` is append-only and owned by the
//! supervisor of the job; `<id>.pid` (executor private) records the process group of a live job
//! so a restarted executor can tell a vanished job from one that is still running.
//!
//! The directory is `<deploy user>:<executor group>` `0750` and the files `0640`: only the executor and
//! the group of the Web container (which mounts the state read-only) can read job output.
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic_file;
use crate::error::Result;

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

/// Files readable by the owner and the group of the executor (the Web container's group).
const FILE_MODE: u32 = 0o640;
/// No setgid bit: the unit's `RestrictSUIDSGID=yes` forbids setting it (EPERM), and the executor
/// already runs with the group as its effective gid, so new files get it anyway.
const DIR_MODE: u32 = 0o750;

fn egid() -> u32 {
    unsafe { libc::getegid() }
}

fn chgrp(path: &Path, gid: u32) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.gid() == gid {
        return Ok(());
    }
    std::os::unix::fs::lchown(path, None, Some(gid))
}

/// Create the directory `0750` with the executor's group whatever the umask, and bring files of
/// an older layout (`0644`, group root) to `0640` in that group. The read-only state mount of the
/// Web container reaches the files through its supplementary group, never as "other".
pub fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let gid = egid();
    chgrp(dir, gid)?;
    if fs::metadata(dir)?.permissions().mode() & 0o7777 != DIR_MODE {
        fs::set_permissions(dir, fs::Permissions::from_mode(DIR_MODE))?;
    }
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let public = path
            .extension()
            .is_some_and(|ext| ext == "json" || ext == "log");
        if public && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = chgrp(&path, gid);
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(FILE_MODE));
        }
    }
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
    atomic_file::write_with_mode(&json_path(dir, &job.id), &bytes, FILE_MODE)
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
        .mode(FILE_MODE)
        .open(&path)?;
    // The service umask would otherwise strip the group read bit.
    file.set_permissions(fs::Permissions::from_mode(FILE_MODE))?;
    Ok(file)
}

pub fn log_len(dir: &Path, id: &str) -> u64 {
    fs::metadata(log_path(dir, id))
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Start time (clock ticks since boot, field 22 of `/proc/<pid>/stat`) of a process.
fn start_time(pid: i32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may contain spaces and parentheses: fields start after the last ')'.
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    rest.split_whitespace().nth(19)?.parse().ok()
}

fn boot_id() -> Option<String> {
    let id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// `<pgid> <start time of the leader> <boot id>`. A pid file that survives a reboot (or a pid
/// wrap-around) must not be mistaken for a live job of the same number.
pub fn write_pgid(dir: &Path, id: &str, pgid: i32) -> Result<()> {
    let line = format!(
        "{pgid} {} {}",
        start_time(pgid).unwrap_or(0),
        boot_id().unwrap_or_default()
    );
    atomic_file::write(&pid_path(dir, id), line.as_bytes())
}

/// The recorded group, only if it still is the group this job started: same boot and, while the
/// leader exists, the same process (start time). A bare number is the legacy format and is only
/// checked for existence.
fn read_pgid(dir: &Path, id: &str) -> Option<i32> {
    let content = fs::read_to_string(pid_path(dir, id)).ok()?;
    let mut fields = content.split_whitespace();
    let pgid: i32 = fields.next()?.parse().ok().filter(|pgid| *pgid > 1)?;
    let (Some(started), Some(boot)) = (fields.next(), fields.next()) else {
        return Some(pgid);
    };
    if boot_id().as_deref().is_some_and(|current| current != boot) {
        return None;
    }
    let started: u64 = started.parse().ok()?;
    match (start_time(pgid), started) {
        (Some(now), recorded) if recorded != 0 && now != recorded => None,
        _ => Some(pgid),
    }
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

/// Outcome of a cleanup pass: it never stops at the first failure.
#[derive(Debug, Default)]
pub struct Pruned {
    pub removed: usize,
    pub errors: Vec<String>,
}

fn remove_job(dir: &Path, id: &str) -> std::result::Result<(), String> {
    // Remove the metadata first: a half-deleted job must never look live.
    match fs::remove_file(json_path(dir, id)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot remove job {id}: {error}")),
    }
    let log = match fs::remove_file(log_path(dir, id)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            Err(format!("cannot remove log of job {id}: {error}"))
        }
        _ => Ok(()),
    };
    remove_pid(dir, id);
    log
}

/// Delete terminal jobs finished before `now - max_age`. Unfinished jobs are never removed.
pub fn retain(dir: &Path, now: u64, max_age: Duration) -> Pruned {
    let mut pruned = Pruned::default();
    for id in ids(dir) {
        let Some(job) = read(dir, &id) else { continue };
        let Some(finished) = job.finished_at_unix.filter(|_| job.state.is_terminal()) else {
            continue;
        };
        if now.saturating_sub(finished) > max_age.as_secs() {
            match remove_job(dir, &id) {
                Ok(()) => pruned.removed += 1,
                Err(error) => pruned.errors.push(error),
            }
        }
    }
    pruned
}

/// Storage caps. `0` disables a cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_jobs: usize,
    pub max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_jobs: 500,
            max_bytes: 512 * 1024 * 1024,
        }
    }
}

/// Delete the oldest finished jobs until both caps hold. Queued and running jobs are never
/// deleted, even if they alone exceed a cap.
pub fn enforce_limits(dir: &Path, limits: Limits) -> Pruned {
    let mut pruned = Pruned::default();
    let mut total_jobs = 0_usize;
    let mut total_bytes = 0_u64;
    let mut finished: Vec<(u64, String, u64)> = Vec::new();
    for id in ids(dir) {
        let size = [json_path(dir, &id), log_path(dir, &id)]
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum::<u64>();
        total_jobs += 1;
        total_bytes += size;
        if let Some(job) = read(dir, &id) {
            if let Some(at) = job.finished_at_unix.filter(|_| job.state.is_terminal()) {
                finished.push((at, id, size));
            }
        }
    }
    // Oldest first; ids start with their creation time, which breaks ties.
    finished.sort();
    for (_, id, size) in finished {
        let over_jobs = limits.max_jobs > 0 && total_jobs > limits.max_jobs;
        let over_bytes = limits.max_bytes > 0 && total_bytes > limits.max_bytes;
        if !over_jobs && !over_bytes {
            break;
        }
        match remove_job(dir, &id) {
            Ok(()) => {
                pruned.removed += 1;
                total_jobs -= 1;
                total_bytes = total_bytes.saturating_sub(size);
            }
            Err(error) => pruned.errors.push(error),
        }
    }
    pruned
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

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
    fn metadata_is_group_readable_only_and_wire_job_has_no_version_field_inside() {
        let dir = temp("mode");
        let item = job("20261019T142501Z-3fa9c01b", State::Queued, None);
        write(&dir, &item).unwrap();
        let path = json_path(&dir, &item.id);
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
        assert_eq!(metadata.gid(), unsafe { libc::getegid() });
        let log = create_log(&dir, &item.id).unwrap();
        assert_eq!(log.metadata().unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o7777,
            0o750
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
        let pruned = retain(&dir, now, RETENTION);
        assert_eq!((pruned.removed, pruned.errors.len()), (1, 0));
        assert!(!json_path(&dir, "20260101T000000Z-0000000a").exists());
        assert!(!log_path(&dir, "20260101T000000Z-0000000a").exists());
        assert!(json_path(&dir, "20260101T000000Z-0000000b").exists());
        assert!(log_path(&dir, "20260101T000000Z-0000000c").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ensure_dir_migrates_files_of_the_older_layout() {
        let dir = temp("migrate");
        let id = "20260101T000000Z-00000001";
        write(&dir, &job(id, State::Succeeded, Some(1))).unwrap();
        fs::write(log_path(&dir, id), "x").unwrap();
        for path in [json_path(&dir, id), log_path(&dir, id)] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        ensure_dir(&dir).unwrap();
        for path in [json_path(&dir, id), log_path(&dir, id)] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pid_files_from_another_boot_or_a_recycled_pid_are_not_adopted() {
        let dir = temp("pidfile");
        let id = "20261019T000001Z-00000001";
        let own = unsafe { libc::getpgrp() };
        write_pgid(&dir, id, own).unwrap();
        assert_eq!(read_pgid(&dir, id), Some(own));
        let fresh = fs::read_to_string(pid_path(&dir, id)).unwrap();
        let mut fields = fresh.split_whitespace();
        let (started, boot) = (
            fields.nth(1).unwrap().parse::<u64>().unwrap(),
            fields.next().unwrap().to_owned(),
        );
        assert!(started > 0 && !boot.is_empty(), "{fresh}");
        // Same pid, but it started at another time: the number was recycled.
        fs::write(pid_path(&dir, id), format!("{own} {} {boot}", started + 1)).unwrap();
        assert_eq!(read_pgid(&dir, id), None);
        // Same number and start time recorded before a reboot.
        fs::write(
            pid_path(&dir, id),
            format!("{own} {started} 00000000-0000-0000-0000-000000000000"),
        )
        .unwrap();
        assert_eq!(read_pgid(&dir, id), None);
        // The legacy bare-number format is still accepted.
        fs::write(pid_path(&dir, id), own.to_string()).unwrap();
        assert_eq!(read_pgid(&dir, id), Some(own));
        // A stale file makes the sweep fail the job instead of adopting it.
        write(&dir, &job(id, State::Running, None)).unwrap();
        fs::write(pid_path(&dir, id), format!("{own} {} {boot}", started + 1)).unwrap();
        assert!(sweep_orphans(&dir).unwrap().is_empty());
        assert_eq!(read(&dir, id).unwrap().state, State::Failed);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retention_continues_past_a_failed_delete_and_reports_it() {
        let dir = temp("retain-fail");
        let now = 1_000_000_000;
        let old = now - RETENTION.as_secs() - 1;
        for id in ["20260101T000000Z-0000000a", "20260101T000000Z-0000000b"] {
            write(&dir, &job(id, State::Failed, Some(old))).unwrap();
        }
        // A non-empty directory where the log should be makes its removal fail.
        let blocker = log_path(&dir, "20260101T000000Z-0000000a");
        fs::create_dir(&blocker).unwrap();
        fs::write(blocker.join("keep"), "x").unwrap();
        let pruned = retain(&dir, now, RETENTION);
        assert_eq!(pruned.removed, 1);
        assert_eq!(pruned.errors.len(), 1, "{pruned:?}");
        assert!(pruned.errors[0].contains("0000000a"));
        assert!(!json_path(&dir, "20260101T000000Z-0000000b").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn storage_caps_prune_oldest_finished_jobs_and_never_running_ones() {
        let dir = temp("limits");
        let ids = [
            ("20260101T000001Z-00000001", State::Running, None),
            ("20260101T000002Z-00000002", State::Succeeded, Some(20)),
            ("20260101T000003Z-00000003", State::Failed, Some(10)),
            ("20260101T000004Z-00000004", State::Succeeded, Some(30)),
        ];
        for (id, state, finished) in ids {
            write(&dir, &job(id, state, finished)).unwrap();
            fs::write(log_path(&dir, id), vec![b'x'; 1000]).unwrap();
        }
        let no_caps = Limits {
            max_jobs: 0,
            max_bytes: 0,
        };
        assert_eq!(enforce_limits(&dir, no_caps).removed, 0);
        // Count cap: oldest by finish time first (10, then 20).
        let pruned = enforce_limits(
            &dir,
            Limits {
                max_jobs: 2,
                max_bytes: 0,
            },
        );
        assert_eq!(pruned.removed, 2);
        assert!(!json_path(&dir, "20260101T000003Z-00000003").exists());
        assert!(!json_path(&dir, "20260101T000002Z-00000002").exists());
        assert!(json_path(&dir, "20260101T000004Z-00000004").exists());
        // Byte cap smaller than everything: only the running job survives.
        let pruned = enforce_limits(
            &dir,
            Limits {
                max_jobs: 0,
                max_bytes: 10,
            },
        );
        assert_eq!(pruned.removed, 1);
        assert!(json_path(&dir, "20260101T000001Z-00000001").exists());
        assert!(log_path(&dir, "20260101T000001Z-00000001").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
