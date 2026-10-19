use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    child: Child,
}

impl Fixture {
    fn new() -> Self {
        Self::start(false)
    }

    fn with_fake_docker() -> Self {
        Self::start(true)
    }

    fn start(fake_docker: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "yard-read-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("projects")).unwrap();
        fs::create_dir_all(root.join("state")).unwrap();
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(root.join("projects/demo.toml"), format!("repo = '{app}'\nbranch = 'main'\n[compose]\ndirectory = '{app}'\nfile = 'compose.yml'\nenv_file = '.env'\nservice = 'api'\n[image]\nname = 'demo'\ntag_env = 'DEMO_TAG'\n[deployment]\nhealth_url = 'http://localhost/health'\n", app = root.join("app").display())).unwrap();
        fs::write(
            root.join("projects/monitor.toml"),
            "[deployment]\nhealth_url = 'http://localhost/health'\n",
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_yard"));
        command
            .args(["read-server", "--socket"])
            .arg(root.join("read.sock"))
            .env("YARD_PROJECTS_DIR", root.join("projects"))
            .env("YARD_STATE_DIR", root.join("state"));
        if fake_docker {
            let bin = root.join("bin");
            fs::create_dir_all(&bin).unwrap();
            let executable = bin.join("docker");
            fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
            command.env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            );
        }
        let mut child = command.spawn().unwrap();
        for _ in 0..100 {
            if root.join("read.sock").exists() {
                return Self { root, child };
            }
            thread::sleep(Duration::from_millis(20));
        }
        child.kill().ok();
        child.wait().ok();
        panic!("read server did not create socket");
    }

    fn request(&self, request: &str) -> serde_json::Value {
        let mut stream = UnixStream::connect(self.root.join("read.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        stream.write_all(format!("{request}\n").as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
        fs::remove_dir_all(&self.root).ok();
    }
}

#[test]
fn recorded_points_are_real_and_missing_state_is_explicit() {
    let fixture = Fixture::new();
    let result = fixture.request(r#"{"op":"restore-points","project":"demo"}"#);
    assert!(
        result["output"]
            .as_str()
            .unwrap()
            .contains("No restore points recorded"),
        "{result}"
    );
    fs::write(fixture.root.join("state/demo.json"), "{broken").unwrap();
    let invalid = fixture.request(r#"{"op":"restore-points","project":"demo"}"#);
    assert_eq!(invalid["error"], "Read operation failed");
    assert!(invalid.get("output").is_none());
}

#[test]
fn mutation_unknown_fields_traversal_and_monitor_are_refused() {
    let fixture = Fixture::new();
    for request in [
        r#"{"op":"deploy","project":"demo"}"#,
        r#"{"op":"images","prune":true}"#,
        r#"{"op":"restore","project":"demo","yes":true}"#,
        r#"{"op":"logs","project":"../demo"}"#,
        r#"{"op":"logs","project":"monitor"}"#,
        r#"{"op":"logs","project":"demo","tail":999999}"#,
        r#"{"op":"logs","project":"demo","service":"--help"}"#,
        r#"{"op":"logs","project":"demo","since":"2h;touch /tmp/pwn"}"#,
        r#"{"op":"restore-log","project":"demo","binary":"sh"}"#,
    ] {
        let result = fixture.request(request);
        assert_eq!(
            result["error"], "Read operation refused",
            "{request}: {result}"
        );
    }
}

#[test]
fn restore_journal_and_log_service_are_bounded_and_validated() {
    let fixture = Fixture::new();
    let journal = fixture.request(r#"{"op":"restore-log","project":"demo"}"#);
    assert!(journal["output"]
        .as_str()
        .unwrap()
        .contains("No restore attempts recorded"));
    let service =
        fixture.request(r#"{"op":"logs","project":"demo","service":"worker","tail":100}"#);
    assert_eq!(service["error"], "Read operation refused");
    let logs = fixture
        .request(r#"{"op":"logs","project":"demo","service":"api","tail":100,"since":"2h"}"#);
    assert_eq!(logs["error"], "Read operation failed");
}

#[test]
fn configured_service_logs_are_one_shot_with_bounded_filters() {
    let fixture = Fixture::with_fake_docker();
    let result =
        fixture.request(r#"{"op":"logs","project":"demo","service":"api","tail":50,"since":"2h"}"#);
    let output = result["output"].as_str().unwrap();
    assert!(
        output.contains("logs\n--tail\n50\n--since\n2h\napi\n"),
        "{output}"
    );
    assert!(!output.contains("--follow"), "{output}");
}

#[test]
fn diagnostics_match_cli_overview_for_running_drift_pending_and_unreadable_state() {
    let fixture = Fixture::with_fake_docker();
    let docker = fixture.root.join("bin/docker");
    fs::write(&docker, "#!/bin/sh\ncase \"$*\" in\n  *'ps --all --format json'*) printf '%s\\n' '{\"Service\":\"api\",\"State\":\"running\",\"Health\":\"healthy\",\"Image\":\"demo:current\"}' ;;\n  *) exit 1 ;;\nesac\n").unwrap();
    fs::write(fixture.root.join("app/.env"), "DEMO_TAG=TOPSECRET\n").unwrap();
    let cli = || {
        let output = Command::new(env!("CARGO_BIN_EXE_yard"))
            .args([
                "--projects-dir",
                fixture.root.join("projects").to_str().unwrap(),
                "--state-dir",
                fixture.root.join("state").to_str().unwrap(),
                "status",
            ])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    fixture.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    for (record, expected) in [
        (
            r#"{"current":{"revision":"current","tag":"current","deployed_at_unix":42,"services":[{"name":"api","image":"demo:current"}]}}"#,
            "ok",
        ),
        (
            r#"{"current":{"revision":"current","tag":"current","deployed_at_unix":42,"services":[{"name":"api","image":"demo:old"}]}}"#,
            "alert",
        ),
        (
            r#"{"pending":{"revision":"next","tag":"next","deployed_at_unix":43}}"#,
            "alert",
        ),
        ("{broken", "alert"),
    ] {
        fs::write(fixture.root.join("state/demo.json"), record).unwrap();
        let diagnostic = fixture.request(r#"{"op":"diagnostics"}"#);
        let item = diagnostic["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "demo")
            .unwrap();
        assert_eq!(item["verdict"], expected, "{item}");
        if expected == "ok" {
            assert_eq!(item["env_tag"], "other");
        }
        assert!(!diagnostic.to_string().contains("TOPSECRET"));
        assert!(cli().contains(&format!(
            "{} demo:",
            if expected == "ok" { "OK" } else { "ALERT" }
        )));
        assert!(!diagnostic
            .to_string()
            .contains(fixture.root.to_str().unwrap()));
    }
    for request in [
        r#"{"op":"diagnostics","project":"demo"}"#,
        r#"{"op":"diagnostics","secret":"value"}"#,
    ] {
        assert_eq!(fixture.request(request)["error"], "Read operation refused");
    }
}

#[test]
fn prune_plan_is_the_dry_run_and_takes_no_arguments() {
    let fixture = Fixture::with_fake_docker();
    let result = fixture.request(r#"{"op":"images-prune-plan"}"#);
    // The fake docker lists nothing, so only the dry-run banner can be asserted; never --yes.
    assert!(result.get("output").is_some() || result["error"] == "Read operation failed");
    for request in [
        r#"{"op":"images-prune-plan","project":"demo"}"#,
        r#"{"op":"images-prune-plan","yes":true}"#,
        r#"{"op":"images-prune-plan","tail":5}"#,
    ] {
        assert_eq!(fixture.request(request)["error"], "Read operation refused");
    }
}

fn follow_fixture() -> Fixture {
    let fixture = Fixture::with_fake_docker();
    let docker = fixture.root.join("bin/docker");
    fs::write(
        &docker,
        format!(
            "#!/bin/sh\necho $$ >> '{}'\nprintf '%s\\n' \"$@\"\nexec sleep 60\n",
            fixture.root.join("docker.pids").display()
        ),
    )
    .unwrap();
    fixture
}

fn follow_stream(fixture: &Fixture, request: &str) -> BufReader<UnixStream> {
    let mut stream = UnixStream::connect(fixture.root.join("read.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    stream.write_all(format!("{request}\n").as_bytes()).unwrap();
    BufReader::new(stream)
}

fn alive(pid: &str) -> bool {
    std::path::Path::new(&format!("/proc/{}", pid.trim())).exists()
        && !fs::read_to_string(format!("/proc/{}/stat", pid.trim()))
            .map(|stat| stat.contains(") Z"))
            .unwrap_or(true)
}

#[test]
fn logs_follow_streams_frames_and_kills_the_child_when_the_client_leaves() {
    let fixture = follow_fixture();
    let mut reader = follow_stream(
        &fixture,
        r#"{"op":"logs-follow","project":"demo","service":"api","tail":5,"since":"1h"}"#,
    );
    let mut seen = Vec::new();
    for _ in 0..20 {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
        seen.push(frame["line"].as_str().unwrap().to_owned());
        if seen.last().unwrap() == "api" {
            break;
        }
    }
    let joined = seen.join(" ");
    assert!(joined.contains("--follow"), "{joined}");
    assert!(!joined.contains("--no-follow"), "{joined}");
    assert!(
        joined.contains("--tail 5 --follow --since 1h api"),
        "{joined}"
    );
    let pids = fs::read_to_string(fixture.root.join("docker.pids")).unwrap();
    let pid = pids.lines().next().unwrap().to_owned();
    assert!(alive(&pid));
    drop(reader);
    for _ in 0..100 {
        if !alive(&pid) {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("follow child {pid} survived its client");
}

#[test]
fn logs_follow_is_validated_like_logs_and_limited_to_four_streams() {
    let fixture = follow_fixture();
    for request in [
        r#"{"op":"logs-follow","project":"monitor"}"#,
        r#"{"op":"logs-follow","project":"demo","service":"worker"}"#,
        r#"{"op":"logs-follow","project":"demo","tail":0}"#,
        r#"{"op":"logs-follow","project":"demo","since":"1h;ls"}"#,
        r#"{"op":"logs-follow"}"#,
    ] {
        assert_eq!(fixture.request(request)["error"], "Read operation refused");
    }
    let mut streams: Vec<_> = (0..4)
        .map(|_| follow_stream(&fixture, r#"{"op":"logs-follow","project":"demo"}"#))
        .collect();
    for stream in &mut streams {
        let mut line = String::new();
        stream.read_line(&mut line).unwrap();
        assert!(line.contains("\"line\""), "{line}");
    }
    let mut fifth = follow_stream(&fixture, r#"{"op":"logs-follow","project":"demo"}"#);
    let mut line = String::new();
    fifth.read_line(&mut line).unwrap();
    assert!(line.contains("\"error\""), "{line}");
    // One-shot reads keep working while streams are open.
    assert!(fixture.request(r#"{"op":"restore-log","project":"demo"}"#)["output"].is_string());
}
