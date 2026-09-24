use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "yard-status-overview-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("projects")).unwrap();
        fs::create_dir_all(root.join("state")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        let docker = root.join("bin/docker");
        fs::write(&docker, "#!/bin/sh\ncase \"$*\" in\n  *'system df'*) exit 1 ;;\n  *'ps --all --format json'*) printf '%s\\n' '{\"Service\":\"api\",\"State\":\"running\",\"Health\":\"healthy\",\"Image\":\"app:current\"}' ;;\n  *'exec -T api stat'*) printf '%s\\n' 42 ;;\n  *) exit 1 ;;\nesac\n").unwrap();
        fs::set_permissions(docker, fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self { root };
        fixture.manifest("demo");
        fixture
    }

    fn manifest(&self, name: &str) {
        fs::write(self.root.join(format!("projects/{name}.toml")), format!(
            "repo = {:?}\nbranch = 'main'\n[compose]\ndirectory = {:?}\nfile = 'compose.yml'\nenv_file = '.env'\nservice = 'api'\n[image]\nname = 'app'\ntag_env = 'APP_TAG'\n",
            Path::new(env!("CARGO_MANIFEST_DIR")).to_str().unwrap(), self.root.to_str().unwrap()
        )).unwrap();
    }

    fn state(&self, text: &str) {
        fs::write(self.root.join("state/demo.json"), text).unwrap();
    }

    fn run(&self, name: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yard"));
        command.args([
            "--projects-dir",
            self.root.join("projects").to_str().unwrap(),
            "--state-dir",
            self.root.join("state").to_str().unwrap(),
            "status",
        ]);
        if let Some(name) = name {
            command.arg(name);
        }
        command
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn full_state() -> &'static str {
    r#"{"current":{"revision":"current","tag":"current","deployed_at_unix":42,"status":"active","services":[{"name":"api","image":"app:current"}]},"previous":{"revision":"previous","tag":"previous","deployed_at_unix":21,"status":"superseded","services":[{"name":"api","image":"app:previous"}]},"last_backup":{"started_at_unix":42,"duration_ms":12,"result":"success","destination":"/backup"},"last_offsite":{"started_at_unix":43,"duration_ms":13,"result":"failure","destination":"remote"}}"#
}

#[test]
fn complete_project_keeps_all_existing_fields_and_adds_deployment_health_and_disk() {
    let fixture = Fixture::new();
    fixture.state(full_state());
    let manifest = fixture.root.join("projects/demo.toml");
    let mut config = fs::read_to_string(&manifest).unwrap();
    config.push_str("[backup]\ncommand = ['true']\noffsite_command = ['true']\n[service_health.api]\ntype = 'heartbeat'\npath = '/run/beat'\nmax_age_seconds = 30\n");
    fs::write(manifest, config).unwrap();
    let result = fixture.run(Some("demo"));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = String::from_utf8_lossy(&result.stdout);
    for field in [
        "Project: demo",
        "Branch:",
        "HEAD:",
        "Release",
        "current",
        "previous",
        "pending",
        "env",
        "Docker: matched",
        "Backup",
        "Local: success",
        "Off-site: failure",
        "Docker Compose",
        "api",
        "healthy",
        "app:current",
        "HOST",
        "Service health",
        "Last deployment:",
        "1970-01-01",
        "Disk (repo):",
    ] {
        assert!(text.contains(field), "missing {field}: {text}");
    }
    assert!(text.contains("measured"), "{text}");
    assert!(
        text.contains("api: Unhealthy") && text.contains("heartbeat age:"),
        "{text}"
    );
}

#[test]
fn previous_image_is_not_drift() {
    let fixture = Fixture::new();
    fixture.state(full_state());
    fs::write(fixture.root.join("bin/docker"), "#!/bin/sh\ncase \"$*\" in *'ps --all --format json'*) printf '%s\\n' '{\"Service\":\"api\",\"State\":\"running\",\"Image\":\"app:previous\"}' ;; esac\n").unwrap();
    let result = fixture.run(Some("demo"));
    assert!(result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("DRIFT"));
}

#[test]
fn legacy_release_without_service_images_uses_its_recorded_tag() {
    let fixture = Fixture::new();
    fixture.state(r#"{"current":{"revision":"current","tag":"current","deployed_at_unix":42},"previous":null}"#);
    let result = fixture.run(Some("demo"));
    assert!(result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("DRIFT"));
}

#[test]
fn legacy_release_does_not_accept_an_unrelated_image_with_the_same_tag() {
    let fixture = Fixture::new();
    fixture.state(r#"{"current":{"revision":"current","tag":"current","deployed_at_unix":42},"previous":null}"#);
    fs::write(fixture.root.join("bin/docker"), "#!/bin/sh\ncase \"$*\" in *'ps --all --format json'*) printf '%s\\n' '{\"Service\":\"api\",\"State\":\"running\",\"Health\":\"healthy\",\"Image\":\"other:current\"}' ;; esac\n").unwrap();
    let detail = String::from_utf8_lossy(&fixture.run(Some("demo")).stdout).into_owned();
    assert!(
        detail.contains("DRIFT") && detail.contains("other:current"),
        "{detail}"
    );
    let overview = String::from_utf8_lossy(&fixture.run(None).stdout).into_owned();
    assert!(
        overview.contains("ALERT demo") && overview.contains("DRIFT"),
        "{overview}"
    );
}

#[test]
fn unreadable_or_future_heartbeat_is_an_overview_alert() {
    let fixture = Fixture::new();
    fixture.state(full_state());
    let manifest = fixture.root.join("projects/demo.toml");
    let mut config = fs::read_to_string(&manifest).unwrap();
    config.push_str(
        "[service_health.api]\ntype = 'heartbeat'\npath = '/run/beat'\nmax_age_seconds = 30\n",
    );
    fs::write(manifest, config).unwrap();
    let docker = fixture.root.join("bin/docker");
    for stamp in ["garbage", "99999999999"] {
        fs::write(&docker, format!("#!/bin/sh\ncase \"$*\" in\n  *'system df'*) exit 1 ;;\n  *'ps --all --format json'*) printf '%s\\n' '{{\"Service\":\"api\",\"State\":\"running\",\"Health\":\"healthy\",\"Image\":\"app:current\"}}' ;;\n  *'exec -T api stat'*) printf '%s\\n' '{stamp}' ;;\n  *) exit 1 ;;\nesac\n")).unwrap();
        let overview = String::from_utf8_lossy(&fixture.run(None).stdout).into_owned();
        assert!(
            overview.contains("ALERT demo") && overview.contains("api=Unknown"),
            "{stamp}: {overview}"
        );
        let detail = String::from_utf8_lossy(&fixture.run(Some("demo")).stdout).into_owned();
        assert!(
            detail.contains("Heartbeat missing, unreadable or in the future"),
            "{stamp}: {detail}"
        );
    }
}

#[test]
fn unreadable_state_is_never_reported_as_no_deployment_or_healthy() {
    let fixture = Fixture::new();
    fixture.state("not json");
    let result = fixture.run(None);
    let text = String::from_utf8_lossy(&result.stdout);
    assert!(
        text.contains("demo") && text.contains("ALERT") && text.contains("state unreadable"),
        "{text}"
    );
    assert!(
        !text.contains("no deployment") && !text.contains("healthy"),
        "{text}"
    );
    let detail = fixture.run(Some("demo"));
    assert!(!detail.status.success());
    assert!(String::from_utf8_lossy(&detail.stderr).contains("state"));
}

#[test]
fn invalid_project_manifest_is_reported_without_hiding_other_projects() {
    let fixture = Fixture::new();
    fixture.state(full_state());
    fs::write(fixture.root.join("projects/broken.toml"), "not toml =").unwrap();
    let result = fixture.run(None);
    assert!(result.status.success());
    let text = String::from_utf8_lossy(&result.stdout);
    assert!(
        text.contains("ALERT broken: project not inspectable") && text.contains("demo:"),
        "{text}"
    );
}

#[test]
fn project_without_backup_is_explicit() {
    let fixture = Fixture::new();
    fixture.state(r#"{"current":null,"previous":null}"#);
    let result = fixture.run(Some("demo"));
    assert!(result.status.success());
    let text = String::from_utf8_lossy(&result.stdout);
    assert!(
        text.contains("No backup recorded") && text.contains("Off-site: not configured"),
        "{text}"
    );
}

#[test]
fn running_image_outside_current_and_previous_is_drift() {
    let fixture = Fixture::new();
    fixture.state(full_state());
    fs::write(fixture.root.join("bin/docker"), "#!/bin/sh\ncase \"$*\" in *'ps --all --format json'*) printf '%s\\n' '{\"Service\":\"api\",\"State\":\"running\",\"Health\":\"healthy\",\"Image\":\"app:rogue\"}' ;; esac\n").unwrap();
    let detail = fixture.run(Some("demo"));
    let text = String::from_utf8_lossy(&detail.stdout);
    assert!(
        text.contains("DRIFT") && text.contains("app:rogue") && text.contains("Docker: MISMATCH"),
        "{text}"
    );
    let overview = String::from_utf8_lossy(&fixture.run(None).stdout).into_owned();
    assert!(
        overview.contains("ALERT") && overview.contains("DRIFT"),
        "{overview}"
    );
}

#[test]
fn overview_has_one_line_per_project_with_alert_for_stopped_service() {
    let fixture = Fixture::new();
    fixture.state(full_state());
    fixture.manifest("second");
    fs::write(fixture.root.join("bin/docker"), "#!/bin/sh\ncase \"$*\" in *'ps --all --format json'*) printf '%s\\n' '{\"Service\":\"api\",\"State\":\"exited\",\"Image\":\"app:current\"}' ;; esac\n").unwrap();
    let text = String::from_utf8_lossy(&fixture.run(None).stdout).into_owned();
    assert!(
        text.contains("demo") && text.contains("second") && text.contains("ALERT"),
        "{text}"
    );
}
