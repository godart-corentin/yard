use std::fs;
use std::io::{ErrorKind, Write};
use std::net::TcpListener;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
}

struct HealthServer {
    url: String,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for HealthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-runs")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.join("projects")).unwrap();
        fs::create_dir_all(root.join("state")).unwrap();
        Self { root }
    }

    fn manifest(&self, compose_services: &str) {
        fs::write(
            self.root.join("projects/demo.toml"),
            format!(
                "repo = \"{}\"\nbranch = \"main\"\n[compose]\ndirectory = \"{}\"\nfile = \"compose.yml\"\nenv_file = \".env\"\n{compose_services}\n[image]\nname = \"demo\"\ntag_env = \"IMAGE_TAG\"\n",
                self.root.display(), self.root.display()
            ),
        ).unwrap();
    }

    fn health_server(&self, healthy_image: Option<&str>) -> HealthServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/health", listener.local_addr().unwrap());
        let root = self.root.clone();
        let healthy_image = healthy_image.map(str::to_owned);
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let count = Arc::clone(&requests);
        let done = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        count.fetch_add(1, Ordering::Relaxed);
                        let healthy = healthy_image.as_ref().map_or(true, |image| {
                            fs::read_to_string(root.join("running-api"))
                                .is_ok_and(|running| running.trim() == image)
                        });
                        let status = if healthy {
                            "200 OK"
                        } else {
                            "503 Service Unavailable"
                        };
                        write!(
                            stream,
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .unwrap();
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("health listener: {error}"),
                }
            }
        });
        HealthServer {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn configure_health(&self, server: &HealthServer) {
        let path = self.root.join("projects/demo.toml");
        let mut manifest = fs::read_to_string(&path).unwrap();
        manifest.push_str(&format!(
            "[deployment]\nhealth_url = \"{}\"\nhealth_attempts = 1\nhealth_interval_seconds = 1\n",
            server.url
        ));
        fs::write(path, manifest).unwrap();
    }

    fn run(&self, command: &str) -> Output {
        if command == "rollback" {
            let state = self.state();
            let release = if state["pending"].is_null() && !state["previous"].is_null() {
                &state["previous"]
            } else if !state["current"].is_null() {
                &state["current"]
            } else {
                &state["previous"]
            };
            let target = format!("release:{}", release["tag"].as_str().unwrap());
            return self.run_with_args(command, &[&target, "--yes"]);
        }
        self.run_with_args(command, &[])
    }

    fn run_with_args(&self, command: &str, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_yard"))
            .args([
                "--projects-dir",
                self.root.join("projects").to_str().unwrap(),
                "--state-dir",
                self.root.join("state").to_str().unwrap(),
                command,
                "demo",
            ])
            .args(extra)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("FAKE_ROOT", &self.root)
            .output()
            .unwrap()
    }

    fn setup_repo(&self) {
        for args in [
            vec!["init", "--bare", "remote.git"],
            vec!["init", "-b", "main", "repo"],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&self.root)
                .output()
                .unwrap()
                .status
                .success());
        }
        let repo = self.root.join("repo");
        fs::write(repo.join("app"), "one").unwrap();
        for args in [
            vec!["add", "app"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "-m",
                "first",
            ],
            vec!["remote", "add", "origin", "../remote.git"],
            vec!["push", "-u", "origin", "main"],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success());
        }
        fs::write(self.root.join(".env"), "IMAGE_TAG=old\n").unwrap();
        fs::write(self.root.join("compose.yml"), "services: {}\n").unwrap();
        fs::create_dir_all(self.root.join("bin")).unwrap();
        let docker = self.root.join("bin/docker");
        fs::write(&docker, r#"#!/bin/sh
test -r "$FAKE_ROOT/.env" || exit 13
printf '%s | %s\n' "$*" "$IMAGE_TAG" >> "$FAKE_ROOT/docker.log"
case " $* " in
  *" config --format json "*)
    if [ -f "$FAKE_ROOT/missing-worker" ]; then
      printf '{"services":{"api":{"image":"demo-api:%s"}}}\n' "$IMAGE_TAG"
    else
      printf '{"services":{"api":{"image":"demo-api:%s"},"worker":{"image":"demo-worker:%s"}}}\n' "$IMAGE_TAG" "$IMAGE_TAG"
    fi ;;
  *" image inspect "*) exit 0 ;;
  *" ps --all --format json "*)
    for service in api worker; do
      if [ -f "$FAKE_ROOT/running-$service" ]; then
        image=$(sed -n '1p' "$FAKE_ROOT/running-$service")
        printf '{"Service":"%s","State":"running","Image":"%s"}\n' "$service" "$image"
      fi
    done ;;
  *" build "*|*" up -d "*)
    for service in api worker; do
      case " $* " in
        *" $service "*)
          if [ "$service" = "$(sed -n '1p' "$FAKE_ROOT/fail-service" 2>/dev/null)" ] && [ "$IMAGE_TAG" = "$(sed -n '1p' "$FAKE_ROOT/fail-tag" 2>/dev/null)" ]; then
            case " $* " in *" $(sed -n '1p' "$FAKE_ROOT/fail-step" 2>/dev/null) "*)
              if [ -f "$FAKE_ROOT/fail-once" ]; then rm "$FAKE_ROOT/fail-once" "$FAKE_ROOT/fail-step"; fi
              exit 9 ;;
            esac
          fi
          case " $* " in *" up -d "*) printf 'demo-%s:%s\n' "$service" "$IMAGE_TAG" > "$FAKE_ROOT/running-$service" ;; esac ;;
      esac
    done ;;
esac
exit 0
"#).unwrap();
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn repo_manifest(&self, services: &str) {
        self.manifest(services);
        let path = self.root.join("projects/demo.toml");
        let contents = fs::read_to_string(&path).unwrap().replace(
            &format!("repo = \"{}\"", self.root.display()),
            &format!("repo = \"{}\"", self.root.join("repo").display()),
        );
        fs::write(path, contents).unwrap();
    }

    fn state(&self) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(self.root.join("state/demo.json")).unwrap())
            .unwrap()
    }

    fn next_revision(&self) -> String {
        let repo = self.root.join("repo");
        fs::write(repo.join("app"), "two").unwrap();
        for args in [
            vec!["add", "app"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "-m",
                "second",
            ],
            vec!["push", "origin", "main"],
        ] {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success());
        }
        let result = Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn explicit_restore_guards_and_preserves_state() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    let backup = fixture.root.join("bin/backup-sentinel");
    fs::write(&backup, "#!/bin/sh\ntouch \"$FAKE_ROOT/backup-ran\"\n").unwrap();
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o700)).unwrap();
    let manifest = fixture.root.join("projects/demo.toml");
    let contents = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!(
            "{contents}\n[backup]\ncommand = [\"{}\"]\n",
            backup.display()
        ),
    )
    .unwrap();
    assert!(fixture.run("deploy").status.success());
    fs::remove_file(fixture.root.join("backup-ran")).unwrap();
    let before = fixture.state();
    let revision = format!("release:{}", before["previous"]["tag"].as_str().unwrap());
    assert!(!fixture.run("restore").status.success());
    assert!(!fixture
        .run_with_args("rollback", &["--yes"])
        .status
        .success());
    assert!(!fixture
        .run_with_args("restore", &[&revision])
        .status
        .success());
    assert!(!fixture
        .run_with_args("restore", &["backup:local", "--yes"])
        .status
        .success());
    assert_eq!(fixture.state(), before);
    let result = fixture.run_with_args("restore", &[&revision, "--yes"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let state = fixture.state();
    assert_eq!(state["current"]["revision"], before["previous"]["revision"]);
    assert_eq!(
        state["current"]["deployed_at_unix"],
        before["previous"]["deployed_at_unix"]
    );
    assert_eq!(state["previous"]["revision"], before["current"]["revision"]);
    assert!(state["pending"].is_null());
    let snapshots: Vec<_> = fs::read_dir(fixture.root.join("state"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".restore-")
        })
        .collect();
    assert_eq!(snapshots.len(), 2);
    assert!(snapshots
        .iter()
        .all(|path| fs::metadata(path).unwrap().permissions().mode() & 0o777 == 0o600));
    assert!(snapshots
        .iter()
        .any(|path| fs::read_to_string(path)
            .ok()
            .is_some_and(|text| serde_json::from_str::<serde_json::Value>(&text).ok()
                == Some(before.clone()))));
    let audit = fs::read_to_string(fixture.root.join("state/demo.restore.jsonl")).unwrap();
    assert_eq!(audit.lines().count(), 10);
    assert!(audit.contains("Data backups cannot"));
    assert!(audit.contains("success"));
    assert!(
        !fixture.root.join("backup-ran").exists(),
        "restore must not invoke the configured backup command"
    );
    assert!(!fs::read_to_string(fixture.root.join("docker.log"))
        .unwrap()
        .contains(" run --rm "));
    let log = fixture.run("restore-log");
    assert!(log.status.success());
    assert_eq!(String::from_utf8_lossy(&log.stdout), audit);
}

#[test]
fn unknown_restore_target_names_available_releases_without_activation() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    assert!(fixture.run("deploy").status.success());
    let state = fixture.state();
    let known = format!("release:{}", state["previous"]["tag"].as_str().unwrap());
    let before_docker = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    for target in ["release:unknown", "not-a-git-revision"] {
        let result = fixture.run_with_args("restore", &[target, "--yes"]);
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains(&known), "{error}");
        assert_eq!(fixture.state(), state);
    }
    assert_eq!(
        fs::read_to_string(fixture.root.join("docker.log")).unwrap(),
        before_docker
    );
}

#[test]
fn restore_refuses_recorded_data_service_before_activation_or_snapshot() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    assert!(fixture.run("deploy").status.success());
    let mut state = fixture.state();
    let target = format!("release:{}", state["previous"]["tag"].as_str().unwrap());
    state["previous"]["services"] = serde_json::json!([{
        "name": "postgres", "image": format!("demo-postgres:{}", state["previous"]["tag"].as_str().unwrap())
    }]);
    fs::write(
        fixture.root.join("state/demo.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    let before_docker = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    let result = fixture.run_with_args("restore", &[&target, "--yes"]);
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("postgres") && error.contains("api"),
        "{error}"
    );
    assert_eq!(fixture.state(), state);
    assert_eq!(
        fs::read_to_string(fixture.root.join("docker.log")).unwrap(),
        before_docker
    );
    assert!(!fs::read_dir(fixture.root.join("state"))
        .unwrap()
        .any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".restore-")
        }));
    let audit = fs::read_to_string(fixture.root.join("state/demo.restore.jsonl")).unwrap();
    assert!(audit.contains("postgres") && audit.contains("refused_or_failed"));
}

#[test]
fn restore_refuses_data_service_in_recovery_release() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    assert!(fixture.run("deploy").status.success());
    let mut state = fixture.state();
    let target = format!("release:{}", state["previous"]["tag"].as_str().unwrap());
    state["current"]["services"][0]["name"] = "postgres".into();
    fs::write(
        fixture.root.join("state/demo.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    let before_docker = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    let result = fixture.run_with_args("restore", &[&target, "--yes"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("postgres"));
    assert_eq!(fixture.state(), state);
    assert_eq!(
        fs::read_to_string(fixture.root.join("docker.log")).unwrap(),
        before_docker
    );
}

#[test]
fn restore_refuses_migration_service_even_if_listed_as_application() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    assert!(fixture.run("deploy").status.success());
    let mut state = fixture.state();
    let target = format!("release:{}", state["previous"]["tag"].as_str().unwrap());
    state["previous"]["services"][0]["name"] = "migrate".into();
    fs::write(
        fixture.root.join("state/demo.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    let path = fixture.root.join("projects/demo.toml");
    let manifest = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        format!("{manifest}\n[deployment]\nmigration_service = \"migrate\"\n"),
    )
    .unwrap();
    let before_docker = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    let result = fixture.run_with_args("restore", &[&target, "--yes"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("migrate"));
    assert_eq!(
        fs::read_to_string(fixture.root.join("docker.log")).unwrap(),
        before_docker
    );
}

#[test]
fn restore_refuses_migration_service_even_when_currently_whitelisted() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    assert!(fixture.run("deploy").status.success());
    let state = fixture.state();
    let target = format!("release:{}", state["previous"]["tag"].as_str().unwrap());
    let path = fixture.root.join("projects/demo.toml");
    let manifest = fs::read_to_string(&path).unwrap();
    fs::write(
        path,
        format!("{manifest}\n[deployment]\nmigration_service = \"api\"\n"),
    )
    .unwrap();
    let before_docker = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    let result = fixture.run_with_args("restore", &[&target, "--yes"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("migration service: api"));
    assert_eq!(fixture.state(), state);
    assert_eq!(
        fs::read_to_string(fixture.root.join("docker.log")).unwrap(),
        before_docker
    );
}

#[test]
fn restore_populates_legacy_release_services_and_preserves_multiple_apps() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let mut state = fixture.state();
    let target = format!("release:{}", state["previous"]["tag"].as_str().unwrap());
    state["previous"]
        .as_object_mut()
        .unwrap()
        .remove("services");
    fs::write(
        fixture.root.join("state/demo.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    let result = fixture.run_with_args("restore", &[&target, "--yes"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let restored = fixture.state();
    assert_eq!(restored["current"]["services"].as_array().unwrap().len(), 2);
    let log = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    assert!(log.contains("up -d --no-build --no-deps api | old"));
    assert!(log.contains("up -d --no-build --no-deps worker | old"));
}

#[test]
fn restore_points_report_unreadable_backup_record() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    fs::write(
        fixture.root.join("state/demo.json"),
        r#"{"current":null,"last_backup":{"result":"garbage"}}"#,
    )
    .unwrap();
    let result = fixture.run("restore-points");
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("unreadable"));
}

#[test]
fn failed_restore_recovers_original_image_and_journals_failure() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    assert!(fixture.run("deploy").status.success());
    fixture.next_revision();
    assert!(fixture.run("deploy").status.success());
    let before = fixture.state();
    let target = before["previous"]["tag"].as_str().unwrap();
    fs::write(fixture.root.join("fail-step"), "up").unwrap();
    fs::write(fixture.root.join("fail-service"), "api").unwrap();
    fs::write(fixture.root.join("fail-tag"), target).unwrap();
    let result = fixture.run_with_args("restore", &[&format!("release:{target}"), "--yes"]);
    assert!(!result.status.success());
    assert_eq!(fixture.state(), before);
    assert_eq!(
        fs::read_to_string(fixture.root.join(".env"))
            .unwrap()
            .trim(),
        format!("IMAGE_TAG={}", before["current"]["tag"].as_str().unwrap())
    );
    assert!(
        fs::read_to_string(fixture.root.join("state/demo.restore.jsonl"))
            .unwrap()
            .contains("refused_or_failed")
    );
}

#[test]
fn backup_records_local_and_offsite_outcomes_independently() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    let manifest = fixture.root.join("projects/demo.toml");
    let original = fs::read_to_string(&manifest).unwrap();
    let local = fixture.root.join("bin/local-backup");
    let offsite = fixture.root.join("bin/offsite-backup");
    fs::write(&local, "#!/bin/sh\ntest ! -f \"$FAKE_ROOT/fail-local\"\n").unwrap();
    fs::write(
        &offsite,
        "#!/bin/sh\ntouch \"$FAKE_ROOT/offsite-ran\"\ntest ! -f \"$FAKE_ROOT/fail-offsite\"\n",
    )
    .unwrap();
    for script in [&local, &offsite] {
        fs::set_permissions(script, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let backup = format!("\n[backup]\ncommand = [\"{}\"]\ndirectory = \"{}\"\noffsite_command = [\"{}\"]\noffsite_destination = \"remote:test\"\n", local.display(), fixture.root.join("archives").display(), offsite.display());
    fs::write(&manifest, format!("{original}{backup}")).unwrap();

    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let elapsed = std::time::Instant::now();
    let result = fixture.run("backup");
    let wall_ms = elapsed.elapsed().as_millis();
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let state = fixture.state();
    assert_eq!(state["last_backup"]["result"], "success");
    assert_eq!(
        state["last_backup"]["destination"],
        fixture.root.join("archives").to_string_lossy().as_ref()
    );
    let started = state["last_backup"]["started_at_unix"].as_u64().unwrap();
    assert!((before..=after).contains(&started));
    assert!(state["last_backup"]["duration_ms"].as_u64().unwrap() as u128 <= wall_ms);
    assert!(state["last_backup"].get("size_bytes").is_none());
    assert_eq!(state["last_offsite"]["result"], "success");
    assert_eq!(state["last_offsite"]["destination"], "remote:test");

    fs::write(fixture.root.join("fail-offsite"), "").unwrap();
    let result = fixture.run("backup");
    assert!(!result.status.success());
    assert_eq!(fixture.state()["last_backup"]["result"], "success");
    assert_eq!(fixture.state()["last_offsite"]["result"], "failure");
    let status = String::from_utf8_lossy(&fixture.run("status").stdout).into_owned();
    assert!(status.contains("Local: success"), "{status}");
    assert!(status.contains("Off-site: failure"), "{status}");

    fs::remove_file(fixture.root.join("fail-offsite")).unwrap();
    fs::remove_file(fixture.root.join("offsite-ran")).unwrap();
    fs::write(fixture.root.join("fail-local"), "").unwrap();
    let result = fixture.run("backup");
    assert!(!result.status.success());
    assert_eq!(fixture.state()["last_backup"]["result"], "failure");
    assert!(fixture.state()["last_offsite"].is_null());
    assert!(!fixture.root.join("offsite-ran").exists());
    let status = String::from_utf8_lossy(&fixture.run("status").stdout).into_owned();
    assert!(status.contains("Local: failure"), "{status}");
    assert!(status.contains("no copy recorded"), "{status}");

    fs::remove_file(fixture.root.join("fail-local")).unwrap();
    assert!(fixture.run("backup").status.success());
    assert_eq!(fixture.state()["last_backup"]["result"], "success");
    assert_eq!(fixture.state()["last_offsite"]["result"], "success");

    fs::remove_file(local).unwrap();
    assert!(!fixture.run("backup").status.success());
    assert_eq!(fixture.state()["last_backup"]["result"], "failure");
    assert!(fixture.state()["last_offsite"].is_null());
}

#[test]
fn backup_without_offsite_survives_deploy_and_rollback_state_writes() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    let manifest = fixture.root.join("projects/demo.toml");
    let original = fs::read_to_string(&manifest).unwrap();
    let local = fixture.root.join("bin/local-backup");
    fs::write(&local, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&local, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        &manifest,
        format!(
            "{original}\n[backup]\ncommand = [\"{}\"]\n",
            local.display()
        ),
    )
    .unwrap();
    assert!(fixture.run("backup").status.success());
    assert_eq!(fixture.state()["last_backup"]["result"], "success");
    assert!(fixture.state()["last_offsite"].is_null());
    let status = String::from_utf8_lossy(&fixture.run("status").stdout).into_owned();
    assert!(status.contains("Off-site: not configured"), "{status}");
    assert!(status.contains("unknown destination"), "{status}");
    assert!(fixture.run("deploy").status.success());
    assert_eq!(fixture.state()["last_backup"]["result"], "success");
    assert!(fixture.run("rollback").status.success());
    assert_eq!(fixture.state()["last_backup"]["result"], "success");
}

#[test]
fn failed_offsite_copy_stops_deploy_without_marking_local_backup_failed() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    let manifest = fixture.root.join("projects/demo.toml");
    let original = fs::read_to_string(&manifest).unwrap();
    let local = fixture.root.join("bin/local-backup");
    let offsite = fixture.root.join("bin/offsite-backup");
    fs::write(&local, "#!/bin/sh\nexit 0\n").unwrap();
    fs::write(&offsite, "#!/bin/sh\nexit 9\n").unwrap();
    for script in [&local, &offsite] {
        fs::set_permissions(script, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(
        &manifest,
        format!(
            "{original}\n[backup]\ncommand = [\"{}\"]\noffsite_command = [\"{}\"]\n",
            local.display(),
            offsite.display()
        ),
    )
    .unwrap();
    let result = fixture.run("deploy");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("off-site copy failed (local backup succeeded)"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("configuration error"));
    let state = fixture.state();
    assert_eq!(state["last_backup"]["result"], "success");
    assert_eq!(state["last_offsite"]["result"], "failure");
    assert!(state["pending"].is_null());
    assert!(state["current"].is_null());
}

#[test]
fn legacy_state_and_unconfigured_offsite_remain_explicit() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    fs::write(
        fixture.root.join("state/demo.json"),
        "{\"current\":null,\"previous\":null}\n",
    )
    .unwrap();
    let status = fixture.run("status");
    assert!(status.status.success());
    let output = String::from_utf8_lossy(&status.stdout);
    assert!(output.contains("No backup recorded"), "{output}");
    assert!(output.contains("Off-site: not configured"), "{output}");
}

#[test]
fn rejects_simultaneous_legacy_and_multi_service_keys() {
    let fixture = Fixture::new();
    fixture.manifest("service = \"api\"\nservices = [\"api\", \"worker\"]");
    let result = fixture.run("status");
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("compose.service and compose.services"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn rejects_empty_services_list() {
    let fixture = Fixture::new();
    fixture.manifest("services = []");
    let result = fixture.run("status");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("compose.services must not be empty"));
}

#[test]
fn rejects_duplicate_and_blank_service_names() {
    for (services, expected) in [
        ("services = [\"api\", \"api\"]", "duplicate service: api"),
        ("services = [\"api\", \"  \"]", "empty name"),
        ("services = [\"api\", \"bad name\"]", "invalid service name"),
    ] {
        let fixture = Fixture::new();
        fixture.manifest(services);
        let result = fixture.run("status");
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
    }
}

#[test]
fn deploys_legacy_single_service() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    fs::write(
        fixture.root.join(".env"),
        "IMAGE_TAG=old\nAPP_SECRET=synthetic-marker\n",
    )
    .unwrap();
    fs::set_permissions(fixture.root.join(".env"), fs::Permissions::from_mode(0o600)).unwrap();
    let result = fixture.run("deploy");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(fs::read_to_string(fixture.root.join("docker.log"))
        .unwrap()
        .contains("build api"));
    assert_eq!(fixture.state()["current"]["services"][0]["name"], "api");
    assert!(!fs::read_to_string(fixture.root.join("state/demo.json"))
        .unwrap()
        .contains("synthetic-marker"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("synthetic-marker"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("synthetic-marker"));
}

#[test]
fn deploy_rejects_preexisting_env_temp_symlink_without_touching_victim() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    let victim = fixture.root.join("victim");
    fs::write(&victim, "victim-intact\n").unwrap();
    let legacy = fixture.root.join("..env.yard.tmp");
    symlink(&victim, &legacy).unwrap();

    let result = fixture.run("deploy");
    assert!(
        !result.status.success(),
        "deploy must reject a planted temp symlink"
    );
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains(&legacy.display().to_string()), "{error}");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "victim-intact\n");
    assert_eq!(
        fs::read_to_string(fixture.root.join(".env")).unwrap(),
        "IMAGE_TAG=old\n"
    );
    assert!(fs::symlink_metadata(&legacy)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(
        !fixture.root.join("state/demo.json").exists(),
        "a refused deployment must not leave a pending release"
    );
    assert_eq!(
        fs::read_dir(&fixture.root)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        1
    );
    fs::remove_file(&legacy).unwrap();
    let deploy = fixture.run("deploy");
    assert!(
        deploy.status.success(),
        "{}",
        String::from_utf8_lossy(&deploy.stderr)
    );
    assert!(fixture.state()["pending"].is_null());
    let rollback = fixture.run("rollback");
    assert!(
        rollback.status.success(),
        "{}",
        String::from_utf8_lossy(&rollback.stderr)
    );
    assert_eq!(fixture.state()["current"]["tag"], "old");
    assert!(fixture.state()["pending"].is_null());
    assert!(!fs::read_dir(&fixture.root)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp")));
}

#[test]
fn interrupted_release_with_planted_temp_can_recover_after_removal() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    assert!(fixture.run("deploy").status.success());
    let mut state = fixture.state();
    let current = state["current"].clone();
    state["pending"] = current.clone();
    state["pending"]["status"] = "activating".into();
    let state_path = fixture.root.join("state/demo.json");
    fs::write(&state_path, serde_json::to_string(&state).unwrap()).unwrap();
    let legacy = fixture.root.join("..env.yard.tmp");
    fs::write(&legacy, "planted\n").unwrap();
    let refused = fixture.run("rollback");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains(&legacy.display().to_string()));
    assert_eq!(fixture.state(), state);
    assert_eq!(fs::read_to_string(&legacy).unwrap(), "planted\n");
    fs::remove_file(&legacy).unwrap();
    let recovered = fixture.run("rollback");
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert_eq!(fixture.state()["current"]["tag"], current["tag"]);
    assert!(fixture.state()["pending"].is_null());
    assert!(fixture.run("deploy").status.success());
}

#[test]
fn rejected_deploy_and_rollback_preserve_existing_state() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    assert!(fixture.run("deploy").status.success());
    let state = fixture.state();
    let legacy = fixture.root.join("..env.yard.tmp");
    fs::write(&legacy, "planted\n").unwrap();
    for command in ["deploy", "rollback"] {
        let refused = fixture.run(command);
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains(&legacy.display().to_string()));
        assert_eq!(fixture.state(), state);
    }
    fs::remove_file(legacy).unwrap();
    assert!(fixture.run("deploy").status.success());
    assert!(fixture.run("rollback").status.success());
    assert!(fixture.state()["pending"].is_null());
}

#[test]
fn env_symlink_refusal_does_not_create_pending_or_modify_victim() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    let env = fixture.root.join(".env");
    let victim = fixture.root.join("victim");
    fs::write(&victim, "IMAGE_TAG=old\n").unwrap();
    fs::remove_file(&env).unwrap();
    symlink(&victim, &env).unwrap();
    let refused = fixture.run("deploy");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains(&env.display().to_string()));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "IMAGE_TAG=old\n");
    assert!(!fixture.root.join("state/demo.json").exists());
}

#[test]
fn deploys_two_services_at_one_revision_with_distinct_images() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    let result = fixture.run("deploy");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let state = fixture.state();
    let tag = state["current"]["tag"].as_str().unwrap();
    assert_eq!(
        state["current"]["services"][0]["image"],
        format!("demo-api:{tag}")
    );
    assert_eq!(
        state["current"]["services"][1]["image"],
        format!("demo-worker:{tag}")
    );
    assert_eq!(state["current"]["status"], "active");
    assert!(state["pending"].is_null());
    let log = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    assert!(log.contains("build api"));
    assert!(log.contains("build worker"));
    assert!(log.contains("up -d --no-build --no-deps api"));
    assert!(log.contains("up -d --no-build --no-deps worker"));
}

#[test]
fn declared_service_missing_from_compose_fails_at_deploy_not_load() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    fs::write(fixture.root.join("missing-worker"), "").unwrap();
    assert!(fixture.run("status").status.success());
    let result = fixture.run("deploy");
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("Compose service worker requires an image reference"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.root.join("state/demo.json").exists());
}

#[test]
fn logs_pass_both_services_to_docker() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    let result = fixture.run("logs");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let log = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    assert!(
        log.contains("logs --tail 200 --follow api worker |"),
        "{log}"
    );
}

#[test]
fn healthy_http_endpoint_activates_both_services() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    let server = fixture.health_server(None);
    fixture.configure_health(&server);
    let result = fixture.run("deploy");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(server.requests.load(Ordering::Relaxed), 1);
    let state = fixture.state();
    assert_eq!(state["current"]["status"], "active");
    assert!(state["pending"].is_null());
    let log = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    assert!(log.contains("up -d --no-build --no-deps api"));
    assert!(log.contains("up -d --no-build --no-deps worker"));
}

#[test]
fn failed_http_health_restores_previous_services_and_clears_pending() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let old = fixture.state()["current"].clone();
    fixture.next_revision();
    let server = fixture.health_server(old["services"][0]["image"].as_str());
    fixture.configure_health(&server);
    let result = fixture.run("deploy");
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("service api") && error.contains("health check failed"),
        "{error}"
    );
    assert_eq!(server.requests.load(Ordering::Relaxed), 2);
    assert_eq!(fixture.state()["current"]["revision"], old["revision"]);
    assert!(fixture.state()["pending"].is_null());
    for service in ["api", "worker"] {
        let running = fs::read_to_string(fixture.root.join(format!("running-{service}"))).unwrap();
        let expected = old["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == service)
            .unwrap();
        assert_eq!(running.trim(), expected["image"].as_str().unwrap());
    }
}

#[test]
fn unverified_http_health_restoration_preserves_pending_release() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let old = fixture.state()["current"].clone();
    fixture.next_revision();
    let server = fixture.health_server(Some("never-healthy"));
    fixture.configure_health(&server);
    let result = fixture.run("deploy");
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("health check failed"));
    assert_eq!(server.requests.load(Ordering::Relaxed), 2);
    let state = fixture.state();
    assert_eq!(state["current"]["revision"], old["revision"]);
    assert_eq!(state["pending"]["status"], "activating");
    assert_ne!(state["pending"]["revision"], old["revision"]);
    let log = fs::read_to_string(fixture.root.join("docker.log")).unwrap();
    assert_eq!(
        log.matches("up -d --no-build --no-deps api").count(),
        3,
        "{log}"
    );
    assert!(!fixture.run("deploy").status.success());
}

#[test]
fn reads_v1_state_without_losing_previous_revision() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    fs::write(fixture.root.join("state/demo.json"), r#"{"current":{"revision":"abc123","tag":"abc123","deployed_at_unix":42},"previous":{"revision":"def456","tag":"def456","deployed_at_unix":21}}"#).unwrap();
    let result = fixture.run("status");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("abc123") && output.contains("def456"));
    assert_eq!(fixture.state()["previous"]["deployed_at_unix"], 21);
}

#[test]
fn upgrading_v1_state_retains_metadata_and_adds_service_images() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    fs::write(
        fixture.root.join("state/demo.json"),
        r#"{"current":{"revision":"abc123","tag":"old","deployed_at_unix":42},"previous":null}"#,
    )
    .unwrap();
    let result = fixture.run("deploy");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let state = fixture.state();
    assert_eq!(state["previous"]["revision"], "abc123");
    assert_eq!(state["previous"]["deployed_at_unix"], 42);
    assert_eq!(state["previous"]["services"][0]["image"], "demo-api:old");
    assert_eq!(state["current"]["services"][0]["name"], "api");
}

#[test]
fn second_service_failure_restores_previous_release_and_reports_runtime() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let previous = fixture.state()["current"].clone();
    let next = fixture.next_revision();
    fs::write(fixture.root.join("fail-step"), "up").unwrap();
    fs::write(fixture.root.join("fail-service"), "worker").unwrap();
    fs::write(fixture.root.join("fail-tag"), next).unwrap();
    let result = fixture.run("deploy");
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("service worker"), "{error}");
    assert!(
        error.contains("Service") && error.contains("api"),
        "{error}"
    );
    assert_eq!(fixture.state()["current"]["revision"], previous["revision"]);
    assert!(fixture.state()["pending"].is_null());
    assert_eq!(
        fs::read_to_string(fixture.root.join("running-api"))
            .unwrap()
            .trim(),
        previous["services"][0]["image"]
    );
    assert!(!fs::read_to_string(fixture.root.join("docker.log"))
        .unwrap()
        .contains("restore"));
}

#[test]
fn interrupted_activation_is_visible_and_blocks_another_deploy() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let mut state = fixture.state();
    state["pending"] = state["current"].clone();
    state["pending"]["status"] = "activating".into();
    fs::write(
        fixture.root.join("state/demo.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    fs::write(fixture.root.join("running-worker"), "demo-worker:other\n").unwrap();
    let status = fixture.run("status");
    assert!(status.status.success());
    let output = String::from_utf8_lossy(&status.stdout);
    assert!(
        output.contains("activating") && output.contains("MISMATCH") && output.contains("worker"),
        "{output}"
    );
    let deploy = fixture.run("deploy");
    assert!(!deploy.status.success());
    assert!(String::from_utf8_lossy(&deploy.stderr).contains("interrupted release"));
}

#[test]
fn rollback_restores_all_services_from_previous_release() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let first = fixture.state()["current"].clone();
    fixture.next_revision();
    assert!(fixture.run("deploy").status.success());
    let second = fixture.state()["current"].clone();
    assert_ne!(first["revision"], second["revision"]);
    assert_eq!(fixture.state()["previous"]["status"], "superseded");
    let result = fixture.run("rollback");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fixture.state()["current"]["revision"], first["revision"]);
    assert_eq!(fixture.state()["previous"]["revision"], second["revision"]);
    assert_eq!(fixture.state()["previous"]["status"], "superseded");
    assert_eq!(fixture.state()["current"]["status"], "active");
    for service in ["api", "worker"] {
        let image = fs::read_to_string(fixture.root.join(format!("running-{service}"))).unwrap();
        assert!(image.contains(first["tag"].as_str().unwrap()));
    }
    assert!(fixture.state()["pending"].is_null());
}

#[test]
fn build_failure_names_service_and_reports_running_release() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let previous = fixture.state()["current"].clone();
    let next = fixture.next_revision();
    fs::write(fixture.root.join("fail-step"), "build").unwrap();
    fs::write(fixture.root.join("fail-service"), "worker").unwrap();
    fs::write(fixture.root.join("fail-tag"), next).unwrap();
    let result = fixture.run("deploy");
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("service worker") && error.contains("demo-api:"),
        "{error}"
    );
    assert_eq!(fixture.state()["current"]["revision"], previous["revision"]);
}

#[test]
fn preexisting_temp_file_is_not_overwritten_by_state_save() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("service = \"api\"");
    let temp = fixture.root.join("state/demo.json.tmp");
    fs::write(&temp, "leftover").unwrap();
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(fixture.run("deploy").status.success());
    assert_eq!(fs::read_to_string(&temp).unwrap(), "leftover");
    let mode = fs::metadata(fixture.root.join("state/demo.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn rollback_recovers_interrupted_activation_to_last_active_release() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let mut state = fixture.state();
    let current = state["current"].clone();
    let next = fixture.next_revision();
    state["pending"] = current.clone();
    state["pending"]["tag"] = next.clone().into();
    state["pending"]["status"] = "activating".into();
    fs::write(
        fixture.root.join("state/demo.json"),
        serde_json::to_string(&state).unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.root.join("running-api"),
        format!("demo-api:{next}\n"),
    )
    .unwrap();
    let result = fixture.run("rollback");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fixture.state()["current"]["revision"], current["revision"]);
    assert!(fixture.state()["pending"].is_null());
    assert_eq!(
        fs::read_to_string(fixture.root.join("running-api"))
            .unwrap()
            .trim(),
        current["services"][0]["image"]
    );
}

#[test]
fn first_deploy_interruption_can_recover_previous_images() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    fs::write(fixture.root.join("state/demo.json"), r#"{"current":null,"previous":{"revision":"abc123","tag":"old","deployed_at_unix":42,"status":"superseded","services":[{"name":"api","image":"demo-api:old"},{"name":"worker","image":"demo-worker:old"}]},"pending":{"revision":"def456","tag":"new","deployed_at_unix":43,"status":"activating","services":[{"name":"api","image":"demo-api:new"},{"name":"worker","image":"demo-worker:new"}]}}"#).unwrap();
    fs::write(fixture.root.join("running-api"), "demo-api:new\n").unwrap();
    let result = fixture.run("rollback");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fixture.state()["current"]["tag"], "old");
    assert!(fixture.state()["pending"].is_null());
}

#[test]
fn failed_deploy_restores_recorded_tag_when_env_was_stale() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    assert!(fixture.run("deploy").status.success());
    let original = fixture.state()["current"]["tag"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::write(fixture.root.join(".env"), "IMAGE_TAG=stale\n").unwrap();
    let next = fixture.next_revision();
    fs::write(fixture.root.join("fail-step"), "up").unwrap();
    fs::write(fixture.root.join("fail-service"), "worker").unwrap();
    fs::write(fixture.root.join("fail-tag"), next).unwrap();
    assert!(!fixture.run("deploy").status.success());
    assert_eq!(
        fs::read_to_string(fixture.root.join(".env")).unwrap(),
        format!("IMAGE_TAG={original}\n")
    );
}

#[test]
fn failed_recovery_restores_active_state_after_first_deploy_interruption() {
    let fixture = Fixture::new();
    fixture.setup_repo();
    fixture.repo_manifest("services = [\"api\", \"worker\"]");
    fs::write(fixture.root.join("state/demo.json"), r#"{"current":null,"previous":{"revision":"abc123","tag":"old","deployed_at_unix":42,"status":"superseded","services":[{"name":"api","image":"demo-api:old"},{"name":"worker","image":"demo-worker:old"}]},"pending":{"revision":"def456","tag":"new","deployed_at_unix":43,"status":"activating","services":[{"name":"api","image":"demo-api:new"},{"name":"worker","image":"demo-worker:new"}]}}"#).unwrap();
    fs::write(fixture.root.join("running-api"), "demo-api:new\n").unwrap();
    fs::write(fixture.root.join("fail-step"), "up").unwrap();
    fs::write(fixture.root.join("fail-service"), "worker").unwrap();
    fs::write(fixture.root.join("fail-tag"), "old").unwrap();
    fs::write(fixture.root.join("fail-once"), "").unwrap();
    let result = fixture.run("rollback");
    assert!(!result.status.success());
    assert_eq!(fixture.state()["current"]["tag"], "old");
    assert!(fixture.state()["pending"].is_null());
}
