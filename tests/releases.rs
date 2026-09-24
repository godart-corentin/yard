use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
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

    fn run(&self, command: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_yard"))
            .args([
                "--projects-dir",
                self.root.join("projects").to_str().unwrap(),
                "--state-dir",
                self.root.join("state").to_str().unwrap(),
                command,
                "demo",
            ])
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
  *" config --format json "*) printf '{"services":{"api":{"image":"demo-api:%s"},"worker":{"image":"demo-worker:%s"}}}\n' "$IMAGE_TAG" "$IMAGE_TAG" ;;
  *" image inspect "*) exit 0 ;;
  *" ps --format json "*)
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
    assert!(String::from_utf8_lossy(&result.stderr).contains("temporary"));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "victim-intact\n");
    assert_eq!(
        fs::read_to_string(fixture.root.join(".env")).unwrap(),
        "IMAGE_TAG=old\n"
    );
    assert!(fs::symlink_metadata(&legacy)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_dir(&fixture.root)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        1
    );
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
