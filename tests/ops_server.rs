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
        let root = std::env::temp_dir().join(format!(
            "yard-ops-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("projects")).unwrap();
        fs::create_dir_all(root.join("state")).unwrap();
        fs::create_dir_all(root.join("app")).unwrap();
        let manifest = |backup: &str| {
            format!(
                "repo = '{app}'\nbranch = 'main'\n[compose]\ndirectory = '{app}'\nfile = 'compose.yml'\nenv_file = '.env'\nservice = 'api'\n[image]\nname = 'demo'\ntag_env = 'DEMO_TAG'\n{backup}",
                app = root.join("app").display()
            )
        };
        fs::write(
            root.join("projects/demo.toml"),
            manifest(
                "[backup]\ncommand = ['sh', '-c', 'echo backing-up; echo $HOME-$SECRET_LEAK']\n",
            ),
        )
        .unwrap();
        fs::write(
            root.join("projects/slow.toml"),
            manifest("[backup]\ncommand = ['sleep', '2']\n"),
        )
        .unwrap();
        fs::write(root.join("projects/plain.toml"), manifest("")).unwrap();
        fs::write(
            root.join("projects/monitor.toml"),
            "[deployment]\nhealth_url = 'http://localhost/health'\n",
        )
        .unwrap();
        // A job left running by a previous executor.
        let jobs = root.join("state/jobs");
        fs::create_dir_all(&jobs).unwrap();
        fs::write(
            jobs.join("20260101T000000Z-deadbeef.json"),
            r#"{"v":1,"id":"20260101T000000Z-deadbeef","action":"deploy","project":"demo","revision":null,"state":"running","actor":"old","created_at_unix":1,"started_at_unix":2,"finished_at_unix":null,"exit_code":null,"output_bytes":0,"error":null}"#,
        )
        .unwrap();
        fs::write(jobs.join("20260101T000000Z-deadbeef.log"), "half\n").unwrap();
        fs::write(
            jobs.join("20200101T000000Z-0ddba115.json"),
            r#"{"v":1,"id":"20200101T000000Z-0ddba115","action":"backup","project":"demo","revision":null,"state":"succeeded","actor":"old","created_at_unix":1,"started_at_unix":2,"finished_at_unix":3,"exit_code":0,"output_bytes":0,"error":null}"#,
        )
        .unwrap();
        fs::write(jobs.join("20200101T000000Z-0ddba115.log"), "").unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_yard"))
            .args(["ops-server", "--socket"])
            .arg(root.join("ops.sock"))
            .env("YARD_PROJECTS_DIR", root.join("projects"))
            .env("YARD_STATE_DIR", root.join("state"))
            .env("SECRET_LEAK", "classified")
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if root.join("ops.sock").exists() {
                return Self { root, child };
            }
            thread::sleep(Duration::from_millis(20));
        }
        child.kill().ok();
        child.wait().ok();
        panic!("ops server did not create socket");
    }

    fn raw(&self, request: &str) -> serde_json::Value {
        let mut stream = UnixStream::connect(self.root.join("ops.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    fn request(&self, request: &str) -> serde_json::Value {
        self.raw(&format!("{request}\n"))
    }

    fn job(&self, id: &str) -> serde_json::Value {
        let path = self.root.join(format!("state/jobs/{id}.json"));
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn wait(&self, id: &str) -> serde_json::Value {
        for _ in 0..500 {
            let job = self.job(id);
            if job["state"] == "succeeded" || job["state"] == "failed" {
                return job;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("job {id} did not finish");
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
fn socket_is_group_accessible_and_ping_answers() {
    let fixture = Fixture::new();
    let mode = fs::metadata(fixture.root.join("ops.sock"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o660);
    assert_eq!(
        fixture.request(r#"{"v":1,"op":"ping"}"#),
        serde_json::json!({"ok": true})
    );
}

#[test]
fn second_executor_does_not_displace_a_live_one() {
    let fixture = Fixture::new();
    let status = Command::new(env!("CARGO_BIN_EXE_yard"))
        .args(["ops-server", "--socket"])
        .arg(fixture.root.join("ops.sock"))
        .env("YARD_PROJECTS_DIR", fixture.root.join("projects"))
        .env("YARD_STATE_DIR", fixture.root.join("state"))
        .status()
        .unwrap();
    assert!(!status.success());
    assert_eq!(fixture.request(r#"{"v":1,"op":"ping"}"#)["ok"], true);
}

#[test]
fn startup_recovers_orphans_and_applies_retention() {
    let fixture = Fixture::new();
    let orphan = fixture.job("20260101T000000Z-deadbeef");
    assert_eq!(orphan["state"], "failed");
    assert_eq!(orphan["error"], "Executor restarted");
    assert_eq!(orphan["output_bytes"], 5);
    assert!(orphan["finished_at_unix"].as_u64().unwrap() > 1_700_000_000);
    assert!(!fixture
        .root
        .join("state/jobs/20200101T000000Z-0ddba115.json")
        .exists());
    assert!(!fixture
        .root
        .join("state/jobs/20200101T000000Z-0ddba115.log")
        .exists());
    // The orphan does not hold the project.
    assert_eq!(
        fixture.request(
            r#"{"v":1,"op":"start","action":"backup","project":"demo","actor":"corentin"}"#
        )["ok"],
        true
    );
}

#[test]
fn protocol_refusals_never_reach_the_cli() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join("state/demo.json"),
        r#"{"current":{"revision":"abc123","tag":"t","deployed_at_unix":1}}"#,
    )
    .unwrap();
    let oversized = format!(
        "{{\"v\":1,\"op\":\"ping\",\"actor\":\"{}\"}}\n",
        "a".repeat(5000)
    );
    assert_eq!(fixture.raw(&oversized)["error"], "refused");
    for request in [
        r#"{"v":1,"op":"start","action":"deploy","project":"demo","actor":"a","cmd":"sh"}"#,
        r#"{"v":1,"op":"start","action":"shell","project":"demo","actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"deploy","project":"monitor","actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"deploy","project":"host","actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"deploy","project":"../demo","actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"deploy","project":"nope","actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"backup","project":"plain","actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"deploy","project":"demo"}"#,
        r#"{"v":1,"op":"start","action":"deploy","project":"demo","actor":"a b"}"#,
        r#"{"v":1,"op":"start","action":"restore","project":"demo","revision":"abc123","actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"restore","project":"demo","revision":"ffffff","confirm":true,"actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"images-prune","actor":"a"}"#,
        r#"{"v":1,"op":"start","action":"host-collect","project":"demo","actor":"a"}"#,
    ] {
        let reply = fixture.request(request);
        assert_eq!(reply["ok"], false, "{request}: {reply}");
        assert_eq!(reply["error"], "refused", "{request}: {reply}");
    }
    let listed: Vec<_> = fs::read_dir(fixture.root.join("state/jobs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        listed.iter().all(|name| name.starts_with("2026")),
        "{listed:?}"
    );
}

#[test]
fn backup_job_runs_the_real_cli_with_a_clean_environment() {
    let fixture = Fixture::new();
    let reply = fixture
        .request(r#"{"v":1,"op":"start","action":"backup","project":"demo","actor":"corentin"}"#);
    assert_eq!(reply["ok"], true, "{reply}");
    let job = &reply["job"];
    let id = job["id"].as_str().unwrap();
    assert_eq!(id.len(), 25);
    assert_eq!(job["action"], "backup");
    assert_eq!(job["project"], "demo");
    assert_eq!(job["actor"], "corentin");
    assert!(["queued", "running"].contains(&job["state"].as_str().unwrap()));
    let done = fixture.wait(id);
    assert_eq!(done["state"], "succeeded", "{done}");
    assert_eq!(done["exit_code"], 0);
    let log = fs::read_to_string(fixture.root.join(format!("state/jobs/{id}.log"))).unwrap();
    assert!(log.contains("backing-up"), "{log}");
    assert!(!log.contains("classified"), "{log}");
    for extension in ["json", "log"] {
        let mode = fs::metadata(fixture.root.join(format!("state/jobs/{id}.{extension}")))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
    }
    assert_eq!(done["output_bytes"], log.len());
    assert!(fixture.root.join("state/demo.json").exists());
}

#[test]
fn a_busy_project_conflicts_with_the_running_job_id() {
    let fixture = Fixture::new();
    let first =
        fixture.request(r#"{"v":1,"op":"start","action":"backup","project":"slow","actor":"a"}"#);
    let id = first["job"]["id"].as_str().unwrap();
    let second =
        fixture.request(r#"{"v":1,"op":"start","action":"deploy","project":"slow","actor":"b"}"#);
    assert_eq!(second["ok"], false);
    assert_eq!(second["error"], "conflict");
    assert_eq!(second["running_job_id"], id);
    let global = fixture.request(r#"{"v":1,"op":"start","action":"host-collect","actor":"b"}"#);
    assert_eq!(global["error"], "conflict");
    assert_eq!(global["running_job_id"], id);
    assert_eq!(fixture.wait(id)["state"], "succeeded");
}
