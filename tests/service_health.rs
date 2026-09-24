use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn run_host(root: &Path, stamp: &Path, stopped: bool) -> (String, serde_json::Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_yard"))
        .args([
            "--projects-dir",
            root.join("projects").to_str().unwrap(),
            "--state-dir",
            root.join("state").to_str().unwrap(),
            "host",
        ])
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("YARD_TEST_STAMP", stamp)
        .env("YARD_TEST_STOPPED", if stopped { "1" } else { "0" })
        .env("YARD_HEARTBEAT_CRIT_MULTIPLIER", "3")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let snapshot =
        serde_json::from_slice(&fs::read(root.join("state/host.json")).unwrap()).unwrap();
    (text, snapshot)
}

#[test]
fn heartbeat_age_missing_source_and_stopped_worker_are_not_silent_successes() {
    let root = std::env::temp_dir().join(format!(
        "yard-probes-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    let script = root.join("bin/docker");
    fs::write(&script, "#!/bin/sh\ncase \"$1 $2\" in\n  'system df') printf '%s\\n' '{\"Type\":\"Images\",\"Size\":\"2GB\"}' '{\"Type\":\"Containers\",\"Size\":\"1MB\"}' '{\"Type\":\"Local Volumes\",\"Size\":\"3GB\"}' ;;\n  'compose --env-file') case \"$*\" in\n    *'ps --all --format json'*) if [ \"$YARD_TEST_STOPPED\" = 1 ]; then state=exited; else state=running; fi; printf '[{\"Service\":\"worker\",\"State\":\"%s\"},{\"Service\":\"other\",\"State\":\"running\"}]\\n' \"$state\" ;;\n    *'exec -T worker stat -c %Y -- /run/yard/heartbeat'*) cat \"$YARD_TEST_STAMP\" ;;\n    *) exit 2 ;;\n  esac ;;\n  *) exit 2 ;;\nesac\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.join("projects/demo.toml"), format!("repo = '{}'\nbranch = 'main'\n[compose]\ndirectory = '{}'\nfile = 'compose.yml'\nenv_file = '.env'\nservices = ['worker', 'other']\n[image]\nname = 'demo'\ntag_env = 'APP_TAG'\n[service_health.worker]\ntype = 'heartbeat'\npath = '/run/yard/heartbeat'\nmax_age_seconds = 30\n", env!("CARGO_MANIFEST_DIR"), root.display())).unwrap();
    let stamp = root.join("stamp");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    fs::write(&stamp, (now - 5).to_string()).unwrap();
    let (text, fresh) = run_host(&root, &stamp, false);
    assert_eq!(fresh["services"][0]["status"], "healthy");
    assert!(fresh["services"][0]["age_seconds"].as_u64().unwrap() <= 10);
    assert_eq!(fresh["services"][1]["status"], "unknown");
    assert!(text.contains("demo / other: No service probe configured [Unknown]"));
    assert!(text.contains("demo / worker: ") && text.contains("since heartbeat [Healthy]"));
    let status = Command::new(env!("CARGO_BIN_EXE_yard"))
        .args([
            "--projects-dir",
            root.join("projects").to_str().unwrap(),
            "--state-dir",
            root.join("state").to_str().unwrap(),
            "status",
            "demo",
        ])
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("YARD_TEST_STAMP", &stamp)
        .env("YARD_TEST_STOPPED", "0")
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_text = String::from_utf8(status.stdout).unwrap();
    assert!(status_text.contains("Service health\n"));
    assert!(
        status_text.contains("demo / worker:") && status_text.contains("since heartbeat [Healthy]")
    );

    fs::write(&stamp, (now - 50).to_string()).unwrap();
    let (_, stale) = run_host(&root, &stamp, false);
    assert_eq!(stale["services"][0]["status"], "degraded");
    assert!((50..=55).contains(&stale["services"][0]["age_seconds"].as_u64().unwrap()));
    fs::write(&stamp, (now - 120).to_string()).unwrap();
    let (_, expired) = run_host(&root, &stamp, false);
    assert_eq!(expired["services"][0]["status"], "unhealthy");
    fs::remove_file(&stamp).unwrap();
    let (_, missing) = run_host(&root, &stamp, false);
    assert_eq!(missing["services"][0]["status"], "unknown");
    assert!(missing["services"][0]["age_seconds"].is_null());
    let (_, stopped) = run_host(&root, &stamp, true);
    assert_eq!(stopped["services"][0]["status"], "unhealthy");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn http_probe_reports_latency_and_failure_without_docker_health_assumptions() {
    let root = std::env::temp_dir().join(format!(
        "yard-http-probe-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("projects")).unwrap();
    let script = root.join("bin/docker");
    fs::write(&script, "#!/bin/sh\ncase \"$1 $2\" in\n 'system df') printf '%s\\n' '{\"Type\":\"Images\",\"Size\":\"2GB\"}' '{\"Type\":\"Containers\",\"Size\":\"1MB\"}' '{\"Type\":\"Local Volumes\",\"Size\":\"3GB\"}' ;;\n 'compose --env-file') printf '%s\\n' '[{\"Service\":\"api\",\"State\":\"running\"}]' ;;\n *) exit 2 ;;\nesac\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    let manifest = |port| {
        format!("repo = '{}'\nbranch = 'main'\n[compose]\ndirectory = '{}'\nfile = 'compose.yml'\nenv_file = '.env'\nservice = 'api'\n[image]\nname = 'demo'\ntag_env = 'APP_TAG'\n[service_health.api]\ntype = 'http'\nurl = 'http://127.0.0.1:{port}/health'\ntimeout_ms = 300\n", root.display(), root.display())
    };
    fs::write(root.join("projects/demo.toml"), manifest(address.port())).unwrap();
    let (_, healthy) = run_host(&root, &root.join("unused"), false);
    server.join().unwrap();
    assert_eq!(healthy["services"][0]["status"], "healthy");
    assert!(healthy["services"][0]["latency_ms"].is_u64());
    let bad_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let bad_port = bad_listener.local_addr().unwrap().port();
    let bad_server = thread::spawn(move || {
        let (mut stream, _) = bad_listener.accept().unwrap();
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    fs::write(root.join("projects/demo.toml"), manifest(bad_port)).unwrap();
    let (_, bad) = run_host(&root, &root.join("unused"), false);
    bad_server.join().unwrap();
    assert_eq!(bad["services"][0]["status"], "unhealthy");
    assert_eq!(bad["services"][0]["message"], "HTTP non-success response");

    let slow_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let slow_port = slow_listener.local_addr().unwrap().port();
    let slow_server = thread::spawn(move || {
        let (mut stream, _) = slow_listener.accept().unwrap();
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        thread::sleep(std::time::Duration::from_millis(500));
    });
    fs::write(root.join("projects/demo.toml"), manifest(slow_port)).unwrap();
    let (_, timed_out) = run_host(&root, &root.join("unused"), false);
    slow_server.join().unwrap();
    assert_eq!(timed_out["services"][0]["status"], "unhealthy");
    assert!(timed_out["services"][0]["latency_ms"].as_u64().unwrap() >= 250);

    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    fs::write(root.join("projects/demo.toml"), manifest(port)).unwrap();
    let (_, failed) = run_host(&root, &root.join("unused"), false);
    assert_eq!(failed["services"][0]["status"], "unhealthy");
    assert!(failed["services"][0]["latency_ms"].is_u64());
    fs::remove_dir_all(root).unwrap();
}
