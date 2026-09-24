use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "yard-logs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("projects")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(
            root.join("projects/demo.toml"),
            format!(
                "repo = '{0}'\nbranch = 'main'\n[compose]\ndirectory = '{0}'\nfile = 'compose.yml'\nenv_file = '.env'\nservices = ['api', 'worker']\n[image]\nname = 'demo'\ntag_env = 'IMAGE_TAG'\n",
                root.display()
            ),
        )
        .unwrap();
        let docker = root.join("bin/docker");
        fs::write(
            &docker,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FAKE_ROOT/args\"\nif [ \"${FAKE_FAIL:-0}\" = 1 ]; then echo 'docker failed' >&2; exit 17; fi\nif [ \"${FAKE_WAIT:-0}\" = 1 ]; then\n  trap 'rm \"$FAKE_ROOT/active\"; exit 130' INT TERM\n  printf 'active\\n' > \"$FAKE_ROOT/active\"\n  while :; do sleep 1; done\nfi\nfor arg do last=$arg; done\nprintf 'log for %s\\n' \"$last\"\n",
        )
        .unwrap();
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
        Self { root }
    }

    fn run(&self, options: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_yard"));
        cmd.args([
            "--projects-dir",
            self.root.join("projects").to_str().unwrap(),
            "--state-dir",
            self.root.join("state").to_str().unwrap(),
            "logs",
            "demo",
        ])
        .args(options)
        .env("FAKE_ROOT", &self.root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                self.root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        );
        cmd.output().unwrap()
    }

    fn args(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("args"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn expected(&self, suffix: &[&str]) -> Vec<String> {
        let mut args = vec![
            "compose".to_owned(),
            "--env-file".to_owned(),
            self.root.join(".env").display().to_string(),
            "-f".to_owned(),
            self.root.join("compose.yml").display().to_string(),
        ];
        args.extend(suffix.iter().map(|s| (*s).to_owned()));
        args
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn default_logs_keep_exact_existing_docker_arguments() {
    let fixture = Fixture::new();
    let output = fixture.run(&[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.args(),
        fixture.expected(&["logs", "--tail", "200", "--follow", "api", "worker"])
    );
    assert_eq!(output.stdout, b"log for worker\n");

    let output = fixture.run(&["--tail", "50", "--no-follow"]);
    assert!(output.status.success());
    assert_eq!(
        fixture.args(),
        fixture.expected(&["logs", "--tail", "50", "api", "worker"])
    );
}

#[test]
fn service_logs_only_request_exactly_that_service() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--service", "api", "--tail", "12", "--no-follow"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.args(),
        fixture.expected(&["logs", "--tail", "12", "api"])
    );
    assert_eq!(output.stdout, b"log for api\n");
    let output = fixture.run(&["--service", "worker", "--follow"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.args(),
        fixture.expected(&["logs", "--tail", "200", "--follow", "worker"])
    );
    assert_eq!(output.stdout, b"log for worker\n");
}

#[test]
fn unknown_service_reports_valid_names_without_invoking_docker() {
    let fixture = Fixture::new();
    for name in ["absent", "API", "api-extra"] {
        let output = fixture.run(&["--service", name]);
        assert!(!output.status.success(), "{name} unexpectedly succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(name) && stderr.contains("api") && stderr.contains("worker"),
            "{stderr}"
        );
        assert!(!fixture.root.join("args").exists());
    }
}

#[test]
fn since_is_passed_as_a_separate_argument_with_its_original_value() {
    let fixture = Fixture::new();
    for since in ["2h", "2026-09-24T08:00:00Z"] {
        let output = fixture.run(&["--service", "api", "--since", since, "--no-follow"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fixture.args(),
            fixture.expected(&["logs", "--tail", "200", "--since", since, "api"])
        );
    }
    let output = fixture.run(&["--since", "1h"]);
    assert!(output.status.success());
    assert_eq!(
        fixture.args(),
        fixture.expected(&["logs", "--tail", "200", "--follow", "--since", "1h", "api", "worker"])
    );
}

#[test]
fn explicit_follow_and_no_follow_cannot_be_combined() {
    let fixture = Fixture::new();
    let output = fixture.run(&["--follow", "--no-follow"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    assert!(!fixture.root.join("args").exists());
}

#[test]
fn following_logs_stops_docker_on_terminal_interrupt() {
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_yard"))
        .args([
            "--projects-dir",
            fixture.root.join("projects").to_str().unwrap(),
            "logs",
            "demo",
            "--service",
            "api",
            "--follow",
        ])
        .env("FAKE_ROOT", &fixture.root)
        .env("FAKE_WAIT", "1")
        .env(
            "PATH",
            format!(
                "{}:{}",
                fixture.root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .process_group(0)
        .spawn()
        .unwrap();
    let active = fixture.root.join("active");
    let started = Instant::now();
    while !active.exists() && started.elapsed() < Duration::from_secs(3) {
        thread::sleep(Duration::from_millis(10));
    }
    if !active.exists() {
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let _ = child.wait();
        panic!("fake docker never started");
    }
    // A terminal sends SIGINT to the whole foreground process group, not just Yard.
    assert_eq!(unsafe { libc::kill(-(child.id() as i32), libc::SIGINT) }, 0);
    let interrupted = Instant::now();
    while interrupted.elapsed() < Duration::from_secs(3) {
        if let Some(status) = child.try_wait().unwrap() {
            if !active.exists() {
                assert!(!status.success());
                return;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.wait();
    panic!("Yard or Docker remained after the terminal interrupt");
}

#[test]
fn docker_failure_is_reported_with_nonzero_exit_and_original_stderr() {
    let fixture = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_yard"))
        .args([
            "--projects-dir",
            fixture.root.join("projects").to_str().unwrap(),
            "logs",
            "demo",
            "--service",
            "api",
        ])
        .env("FAKE_ROOT", &fixture.root)
        .env("FAKE_FAIL", "1")
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
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("docker failed"));
    assert!(stderr.contains("status 17"));
}
