use std::fs;
use std::io::{Read, Write};
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
