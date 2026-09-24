use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn host_command_collects_with_fake_docker_and_never_runs_mutating_commands() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("yard-host-test-{}-{unique}", std::process::id()));
    let bin = root.join("bin");
    let projects = root.join("projects");
    let state = root.join("state");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&projects).unwrap();
    fs::write(projects.join("hello.toml"), format!("repo = \"{}\"\nbranch = \"main\"\n[compose]\ndirectory = \"{}\"\nfile = \"compose.yml\"\nenv_file = \"app.env\"\nservice = \"api\"\n[image]\nname = \"hello\"\ntag_env = \"APP_TAG\"\n", root.display(), root.display())).unwrap();
    let docker = bin.join("docker");
    fs::write(&docker, "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$YARD_TEST_DOCKER_LOG\"\ncase \"$1 $2\" in\n  'system df') printf '%s\\n' '{\"Type\":\"Images\",\"Size\":\"2GB\"}' '{\"Type\":\"Containers\",\"Size\":\"1MB\"}' '{\"Type\":\"Local Volumes\",\"Size\":\"3GB\"}' ;;\n  'compose --env-file') case \"$*\" in *'ps --all --format json'*) printf '%s\\n' '{\"Service\":\"api\",\"State\":\"exited\"}' ;; *) exit 2 ;; esac ;;\n  *) exit 2 ;;\nesac\n").unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let log = root.join("docker.log");
    let output = Command::new(env!("CARGO_BIN_EXE_yard"))
        .args([
            "--projects-dir",
            projects.to_str().unwrap(),
            "--state-dir",
            state.to_str().unwrap(),
            "host",
        ])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("YARD_TEST_DOCKER_LOG", &log)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("HOST\n"));
    assert!(text.contains("images 2GB, containers 1MB, volumes 3GB"));
    assert!(text.contains("hello / api: exited [Critical]"));
    let snapshot: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state.join("host.json")).unwrap()).unwrap();
    assert_eq!(snapshot["containers"][0]["status"], "critical");
    assert_eq!(snapshot["version"], 1);
    let log_text = fs::read_to_string(log).unwrap();
    assert!(log_text
        .lines()
        .all(|line| line.starts_with("system df ") || line.starts_with("compose --env-file ")));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn status_reports_host_when_docker_and_application_env_are_unavailable() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("yard-status-test-{}-{unique}", std::process::id()));
    let bin = root.join("bin");
    let projects = root.join("projects");
    let state = root.join("state");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&projects).unwrap();
    fs::write(projects.join("hello.toml"), format!("repo = \"{}\"\nbranch = \"main\"\n[compose]\ndirectory = \"{}\"\nfile = \"compose.yml\"\nenv_file = \"missing.env\"\nservice = \"api\"\n[image]\nname = \"hello\"\ntag_env = \"APP_TAG\"\n", env!("CARGO_MANIFEST_DIR"), root.display())).unwrap();
    let docker = bin.join("docker");
    fs::write(&docker, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_yard"))
        .args([
            "--projects-dir",
            projects.to_str().unwrap(),
            "--state-dir",
            state.to_str().unwrap(),
            "status",
            "hello",
        ])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("SECRET_TOKEN", "DO_NOT_EXPOSE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("HOST\n"));
    assert!(text.contains("Docker       unavailable [Unknown]"));
    assert!(!text.contains("DO_NOT_EXPOSE"));
    assert!(!fs::read_to_string(state.join("host.json"))
        .unwrap()
        .contains("DO_NOT_EXPOSE"));
    fs::remove_dir_all(root).unwrap();
}
