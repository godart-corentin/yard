//! Host executor of the Yard Web actions (docs/web-jobs.md). Web may only ask for a closed set of
//! actions; argv comes from fixed templates, never from the request, and no shell is involved.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Result, YardError};
use crate::jobs::{self, Action, Job, State};
use crate::monitor::Monitor;
use crate::project::Project;
use crate::read_server;
use crate::state::ProjectState;

const MAX_REQUEST: u64 = 4096;
const MAX_CONNECTIONS: usize = 32;
const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

const JOBS_MAX_COUNT: &str = "YARD_JOBS_MAX_COUNT";
const JOBS_MAX_BYTES: &str = "YARD_JOBS_MAX_BYTES";

/// `YARD_JOBS_MAX_COUNT` / `YARD_JOBS_MAX_BYTES` from the unit; `0` disables a cap and an
/// unparsable value keeps the default.
fn limits_from_env() -> jobs::Limits {
    let defaults = jobs::Limits::default();
    let number = |name: &str| std::env::var(name).ok()?.trim().parse::<u64>().ok();
    jobs::Limits {
        max_jobs: number(JOBS_MAX_COUNT).map_or(defaults.max_jobs, |value| value as usize),
        max_bytes: number(JOBS_MAX_BYTES).unwrap_or(defaults.max_bytes),
    }
}

#[derive(Clone, Debug)]
pub struct Timeouts {
    pub deploy: Duration,
    pub restore: Duration,
    pub backup: Duration,
    pub images_prune: Duration,
    pub host_collect: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            deploy: Duration::from_secs(30 * 60),
            restore: Duration::from_secs(30 * 60),
            backup: Duration::from_secs(60 * 60),
            images_prune: Duration::from_secs(10 * 60),
            host_collect: Duration::from_secs(2 * 60),
        }
    }
}

impl Timeouts {
    fn of(&self, action: Action) -> Duration {
        match action {
            Action::Deploy => self.deploy,
            Action::Restore => self.restore,
            Action::Backup => self.backup,
            Action::ImagesPrune => self.images_prune,
            Action::HostCollect => self.host_collect,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub program: PathBuf,
    pub projects_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Complete environment of every job; nothing else is inherited.
    pub env: Vec<(OsString, OsString)>,
    pub timeouts: Timeouts,
    /// Delay between SIGTERM and SIGKILL of a timed out job.
    pub kill_grace: Duration,
    pub output_cap: u64,
    /// Caps on the stored job history (oldest finished jobs are pruned first).
    pub limits: jobs::Limits,
    /// How long to wait for the output pipe to close once the job's process group is gone.
    pub pump_grace: Duration,
    pub refresh: Duration,
}

impl Config {
    pub fn production(projects_dir: &Path, state_dir: &Path) -> Self {
        // PATH and HOME are what git, docker and the project's backup commands need; the YARD_*
        // tuning variables (thresholds) that the unit sets are the only other pass-through.
        let mut env: Vec<(OsString, OsString)> = vec![
            (
                "PATH".into(),
                std::env::var_os("PATH").unwrap_or_else(|| DEFAULT_PATH.into()),
            ),
            (
                "HOME".into(),
                std::env::var_os("HOME").unwrap_or_else(|| "/root".into()),
            ),
        ];
        for (key, value) in std::env::vars_os() {
            if key.to_str().is_some_and(|key| key.starts_with("YARD_"))
                && key != "YARD_PROJECTS_DIR"
                && key != "YARD_STATE_DIR"
                && key != JOBS_MAX_COUNT
                && key != JOBS_MAX_BYTES
            {
                env.push((key, value));
            }
        }
        env.push(("YARD_PROJECTS_DIR".into(), projects_dir.into()));
        env.push(("YARD_STATE_DIR".into(), state_dir.into()));
        Self {
            // /proc/self/exe stays bound to the verified binary across an atomic CLI update, like
            // the read executor; update.sh restarts the unit so new jobs use the new binary.
            program: PathBuf::from("/proc/self/exe"),
            projects_dir: projects_dir.into(),
            state_dir: state_dir.into(),
            env,
            timeouts: Timeouts::default(),
            kill_grace: Duration::from_secs(10),
            output_cap: 16 * 1024 * 1024,
            limits: limits_from_env(),
            pump_grace: Duration::from_secs(5),
            refresh: Duration::from_secs(1),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    v: u8,
    op: String,
    action: Option<Action>,
    project: Option<String>,
    revision: Option<String>,
    confirm: Option<bool>,
    actor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    Global,
    Project(String),
}

#[derive(Default)]
struct Locks {
    global: Option<String>,
    projects: BTreeMap<String, String>,
}

impl Locks {
    /// Err carries the id of a job that holds the way.
    fn acquire(&mut self, scope: &Scope, id: &str) -> std::result::Result<(), String> {
        if let Some(holder) = &self.global {
            return Err(holder.clone());
        }
        match scope {
            Scope::Global => {
                if let Some(holder) = self.projects.values().next() {
                    return Err(holder.clone());
                }
                self.global = Some(id.to_owned());
            }
            Scope::Project(name) => {
                if let Some(holder) = self.projects.get(name) {
                    return Err(holder.clone());
                }
                self.projects.insert(name.clone(), id.to_owned());
            }
        }
        Ok(())
    }

    fn release(&mut self, scope: &Scope, id: &str) {
        match scope {
            Scope::Global if self.global.as_deref() == Some(id) => self.global = None,
            Scope::Project(name) if self.projects.get(name).map(String::as_str) == Some(id) => {
                self.projects.remove(name);
            }
            _ => {}
        }
    }
}

struct LockGuard {
    executor: Arc<Executor>,
    scope: Scope,
    id: String,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Ok(mut locks) = self.executor.locks.lock() {
            locks.release(&self.scope, &self.id);
        }
    }
}

pub struct Executor {
    config: Config,
    jobs_dir: PathBuf,
    locks: Mutex<Locks>,
    /// Serializes cleanup passes (start, daily, after each job).
    cleanup: Mutex<()>,
}

struct Valid {
    action: Action,
    project: Option<String>,
    revision: Option<String>,
    actor: String,
    argv: Vec<String>,
}

enum Reply {
    Started(Box<Job>),
    Pong,
    Refused(String),
    Conflict(String),
    Internal(String),
}

impl Reply {
    fn to_json(&self) -> Value {
        match self {
            Self::Started(job) => json!({"ok": true, "job": job}),
            Self::Pong => json!({"ok": true}),
            Self::Refused(message) => json!({"ok": false, "error": "refused", "message": message}),
            Self::Conflict(holder) => json!({"ok": false, "error": "conflict",
                "message": "A job is already running", "running_job_id": holder}),
            Self::Internal(message) => {
                json!({"ok": false, "error": "internal", "message": message})
            }
        }
    }
}

fn valid_name(value: &str, extra: &[u8], max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || extra.contains(&byte))
}

fn valid_actor(actor: &str) -> bool {
    valid_name(actor, b"._@-", 64)
}

fn valid_revision(revision: &str) -> bool {
    valid_name(revision, b"._:/+-", 128) && !revision.starts_with(['-', '/'])
}

fn describe(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        s if s >= 120 && s % 60 == 0 => format!("{} minutes", s / 60),
        60 => "1 minute".into(),
        1 => "1 second".into(),
        s => format!("{s} seconds"),
    }
}

fn spawn_pipe() -> std::io::Result<(File, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned by nothing else.
    Ok(unsafe { (File::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn killpg(pgid: i32, signal: i32) {
    unsafe {
        libc::kill(-pgid, signal);
    }
}

/// Copy the merged output to the log, capped. Always drains the pipe so the job never blocks.
fn pump(mut pipe: File, mut log: File, cap: u64, written: &AtomicU64, abandoned: &AtomicBool) {
    let mut buffer = [0_u8; 8192];
    let mut total = 0_u64;
    let mut finished_writing = false;
    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if abandoned.load(Ordering::Relaxed) {
            break;
        }
        if finished_writing {
            continue;
        }
        let room = cap.saturating_sub(total) as usize;
        let take = read.min(room);
        let mut ok = log.write_all(&buffer[..take]).is_ok();
        total += take as u64;
        if ok && take < read {
            let marker = b"\n[output truncated]\n";
            ok = log.write_all(marker).is_ok();
            total += marker.len() as u64;
            finished_writing = true;
        }
        if !ok {
            finished_writing = true;
        }
        written.store(total, Ordering::Relaxed);
    }
}

impl Executor {
    /// Recover from a previous executor and apply retention before any request is served.
    pub fn new(config: Config) -> Result<Arc<Self>> {
        let jobs_dir = jobs::jobs_dir(&config.state_dir);
        jobs::ensure_dir(&jobs_dir)?;
        let alive = jobs::sweep_orphans(&jobs_dir)?;
        let executor = Arc::new(Self {
            config,
            jobs_dir,
            locks: Mutex::new(Locks::default()),
            cleanup: Mutex::new(()),
        });
        executor.retain();
        for unfinished in alive {
            if let Some(pgid) = unfinished.pgid {
                executor.adopt(unfinished.job, pgid);
            }
        }
        Ok(executor)
    }

    /// Age retention, then the storage caps. Failures are reported and never stop the pass.
    pub fn retain(&self) {
        let _serial = self.cleanup.lock();
        let aged = jobs::retain(&self.jobs_dir, jobs::now_unix(), jobs::RETENTION);
        let capped = jobs::enforce_limits(&self.jobs_dir, self.config.limits);
        for error in aged.errors.iter().chain(&capped.errors) {
            eprintln!("yard ops executor retention error: {error}");
        }
        if aged.removed + capped.removed > 0 {
            eprintln!(
                "yard ops executor retention: removed {} job(s) by age, {} by storage cap",
                aged.removed, capped.removed
            );
        }
    }

    // A still-living group of a dead executor cannot be supervised; keep its locks and mark the
    // job failed once it is gone.
    fn adopt(self: &Arc<Self>, job: Job, pgid: i32) {
        let scope = Self::scope(job.action, job.project.as_deref());
        if let Ok(mut locks) = self.locks.lock() {
            match &scope {
                Scope::Global => locks.global = Some(job.id.clone()),
                Scope::Project(name) => {
                    locks.projects.insert(name.clone(), job.id.clone());
                }
            }
        }
        let guard = LockGuard {
            executor: self.clone(),
            scope,
            id: job.id.clone(),
        };
        let executor = self.clone();
        thread::spawn(move || {
            let _guard = guard;
            while jobs::group_alive(pgid) {
                thread::sleep(Duration::from_millis(500));
            }
            let mut job = job;
            executor.finish(
                &mut job,
                State::Failed,
                None,
                Some("Executor restarted".into()),
            );
        });
    }

    fn scope(action: Action, project: Option<&str>) -> Scope {
        match (action.is_global(), project) {
            (false, Some(name)) => Scope::Project(name.to_owned()),
            _ => Scope::Global,
        }
    }

    pub fn handle_line(self: &Arc<Self>, bytes: &[u8]) -> Value {
        let reply = match serde_json::from_slice::<Request>(bytes) {
            Ok(request) => match self.validate(&request) {
                Ok(None) => Reply::Pong,
                Ok(Some(valid)) => self.start(valid),
                Err(message) => Reply::Refused(message),
            },
            Err(_) => Reply::Refused("Invalid request".into()),
        };
        reply.to_json()
    }

    fn project(&self, name: &str) -> std::result::Result<Project, String> {
        if name == "host" || !valid_name(name, b"-_", 64) || name.starts_with('-') {
            return Err("Invalid project".into());
        }
        let known = Project::list(&self.config.projects_dir)
            .map_err(|_| "Project inventory unavailable".to_owned())?;
        if !known.iter().any(|candidate| candidate == name) {
            return Err("Unknown project".into());
        }
        let manifest = self.config.projects_dir.join(format!("{name}.toml"));
        if !matches!(Monitor::load(&manifest), Ok(None)) {
            return Err("Project is a URL monitor".into());
        }
        Project::load(name, &self.config.projects_dir, &self.config.state_dir)
            .map_err(|_| "Project manifest is invalid".to_owned())
    }

    /// `Ok(None)` is a valid `ping`.
    fn validate(&self, request: &Request) -> std::result::Result<Option<Valid>, String> {
        if request.v != 1 {
            return Err("Unsupported protocol version".into());
        }
        match request.op.as_str() {
            "ping" => {
                let bare = request.action.is_none()
                    && request.project.is_none()
                    && request.revision.is_none()
                    && request.confirm.is_none()
                    && request.actor.is_none();
                return if bare {
                    Ok(None)
                } else {
                    Err("Unexpected fields".into())
                };
            }
            "start" => {}
            _ => return Err("Unknown operation".into()),
        }
        let action = request.action.ok_or("Missing action")?;
        let actor = request.actor.as_deref().ok_or("Missing actor")?;
        if !valid_actor(actor) {
            return Err("Invalid actor".into());
        }
        let mut argv = vec![
            "--projects-dir".to_owned(),
            self.config.projects_dir.display().to_string(),
            "--state-dir".to_owned(),
            self.config.state_dir.display().to_string(),
        ];
        let confirmed = request.confirm == Some(true);
        let mut revision = None;
        let mut project_name = None;
        if action.is_global() {
            if request.project.is_some() || request.revision.is_some() {
                return Err("Global actions take no project or revision".into());
            }
            if action == Action::ImagesPrune {
                if !confirmed {
                    return Err("Confirmation required".into());
                }
                argv.extend(["images", "--prune", "--yes"].map(String::from));
            } else {
                argv.push("host".into());
            }
        } else {
            let name = request.project.as_deref().ok_or("Missing project")?;
            let project = self.project(name)?;
            project_name = Some(name.to_owned());
            match action {
                Action::Deploy | Action::Backup => {
                    if request.revision.is_some() {
                        return Err("Revision is only valid for restore".into());
                    }
                    if action == Action::Backup && project.config.backup.is_none() {
                        return Err("Project has no backup command".into());
                    }
                    // `--` ends the options: the name is never an option, whatever it contains.
                    argv.extend([
                        if action == Action::Deploy {
                            "deploy"
                        } else {
                            "backup"
                        }
                        .to_owned(),
                        "--".to_owned(),
                        name.to_owned(),
                    ]);
                }
                _ => {
                    let target = request.revision.as_deref().ok_or("Missing revision")?;
                    if !confirmed {
                        return Err("Confirmation required".into());
                    }
                    if !valid_revision(target) || !self.is_restore_point(&project, target) {
                        return Err("Revision is not a restore point".into());
                    }
                    revision = Some(target.to_owned());
                    argv.extend(["restore", "--yes", "--", name, target].map(String::from));
                }
            }
        }
        Ok(Some(Valid {
            action,
            project: project_name,
            revision,
            actor: actor.to_owned(),
            argv,
        }))
    }

    // Same notion as `yard restore-points`: releases recorded in the project's state.
    fn is_restore_point(&self, project: &Project, target: &str) -> bool {
        let Ok(state) = ProjectState::load(&project.state_path) else {
            return false;
        };
        let found = [&state.current, &state.previous, &state.pending]
            .into_iter()
            .flatten()
            .any(|release| match target.strip_prefix("release:") {
                Some(tag) => release.tag == tag,
                None => release.revision == target,
            });
        found
    }

    fn start(self: &Arc<Self>, valid: Valid) -> Reply {
        let now = jobs::now_unix();
        let id = match jobs::new_id(now) {
            Ok(id) => id,
            Err(error) => return Reply::Internal(error.to_string()),
        };
        let scope = Self::scope(valid.action, valid.project.as_deref());
        match self.locks.lock() {
            Ok(mut locks) => {
                if let Err(holder) = locks.acquire(&scope, &id) {
                    return Reply::Conflict(holder);
                }
            }
            Err(_) => return Reply::Internal("lock poisoned".into()),
        }
        let guard = LockGuard {
            executor: self.clone(),
            scope,
            id: id.clone(),
        };
        let mut job = Job {
            id,
            action: valid.action,
            project: valid.project,
            revision: valid.revision,
            state: State::Queued,
            actor: valid.actor,
            created_at_unix: now,
            started_at_unix: None,
            finished_at_unix: None,
            exit_code: None,
            output_bytes: 0,
            error: None,
        };
        // The log exists before the process does, so readers can always open it.
        let log = match jobs::create_log(&self.jobs_dir, &job.id) {
            Ok(log) => log,
            Err(error) => return Reply::Internal(format!("cannot create job log: {error}")),
        };
        if let Err(error) = jobs::write(&self.jobs_dir, &job) {
            return Reply::Internal(format!("cannot persist job: {error}"));
        }
        let spawned = self.spawn(&valid.argv);
        let (child, pipe) = match spawned {
            Ok(spawned) => spawned,
            Err(error) => {
                eprintln!("yard ops executor spawn error for {}: {error}", job.id);
                job.started_at_unix = Some(jobs::now_unix());
                self.finish(
                    &mut job,
                    State::Failed,
                    None,
                    Some("Failed to start".into()),
                );
                return Reply::Started(Box::new(job));
            }
        };
        let pgid = child.id() as i32;
        let _ = jobs::write_pgid(&self.jobs_dir, &job.id, pgid);
        job.state = State::Running;
        job.started_at_unix = Some(jobs::now_unix());
        if let Err(error) = jobs::write(&self.jobs_dir, &job) {
            eprintln!("yard ops executor cannot persist running state: {error}");
        }
        let reply = Reply::Started(Box::new(job.clone()));
        let executor = self.clone();
        thread::spawn(move || executor.supervise(child, pipe, log, job, guard));
        reply
    }

    fn spawn(&self, argv: &[String]) -> std::io::Result<(Child, File)> {
        let (reader, writer) = spawn_pipe()?;
        let mut command = Command::new(&self.config.program);
        command
            .args(argv)
            .env_clear()
            .envs(self.config.env.iter().map(|(key, value)| (key, value)))
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::from(writer.try_clone()?))
            .stderr(Stdio::from(writer));
        // Own session and process group, so a timeout reaches the whole tree. The unit's UMask
        // is for the socket; jobs get the umask an operator running the CLI would have.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::umask(0o022);
                Ok(())
            });
        }
        let child = command.spawn()?;
        // Dropping the command closes our copies of the write end; the pipe now ends with the job.
        drop(command);
        Ok((child, reader))
    }

    fn supervise(
        self: Arc<Self>,
        mut child: Child,
        pipe: File,
        log: File,
        mut job: Job,
        guard: LockGuard,
    ) {
        let pgid = child.id() as i32;
        let written = Arc::new(AtomicU64::new(0));
        let pump_written = written.clone();
        let cap = self.config.output_cap;
        let abandoned = Arc::new(AtomicBool::new(false));
        let pump_abandoned = abandoned.clone();
        let mut note = log.try_clone().ok();
        let (pumped, pump_done) = mpsc::channel::<()>();
        thread::spawn(move || {
            pump(pipe, log, cap, &pump_written, &pump_abandoned);
            let _ = pumped.send(());
        });
        let timeout = self.config.timeouts.of(job.action);
        let started = Instant::now();
        let mut refreshed = (Instant::now(), 0_u64);
        let mut timed_out = false;
        let status: Option<ExitStatus> = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if started.elapsed() >= timeout {
                timed_out = true;
                killpg(pgid, libc::SIGTERM);
                let grace = Instant::now();
                let status = loop {
                    if let Ok(Some(status)) = child.try_wait() {
                        break Some(status);
                    }
                    if grace.elapsed() >= self.config.kill_grace {
                        killpg(pgid, libc::SIGKILL);
                        break child.wait().ok();
                    }
                    thread::sleep(Duration::from_millis(20));
                };
                break status;
            }
            let bytes = written.load(Ordering::Relaxed);
            if bytes != refreshed.1 && refreshed.0.elapsed() >= self.config.refresh {
                job.output_bytes = bytes;
                let _ = jobs::write(&self.jobs_dir, &job);
                refreshed = (Instant::now(), bytes);
            }
            thread::sleep(Duration::from_millis(20));
        };
        // Whatever the job left behind must not outlive it, nor keep the pipe open.
        killpg(pgid, libc::SIGKILL);
        // A descendant that called setsid() escapes the group kill and can hold the pipe open
        // forever; waiting for EOF without a bound would keep the job running and its lock held.
        if pump_done.recv_timeout(self.config.pump_grace).is_err() {
            abandoned.store(true, Ordering::Relaxed);
            if let Some(log) = note.as_mut() {
                let _ = log.write_all(
                    b"\n[output closed: a leftover process kept the output pipe open]\n",
                );
            }
            eprintln!(
                "yard ops executor: job {} left a process holding its output pipe",
                job.id
            );
        }
        let (state, code, error) = match status {
            _ if timed_out => (
                State::Failed,
                status.and_then(|status| status.code()),
                Some(format!("Timed out after {}", describe(timeout))),
            ),
            Some(status) if status.success() => (State::Succeeded, Some(0), None),
            Some(status) => match (status.code(), status.signal()) {
                (Some(code), _) => (
                    State::Failed,
                    Some(code),
                    Some(format!("Exited with status {code}")),
                ),
                (None, Some(signal)) => (
                    State::Failed,
                    None,
                    Some(format!("Terminated by signal {signal}")),
                ),
                _ => (State::Failed, None, Some("Failed".into())),
            },
            None => (
                State::Failed,
                None,
                Some("Failed to wait for process".into()),
            ),
        };
        self.finish(&mut job, state, code, error);
        // Free the project before the cleanup pass, so a client that saw the job end can start
        // the next one at once. The history only grows when a job ends: enforce the caps now.
        drop(guard);
        self.retain();
    }

    fn finish(&self, job: &mut Job, state: State, code: Option<i32>, error: Option<String>) {
        job.state = state;
        job.exit_code = code;
        job.error = error;
        job.finished_at_unix = Some(jobs::now_unix());
        job.output_bytes = jobs::log_len(&self.jobs_dir, &job.id);
        if let Err(error) = jobs::write(&self.jobs_dir, job) {
            eprintln!("yard ops executor cannot persist job {}: {error}", job.id);
        }
        jobs::remove_pid(&self.jobs_dir, &job.id);
    }
}

fn handle(executor: &Arc<Executor>, mut stream: UnixStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut bytes = Vec::new();
    let read =
        BufReader::new(stream.try_clone()?.take(MAX_REQUEST + 1)).read_until(b'\n', &mut bytes)?;
    let answer = if read as u64 > MAX_REQUEST || !bytes.ends_with(b"\n") {
        Reply::Refused("Invalid request".into()).to_json()
    } else {
        executor.handle_line(&bytes)
    };
    serde_json::to_writer(&mut stream, &answer)?;
    stream.write_all(b"\n")?;
    // Unread request bytes would turn the close into a reset that discards the reply.
    stream.shutdown(std::net::Shutdown::Write)?;
    let _ = std::io::copy(&mut stream.take(64 * 1024), &mut std::io::sink());
    Ok(())
}

pub fn serve(socket: &Path, projects: &Path, state: &Path) -> Result<()> {
    let executor = Executor::new(Config::production(projects, state))?;
    let listener = read_server::bind(socket, "ops")?;
    let daily = executor.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(24 * 3600));
        daily.retain();
    });
    let active = Arc::new(AtomicUsize::new(0));
    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let executor = executor.clone();
                let active = active.clone();
                thread::spawn(move || {
                    if let Err(error) = handle(&executor, stream) {
                        eprintln!("yard ops executor request error: {error}");
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(error) => {
                eprintln!("yard ops executor accept error: {error}");
                // EMFILE and friends repeat immediately: do not spin on them.
                thread::sleep(read_server::ACCEPT_BACKOFF);
            }
        }
    }
    Err(YardError::Config("ops listener closed".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    struct Fixture {
        root: PathBuf,
        executor: Arc<Executor>,
    }

    impl Fixture {
        fn new(name: &str, tune: impl FnOnce(&mut Config)) -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("ops-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("projects")).unwrap();
            fs::create_dir_all(root.join("state")).unwrap();
            for project in ["ok", "fail", "slow", "stubborn", "big", "other", "leaky"] {
                fs::write(
                    root.join(format!("projects/{project}.toml")),
                    format!(
                        "repo = '{r}'\nbranch = 'main'\n[compose]\ndirectory = '{r}'\nfile = 'c.yml'\nenv_file = '.env'\nservice = 'api'\n[image]\nname = 'x'\ntag_env = 'T'\n[backup]\ncommand = ['true']\n",
                        r = root.display()
                    ),
                )
                .unwrap();
            }
            fs::write(
                root.join("projects/nobackup.toml"),
                format!("repo = '{r}'\nbranch = 'main'\n[compose]\ndirectory = '{r}'\nfile = 'c.yml'\nenv_file = '.env'\nservice = 'api'\n[image]\nname = 'x'\ntag_env = 'T'\n", r = root.display()),
            )
            .unwrap();
            fs::write(
                root.join("projects/mon.toml"),
                "[deployment]\nhealth_url = 'http://localhost/health'\n",
            )
            .unwrap();
            // Fake CLI: argv is `--projects-dir P --state-dir S <action> [<project>|...]`.
            // Behaviour comes from `<dir>/<project or action>.sh`.
            let program = root.join("yard");
            fs::write(
                &program,
                "#!/bin/sh\nd=$(dirname \"$0\")\nexec sh \"$d/$5.sh\" \"$@\"\n",
            )
            .unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            let script = |name: &str, body: &str| {
                fs::write(root.join(format!("{name}.sh")), body).unwrap();
            };
            // Project actions are routed by action name; the project picks the behaviour.
            script("backup", "exec sh \"$(dirname \"$0\")/p-$7.sh\"\n");
            script("deploy", "exec sh \"$(dirname \"$0\")/p-$7.sh\"\n");
            script("restore", "echo restoring \"$8\" \"$9\" \"$6\"\n");
            script("host", "echo host; echo oops >&2\n");
            script("images", "echo \"$@\"\nsleep 0.3\n");
            script("p-ok", "echo out; echo err >&2\n");
            script("p-fail", "echo bad\nexit 3\n");
            script("p-slow", "echo started\nsleep 30\n");
            script(
                "p-stubborn",
                "trap '' TERM\necho started\nwhile :; do sleep 0.05; done\n",
            );
            script("p-big", "yes x | head -c 100000\n");
            script("p-other", "sleep 0.5\n");
            // A setsid descendant escapes the group kill and keeps the output pipe open.
            script(
                "p-leaky",
                "echo before\nsetsid sleep 20 &\necho $! > \"$(dirname \"$0\")/leaky.pid\"\n",
            );
            let mut config = Config::production(&root.join("projects"), &root.join("state"));
            config.program = program;
            config.env = vec![("PATH".into(), std::env::var_os("PATH").unwrap())];
            config.kill_grace = Duration::from_millis(200);
            config.refresh = Duration::from_millis(50);
            tune(&mut config);
            let executor = Executor::new(config).unwrap();
            Self { root, executor }
        }

        fn jobs(&self) -> PathBuf {
            self.root.join("state/jobs")
        }

        fn call(&self, request: Value) -> Value {
            self.executor.handle_line(request.to_string().as_bytes())
        }

        fn start(&self, mut request: Value) -> Value {
            request["v"] = json!(1);
            request["op"] = json!("start");
            request["actor"] = json!("tester");
            self.call(request)
        }

        fn wait(&self, id: &str) -> Job {
            for _ in 0..500 {
                if let Some(job) = jobs::read(&self.jobs(), id) {
                    if job.state.is_terminal() {
                        return job;
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            panic!("job {id} did not finish");
        }

        fn id(reply: &Value) -> String {
            assert_eq!(reply["ok"], true, "{reply}");
            reply["job"]["id"].as_str().unwrap().to_owned()
        }

        fn log(&self, id: &str) -> String {
            fs::read_to_string(jobs::log_path(&self.jobs(), id)).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn successful_job_has_full_lifecycle_merged_output_and_public_files() {
        let f = Fixture::new("ok", |_| {});
        let reply = f.start(json!({"action": "backup", "project": "ok"}));
        let id = Fixture::id(&reply);
        assert!(["queued", "running"].contains(&reply["job"]["state"].as_str().unwrap()));
        assert_eq!(reply["job"]["actor"], "tester");
        assert!(reply["job"].get("v").is_none());
        let job = f.wait(&id);
        assert_eq!(job.state, State::Succeeded);
        assert_eq!(job.exit_code, Some(0));
        assert!(job.started_at_unix.is_some() && job.finished_at_unix.is_some());
        assert_eq!(job.output_bytes, 8);
        let log = f.log(&id);
        assert!(log.contains("out\n") && log.contains("err\n"), "{log}");
        for path in [
            jobs::json_path(&f.jobs(), &id),
            jobs::log_path(&f.jobs(), &id),
        ] {
            let metadata = fs::metadata(path).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o640);
            assert_eq!(metadata.gid(), fs::metadata(f.jobs()).unwrap().gid());
        }
        assert_eq!(
            fs::metadata(f.jobs()).unwrap().permissions().mode() & 0o7777,
            0o750
        );
        assert!(!jobs::pid_path(&f.jobs(), &id).exists());
    }

    #[test]
    fn failures_record_exit_code_and_clean_environment_is_used() {
        let f = Fixture::new("fail", |_| {});
        let id = Fixture::id(&f.start(json!({"action": "deploy", "project": "fail"})));
        let job = f.wait(&id);
        assert_eq!(job.state, State::Failed);
        assert_eq!(job.exit_code, Some(3));
        assert_eq!(job.error.as_deref(), Some("Exited with status 3"));
        // Spawn failure is a failed job, not a lost request.
        let broken = Fixture::new("spawn", |config| {
            config.program = "/nonexistent/yard".into()
        });
        let reply = broken.start(json!({"action": "host-collect"}));
        assert_eq!(reply["job"]["state"], "failed");
        assert_eq!(reply["job"]["error"], "Failed to start");
        // The failed spawn released the lock.
        assert_eq!(broken.start(json!({"action": "host-collect"}))["ok"], true);
    }

    #[test]
    fn argv_comes_from_templates_with_confirmation_and_restore_points() {
        let f = Fixture::new("argv", |_| {});
        fs::write(
            f.root.join("state/ok.json"),
            r#"{"current":{"revision":"abc123","tag":"t-abc","deployed_at_unix":1},"previous":{"revision":"def456","tag":"t-def","deployed_at_unix":1}}"#,
        )
        .unwrap();
        let id = Fixture::id(&f.start(
            json!({"action": "restore", "project": "ok", "revision": "def456", "confirm": true}),
        ));
        assert_eq!(f.wait(&id).state, State::Succeeded);
        assert_eq!(f.log(&id).trim(), "restoring ok def456 --yes");
        let id = Fixture::id(&f.start(
            json!({"action": "restore", "project": "ok", "revision": "release:t-abc", "confirm": true}),
        ));
        assert_eq!(f.wait(&id).state, State::Succeeded);
        let id = Fixture::id(&f.start(json!({"action": "images-prune", "confirm": true})));
        f.wait(&id);
        assert!(f.log(&id).contains("images --prune --yes"));
    }

    #[test]
    fn invalid_requests_are_refused_without_creating_jobs() {
        let f = Fixture::new("refuse", |_| {});
        fs::write(
            f.root.join("state/ok.json"),
            r#"{"current":{"revision":"abc123","tag":"t-abc","deployed_at_unix":1}}"#,
        )
        .unwrap();
        let ok_restore =
            json!({"action":"restore","project":"ok","revision":"abc123","confirm":true});
        let mut cases = vec![
            json!({"action": "deploy"}),
            json!({"action": "deploy", "project": "ghost"}),
            json!({"action": "deploy", "project": "mon"}),
            json!({"action": "deploy", "project": "host"}),
            json!({"action": "deploy", "project": "../ok"}),
            json!({"action": "deploy", "project": "ok;ls"}),
            json!({"action": "deploy", "project": "-ok"}),
            json!({"action": "backup", "project": "--help"}),
            json!({"action": "deploy", "project": "ok", "revision": "abc123"}),
            json!({"action": "backup", "project": "nobackup"}),
            json!({"action": "restore", "project": "ok", "confirm": true}),
            json!({"action": "restore", "project": "ok", "revision": "abc123"}),
            json!({"action": "restore", "project": "ok", "revision": "abc123", "confirm": false}),
            json!({"action": "restore", "project": "ok", "revision": "zzz999", "confirm": true}),
            json!({"action": "restore", "project": "ok", "revision": "--help", "confirm": true}),
            json!({"action": "restore", "project": "ok", "revision": "abc123 --yes", "confirm": true}),
            json!({"action": "restore", "project": "ok", "revision": "backup:x", "confirm": true}),
            json!({"action": "images-prune"}),
            json!({"action": "images-prune", "confirm": true, "project": "ok"}),
            json!({"action": "host-collect", "project": "ok"}),
            json!({"action": "host-collect", "revision": "abc123"}),
            json!({"action": "rollback", "project": "ok", "revision": "abc123", "confirm": true}),
            json!({"action": "deploy", "project": "ok", "actor": "bad actor"}),
            json!({"project": "ok"}),
        ];
        // The restore that is valid above must pass; any single bad field must not.
        cases.push(json!({"action":"restore","project":"ok","revision":"abc123","confirm":true,"actor":"a b"}));
        for case in cases {
            let mut request = case.clone();
            request["v"] = json!(1);
            request["op"] = json!("start");
            if request.get("actor").is_none() {
                request["actor"] = json!("tester");
            }
            let reply = f.call(request);
            assert_eq!(reply["error"], "refused", "{case}: {reply}");
        }
        for raw in [
            r#"{"v":2,"op":"ping"}"#,
            r#"{"v":1,"op":"ping","action":"deploy"}"#,
            r#"{"v":1,"op":"exec","actor":"a"}"#,
            r#"{"v":1,"op":"start","action":"deploy","project":"ok","actor":"a","argv":["sh"]}"#,
            r#"not json"#,
            r#"{"op":"ping"}"#,
        ] {
            assert_eq!(
                f.executor.handle_line(raw.as_bytes())["error"],
                "refused",
                "{raw}"
            );
        }
        assert_eq!(f.call(json!({"v":1,"op":"ping"})), json!({"ok": true}));
        assert!(fs::read_dir(f.jobs()).unwrap().next().is_none());
        // The valid sibling of the refusals above is accepted.
        let mut accepted = ok_restore;
        accepted["v"] = json!(1);
        accepted["op"] = json!("start");
        accepted["actor"] = json!("tester");
        assert_eq!(f.call(accepted)["ok"], true);
    }

    #[test]
    fn locks_are_per_project_and_global_excludes_everything() {
        let f = Fixture::new("locks", |_| {});
        let slow = Fixture::id(&f.start(json!({"action": "backup", "project": "slow"})));
        let again = f.start(json!({"action": "deploy", "project": "slow"}));
        assert_eq!(again["error"], "conflict", "{again}");
        assert_eq!(again["running_job_id"], slow);
        // Another project runs concurrently.
        let other = Fixture::id(&f.start(json!({"action": "backup", "project": "other"})));
        // A global action is refused while anything runs, with a holder id.
        let global = f.start(json!({"action": "host-collect"}));
        assert_eq!(global["error"], "conflict", "{global}");
        assert!(global["running_job_id"] == slow || global["running_job_id"] == other);
        f.wait(&other);
        // Pure lock-table semantics.
        let mut locks = Locks::default();
        assert!(locks.acquire(&Scope::Global, "g").is_ok());
        assert_eq!(
            locks.acquire(&Scope::Project("a".into()), "p"),
            Err("g".into())
        );
        assert_eq!(locks.acquire(&Scope::Global, "g2"), Err("g".into()));
        locks.release(&Scope::Global, "g");
        assert!(locks.acquire(&Scope::Project("a".into()), "p").is_ok());
        assert!(locks.acquire(&Scope::Project("b".into()), "q").is_ok());
        assert_eq!(locks.acquire(&Scope::Global, "g"), Err("p".into()));
        // Kill the slow job's group so the fixture does not leave a sleeper behind.
        let pgid: i32 = fs::read_to_string(jobs::pid_path(&f.jobs(), &slow))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        killpg(pgid, libc::SIGKILL);
        assert_eq!(f.wait(&slow).state, State::Failed);
        let after = f.start(json!({"action": "host-collect"}));
        assert_eq!(after["ok"], true, "{after}");
        let id = Fixture::id(&after);
        f.wait(&id);
    }

    #[test]
    fn global_lock_blocks_project_jobs() {
        let f = Fixture::new("globalblock", |_| {});
        let id = Fixture::id(&f.start(json!({"action": "images-prune", "confirm": true})));
        let blocked = f.start(json!({"action": "backup", "project": "ok"}));
        assert_eq!(blocked["error"], "conflict", "{blocked}");
        assert_eq!(blocked["running_job_id"], id);
        f.wait(&id);
        assert_eq!(
            f.start(json!({"action": "backup", "project": "ok"}))["ok"],
            true
        );
    }

    #[test]
    fn timeout_terminates_then_kills_the_whole_group() {
        let f = Fixture::new("timeout", |config| {
            config.timeouts.backup = Duration::from_secs(1);
        });
        let started = Instant::now();
        let id = Fixture::id(&f.start(json!({"action": "backup", "project": "stubborn"})));
        let job = f.wait(&id);
        assert_eq!(job.state, State::Failed);
        assert_eq!(job.error.as_deref(), Some("Timed out after 1 second"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(f.log(&id).contains("started"));
        // The lock is free again after the kill.
        let again = f.start(json!({"action": "backup", "project": "stubborn"}));
        assert_eq!(again["ok"], true);
        f.wait(&Fixture::id(&again));
    }

    #[test]
    fn a_setsid_descendant_holding_the_pipe_does_not_pin_the_job_or_its_lock() {
        let f = Fixture::new("leaky", |config| {
            config.pump_grace = Duration::from_millis(300);
        });
        let started = Instant::now();
        let id = Fixture::id(&f.start(json!({"action": "backup", "project": "leaky"})));
        let job = f.wait(&id);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(job.state, State::Succeeded);
        let log = f.log(&id);
        assert!(log.starts_with("before\n"), "{log}");
        assert!(log.contains("[output closed: a leftover process"), "{log}");
        assert_eq!(job.output_bytes, log.len() as u64);
        // The project lock was released although the descendant still lives.
        let again = f.start(json!({"action": "backup", "project": "leaky"}));
        assert_eq!(again["ok"], true, "{again}");
        f.wait(&Fixture::id(&again));
        if let Ok(pid) = fs::read_to_string(f.root.join("leaky.pid")) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }

    #[test]
    fn output_is_capped_once_and_the_job_still_succeeds() {
        let f = Fixture::new("cap", |config| config.output_cap = 1000);
        let id = Fixture::id(&f.start(json!({"action": "backup", "project": "big"})));
        let job = f.wait(&id);
        assert_eq!(job.state, State::Succeeded);
        let log = f.log(&id);
        assert_eq!(log.matches("[output truncated]").count(), 1);
        assert!(log.ends_with("\n[output truncated]\n"));
        assert_eq!(job.output_bytes, log.len() as u64);
        assert_eq!(log.len(), 1000 + "\n[output truncated]\n".len());
    }

    #[test]
    fn restart_fails_orphans_and_releases_nothing_it_does_not_hold() {
        let f = Fixture::new("orphan", |_| {});
        let id = "20261019T000001Z-0000abcd";
        let dir = f.jobs();
        jobs::write(
            &dir,
            &Job {
                id: id.into(),
                action: Action::Backup,
                project: Some("slow".into()),
                revision: None,
                state: State::Running,
                actor: "tester".into(),
                created_at_unix: 1,
                started_at_unix: Some(2),
                finished_at_unix: None,
                exit_code: None,
                output_bytes: 0,
                error: None,
            },
        )
        .unwrap();
        fs::write(jobs::log_path(&dir, id), "partial\n").unwrap();
        let again = Executor::new(f.executor.config.clone()).unwrap();
        let job = jobs::read(&dir, id).unwrap();
        assert_eq!(job.state, State::Failed);
        assert_eq!(job.error.as_deref(), Some("Executor restarted"));
        assert_eq!(job.output_bytes, 8);
        // The project is not locked by the orphan.
        let reply = again.handle_line(
            json!({"v":1,"op":"start","action":"backup","project":"slow","actor":"t"})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(reply["ok"], true);
        let pgid: i32 =
            fs::read_to_string(jobs::pid_path(&dir, reply["job"]["id"].as_str().unwrap()))
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
        killpg(pgid, libc::SIGKILL);
    }

    #[test]
    fn finished_jobs_beyond_the_count_cap_are_pruned() {
        let f = Fixture::new("cap-count", |config| {
            config.limits = jobs::Limits {
                max_jobs: 2,
                max_bytes: 0,
            };
        });
        let mut last = String::new();
        for _ in 0..4 {
            last = Fixture::id(&f.start(json!({"action": "backup", "project": "ok"})));
            f.wait(&last);
            // Finish times have a one second resolution; keep the order of the jobs unambiguous.
            thread::sleep(Duration::from_millis(1100));
        }
        let kept = fs::read_dir(f.jobs())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
            .count();
        assert_eq!(kept, 2);
        assert!(jobs::json_path(&f.jobs(), &last).exists());
    }
}
