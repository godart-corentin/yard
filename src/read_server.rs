//! Local, read-only boundary for Yard Web. The wire format cannot name a binary or CLI flags.
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::error::{Result, YardError};
use crate::monitor::Monitor;
use crate::project::Project;

const MAX_REQUEST: u64 = 1024;
const MAX_OUTPUT: u64 = 128 * 1024;
const DEADLINE: Duration = Duration::from_secs(10);

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
        "logs" | "restore-points" | "restore-log" => request.project.as_deref()?,
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
    if request.op != "logs" {
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
        "--no-follow".into(),
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

fn execute(
    args: &[String],
    projects: &Path,
    state: &Path,
) -> std::result::Result<String, &'static str> {
    // /proc/self/exe remains bound to this verified binary across an atomic CLI update.
    let program = Path::new("/proc/self/exe");
    let mut command = Command::new(program);
    command
        .arg("--projects-dir")
        .arg(projects)
        .arg("--state-dir")
        .arg(state)
        .args(args);
    collect_command(&mut command, DEADLINE)
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
        match allowed(&request, projects) {
            Some(args) => match execute(&args, projects, state) {
                Ok(output) => json!({"output": output}),
                Err(error) => json!({"error": error}),
            },
            None => json!({"error": "Read operation refused"}),
        }
    } else {
        json!({"error": "Read operation refused"})
    };
    serde_json::to_writer(&mut stream, &answer)?;
    stream.write_all(b"\n")
}

pub fn serve(socket: &Path, projects: &Path, state: &Path) -> Result<()> {
    // Only a dedicated administrator-owned runtime directory should contain this socket.
    if let Ok(metadata) = fs::symlink_metadata(socket) {
        if !metadata.file_type().is_socket() {
            return Err(YardError::Config("read socket path is not a socket".into()));
        }
        // A live peer must not be displaced by a second executor.
        if UnixStream::connect(socket).is_ok() {
            return Err(YardError::Config("read executor already running".into()));
        }
        fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o660))?;
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

    #[test]
    fn oversized_command_output_is_refused_without_leaking_it() {
        let mut command = Command::new("sh");
        command.args(["-c", "yes x | head -c 131073"]);
        assert!(collect_command(&mut command, Duration::from_secs(2)).is_err());
    }
}
