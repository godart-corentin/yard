//! Local, read-only boundary for Yard Web. The wire format cannot name a binary or CLI flags.
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::error::{Result, YardError};
use crate::monitor::Monitor;
use crate::project::Project;
use crate::status;

const MAX_REQUEST: u64 = 1024;
const MAX_OUTPUT: u64 = 128 * 1024;
const DEADLINE: Duration = Duration::from_secs(10);
const MAX_FOLLOWS: usize = 4;
const FOLLOW_LIMIT: Duration = Duration::from_secs(3600);
const MAX_FOLLOW_LINE: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    op: String,
    project: Option<String>,
    service: Option<String>,
    tail: Option<u32>,
    since: Option<String>,
}

fn allowed(request: &ReadRequest, projects: &Path) -> Option<Vec<String>> {
    let project = match request.op.as_str() {
        "images"
            if request.project.is_none()
                && request.service.is_none()
                && request.tail.is_none()
                && request.since.is_none() =>
        {
            return Some(vec!["images".into()])
        }
        "images-prune-plan"
            if request.project.is_none()
                && request.service.is_none()
                && request.tail.is_none()
                && request.since.is_none() =>
        {
            // Dry run only: the confirming flag is never part of a read.
            return Some(vec!["images".into(), "--prune".into()]);
        }
        "logs" | "logs-follow" | "restore-points" | "restore-log" => request.project.as_deref()?,
        _ => return None,
    };
    if project.is_empty()
        || project.len() > 64
        || !project
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || !Project::list(projects)
            .ok()?
            .iter()
            .any(|name| name == project)
        || Monitor::load(&projects.join(format!("{project}.toml")))
            .ok()?
            .is_some()
    {
        return None;
    }
    let configured = Project::load(project, projects, Path::new("/dev/null")).ok()?;
    if !matches!(request.op.as_str(), "logs" | "logs-follow") {
        return (request.service.is_none() && request.tail.is_none() && request.since.is_none())
            .then(|| vec![request.op.clone(), project.to_owned()]);
    }
    let tail = request.tail.unwrap_or(200);
    if tail == 0
        || tail > 1000
        || request.service.as_deref().is_some_and(|service| {
            !configured
                .config
                .compose
                .services
                .iter()
                .any(|name| name == service)
        })
    {
        return None;
    }
    if request.since.as_deref().is_some_and(|since| {
        since.is_empty()
            || since.len() > 40
            || !since.bytes().all(|byte| {
                byte.is_ascii_digit()
                    || matches!(
                        byte,
                        b'T' | b'Z' | b'+' | b'-' | b':' | b'.' | b's' | b'm' | b'h' | b'd'
                    )
            })
    }) {
        return None;
    }
    let mut args = vec![
        "logs".into(),
        project.to_owned(),
        if request.op == "logs-follow" {
            "--follow"
        } else {
            "--no-follow"
        }
        .into(),
        "--tail".into(),
        tail.to_string(),
    ];
    if let Some(service) = &request.service {
        args.extend(["--service".into(), service.clone()]);
    }
    if let Some(since) = &request.since {
        args.extend(["--since".into(), since.clone()]);
    }
    Some(args)
}

fn cli_command(args: &[String], projects: &Path, state: &Path) -> Command {
    // /proc/self/exe remains bound to this verified binary across an atomic CLI update.
    let program = Path::new("/proc/self/exe");
    let mut command = Command::new(program);
    command
        .arg("--projects-dir")
        .arg(projects)
        .arg("--state-dir")
        .arg(state)
        .args(args);
    command
}

fn execute(
    args: &[String],
    projects: &Path,
    state: &Path,
) -> std::result::Result<String, &'static str> {
    collect_command(&mut cli_command(args, projects, state), DEADLINE)
}

static FOLLOWS: AtomicUsize = AtomicUsize::new(0);

/// Releases one of the `MAX_FOLLOWS` stream slots when the stream ends, however it ends.
struct FollowSlot;

impl FollowSlot {
    fn take() -> Option<Self> {
        (FOLLOWS.fetch_add(1, Ordering::SeqCst) < MAX_FOLLOWS)
            .then_some(Self)
            .or_else(|| {
                FOLLOWS.fetch_sub(1, Ordering::SeqCst);
                None
            })
    }
}

impl Drop for FollowSlot {
    fn drop(&mut self) {
        FOLLOWS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn frame(stream: &mut UnixStream, value: &serde_json::Value) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    stream.write_all(&bytes)
}

// Lines are bounded so one runaway log line cannot grow memory; the channel is bounded so a slow
// client back-pressures the child instead of buffering its output.
fn pump_lines(mut pipe: impl Read, lines: mpsc::SyncSender<String>) {
    let flush = |line: &mut Vec<u8>| {
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let text = String::from_utf8_lossy(line).into_owned();
        line.clear();
        lines.send(text).is_ok()
    };
    let mut buffer = [0_u8; 8192];
    let mut line = Vec::new();
    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for &byte in &buffer[..read] {
            if byte == b'\n' {
                if !flush(&mut line) {
                    return;
                }
            } else if line.len() < MAX_FOLLOW_LINE {
                line.push(byte);
            }
        }
    }
    if !line.is_empty() {
        flush(&mut line);
    }
}

/// Stream the output of `command` as JSON lines until the peer leaves, `limit` elapses or the
/// command ends. The command's whole process group is killed in every case.
fn follow(mut stream: UnixStream, mut command: Command, limit: Duration) {
    let Some(_slot) = FollowSlot::take() else {
        let _ = frame(&mut stream, &json!({"error": "Read operation unavailable"}));
        return;
    };
    // The reply is a long-lived stream; only a stuck write should time out.
    if stream.set_read_timeout(None).is_err()
        || stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .is_err()
    {
        return;
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let Ok(mut child) = command.spawn() else {
        let _ = frame(&mut stream, &json!({"end": "error"}));
        return;
    };
    let group = child.id() as i32;
    let (sender, lines) = mpsc::sync_channel::<String>(256);
    if let Some(stdout) = child.stdout.take() {
        let sender = sender.clone();
        thread::spawn(move || pump_lines(stdout, sender));
    }
    if let Some(stderr) = child.stderr.take() {
        let sender = sender.clone();
        thread::spawn(move || pump_lines(stderr, sender));
    }
    drop(sender);
    // Web sends nothing after its request: any read result means the peer closed or failed.
    let gone = Arc::new(AtomicBool::new(false));
    if let Ok(mut peer) = stream.try_clone() {
        let gone = gone.clone();
        thread::spawn(move || {
            let mut sink = [0_u8; 64];
            while matches!(peer.read(&mut sink), Ok(read) if read > 0) {}
            gone.store(true, Ordering::SeqCst);
        });
    }
    let started = Instant::now();
    let mut end = Some("error");
    loop {
        if gone.load(Ordering::SeqCst) {
            end = None;
            break;
        }
        if started.elapsed() >= limit {
            end = Some("closed");
            break;
        }
        match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if frame(&mut stream, &json!({"line": line})).is_err() {
                    end = None;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // A command that ended must not be kept alive by its leftover descendants.
                if matches!(child.try_wait(), Ok(Some(_))) {
                    unsafe {
                        libc::kill(-group, libc::SIGKILL);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    unsafe {
        libc::kill(-group, libc::SIGKILL);
    }
    let _ = child.wait();
    if let Some(reason) = end {
        let _ = frame(&mut stream, &json!({"end": reason}));
    }
    // Wakes the peer watcher, which would otherwise hold the connection open.
    let _ = stream.shutdown(Shutdown::Both);
}

fn collect_command(
    command: &mut Command,
    deadline: Duration,
) -> std::result::Result<String, &'static str> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // The entire subprocess tree must be in its own group, including tests of this collector.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| "Read operation failed")?;
    let stdout = child.stdout.take().ok_or("Read operation failed")?;
    let stderr = child.stderr.take().ok_or("Read operation failed")?;
    let output = thread::spawn(move || {
        let mut bytes = Vec::new();
        BufReader::new(stdout)
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut bytes)?;
        Ok::<_, std::io::Error>(bytes)
    });
    let errors = thread::spawn(move || {
        let mut bytes = Vec::new();
        BufReader::new(stderr)
            .take(MAX_OUTPUT + 1)
            .read_to_end(&mut bytes)?;
        Ok::<_, std::io::Error>(bytes)
    });
    let start = Instant::now();
    let status = loop {
        let status = child.try_wait().map_err(|_| "Read operation failed")?;
        if status.is_some() && output.is_finished() && errors.is_finished() {
            break status;
        }
        if start.elapsed() >= deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    if status.is_none() {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        child.wait().ok();
    }
    let stdout = output
        .join()
        .map_err(|_| "Read operation failed")?
        .map_err(|_| "Read operation failed")?;
    let stderr = errors
        .join()
        .map_err(|_| "Read operation failed")?
        .map_err(|_| "Read operation failed")?;
    if status.is_none() || stdout.len() as u64 > MAX_OUTPUT || stderr.len() as u64 > MAX_OUTPUT {
        return Err("Read operation unavailable or output limit exceeded");
    }
    if !status.is_some_and(|status| status.success()) {
        return Err("Read operation failed");
    }
    String::from_utf8(stdout).map_err(|_| "Read operation failed")
}

fn handle(mut stream: UnixStream, projects: &Path, state: &Path) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut bytes = Vec::new();
    let read =
        BufReader::new(stream.try_clone()?.take(MAX_REQUEST + 1)).read_until(b'\n', &mut bytes)?;
    let answer = if read as u64 > MAX_REQUEST || !bytes.ends_with(b"\n") {
        json!({"error": "Read operation refused"})
    } else if let Ok(request) = serde_json::from_slice::<ReadRequest>(&bytes) {
        if request.op == "logs-follow" {
            return match allowed(&request, projects) {
                Some(args) => {
                    let command = cli_command(&args, projects, state);
                    // The stream outlives this call and must not block the accept loop.
                    thread::spawn(move || follow(stream, command, FOLLOW_LIMIT));
                    Ok(())
                }
                None => {
                    serde_json::to_writer(
                        &mut stream,
                        &json!({"error": "Read operation refused"}),
                    )?;
                    stream.write_all(b"\n")
                }
            };
        }
        if request.op == "diagnostics"
            && request.project.is_none()
            && request.service.is_none()
            && request.tail.is_none()
            && request.since.is_none()
        {
            json!({"diagnostics": status::web_diagnostics(projects, state)})
        } else {
            match allowed(&request, projects) {
                Some(args) => match execute(&args, projects, state) {
                    Ok(output) => json!({"output": output}),
                    Err(error) => json!({"error": error}),
                },
                None => json!({"error": "Read operation refused"}),
            }
        }
    } else {
        json!({"error": "Read operation refused"})
    };
    serde_json::to_writer(&mut stream, &answer)?;
    stream.write_all(b"\n")
}

/// Bind a Unix socket (`0660`) in a dedicated runtime directory, replacing only a dead socket.
pub fn bind(socket: &Path, label: &str) -> Result<UnixListener> {
    // Only a dedicated administrator-owned runtime directory should contain this socket.
    if let Ok(metadata) = fs::symlink_metadata(socket) {
        if !metadata.file_type().is_socket() {
            return Err(YardError::Config(format!(
                "{label} socket path is not a socket"
            )));
        }
        // A live peer must not be displaced by a second executor.
        if UnixStream::connect(socket).is_ok() {
            return Err(YardError::Config(format!(
                "{label} executor already running"
            )));
        }
        fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}

pub fn serve(socket: &Path, projects: &Path, state: &Path) -> Result<()> {
    let listener = bind(socket, "read")?;
    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                if let Err(error) = handle(stream, projects, state) {
                    eprintln!("yard read executor request error: {error}");
                }
            }
            Err(error) => eprintln!("yard read executor accept error: {error}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exited_parent_cannot_leave_the_reader_blocked_on_a_descendant() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 3 &"]);
        let start = Instant::now();
        let result = collect_command(&mut command, Duration::from_millis(250));
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    fn pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().unwrap()
    }

    fn lines_of(mut client: UnixStream) -> Vec<serde_json::Value> {
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut text = String::new();
        client.read_to_string(&mut text).unwrap();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn follow_frames_lines_truncates_and_reports_a_command_that_ended() {
        let (server, client) = pair();
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "echo one; echo two >&2; head -c 20000 /dev/zero | tr '\\0' x; echo",
        ]);
        follow(server, command, Duration::from_secs(5));
        let frames = lines_of(client);
        let lines: Vec<_> = frames.iter().filter_map(|f| f["line"].as_str()).collect();
        assert!(
            lines.contains(&"one") && lines.contains(&"two"),
            "{frames:?}"
        );
        assert!(lines.iter().any(|line| line.len() == MAX_FOLLOW_LINE));
        assert_eq!(frames.last().unwrap(), &json!({"end": "error"}));
    }

    #[test]
    fn follow_stops_at_the_time_limit_and_kills_the_group() {
        let (server, client) = pair();
        let mut command = Command::new("sh");
        command.args(["-c", "echo hello; sleep 30 & sleep 30"]);
        let started = Instant::now();
        follow(server, command, Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(5));
        let frames = lines_of(client);
        assert_eq!(frames.first().unwrap(), &json!({"line": "hello"}));
        assert_eq!(frames.last().unwrap(), &json!({"end": "closed"}));
    }

    #[test]
    fn follow_stops_when_the_client_disconnects() {
        let (server, client) = pair();
        let mut command = Command::new("sh");
        command.args(["-c", "while :; do echo tick; sleep 0.05; done"]);
        let handle = thread::spawn(move || follow(server, command, Duration::from_secs(30)));
        let mut reader = BufReader::new(client);
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        drop(reader);
        let started = Instant::now();
        handle.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn oversized_command_output_is_refused_without_leaking_it() {
        let mut command = Command::new("sh");
        command.args(["-c", "yes x | head -c 131073"]);
        assert!(collect_command(&mut command, Duration::from_secs(2)).is_err());
    }
}
