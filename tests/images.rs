use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join("yard-image-tests").join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for dir in ["bin", "projects", "state"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join("bin/docker"), r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_ROOT/docker.log"
case "$*" in
  'image ls --format {{json .}}')
    if [ -f "$FAKE_ROOT/fail-list" ]; then exit 9; fi
    printf '%s\n' '{"Repository":"demo-api","Tag":"aaaaaaaaaaaa"}' '{"Repository":"demo-api","Tag":"bbbbbbbbbbbb"}' '{"Repository":"demo-api","Tag":"cccccccccccc"}' '{"Repository":"demo-api","Tag":"dddddddddddd"}' '{"Repository":"other-api","Tag":"cccccccccccc"}' '{"Repository":"foreign","Tag":"cccccccccccc"}' ;;
  'container ls -q --no-trunc')
    if [ -f "$FAKE_ROOT/fail-containers" ]; then exit 9; fi
    if [ -f "$FAKE_ROOT/late-running" ]; then
      if [ -f "$FAKE_ROOT/late-running-seen" ]; then printf 'container1\n'; else : > "$FAKE_ROOT/late-running-seen"; fi
    fi
    if [ -f "$FAKE_ROOT/running" ]; then printf 'container1\n'; fi ;;
  'container inspect --format {{.Image}} container1')
    if [ -f "$FAKE_ROOT/fail-container-inspect" ]; then exit 9; fi
    printf 'sha256:demo-api-cccccccccccc\n' ;;
  'image inspect demo-api:aaaaaaaaaaaa'|'image inspect demo-api:bbbbbbbbbbbb'|'image inspect demo-api:cccccccccccc'|'image inspect demo-api:dddddddddddd'|'image inspect other-api:cccccccccccc')
    if [ -f "$FAKE_ROOT/fail-inspect" ]; then exit 9; fi
    if [ -f "$FAKE_ROOT/empty-inspect" ]; then exit 0; fi
    if [ -f "$FAKE_ROOT/invalid-inspect" ]; then printf '[]\n'; exit 0; fi
    ref=${3#*:}; repo=${3%%:*}; printf '[{"Id":"sha256:%s-%s","Size":1024,"RepoTags":["%s"]}]\n' "$repo" "$ref" "$3" ;;
  'image rm demo-api:cccccccccccc'|'image rm demo-api:dddddddddddd')
    if [ -f "$FAKE_ROOT/fail-remove" ]; then exit 9; fi ;;
  *) exit 77 ;;
esac
"#).unwrap();
        fs::set_permissions(root.join("bin/docker"), fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self { root };
        fixture.project("demo", "demo-api");
        fixture.project("other", "other-api");
        fixture
    }

    fn project(&self, name: &str, image: &str) {
        fs::write(self.root.join(format!("projects/{name}.toml")), format!(
            "repo = \"{}\"\nbranch = \"main\"\n[compose]\ndirectory = \"{}\"\nfile = \"compose.yml\"\nenv_file = \"app.env\"\nservice = \"api\"\n[image]\nname = \"{image}\"\ntag_env = \"APP_TAG\"\n", self.root.display(), self.root.display()
        )).unwrap();
        let state = if name == "demo" {
            format!(
                "{{\"current\":{},\"previous\":{}}}",
                Self::release(image, "aaaaaaaaaaaa"),
                Self::release(image, "bbbbbbbbbbbb")
            )
        } else {
            format!(
                "{{\"current\":{},\"previous\":null}}",
                Self::release(image, "cccccccccccc")
            )
        };
        fs::write(self.root.join(format!("state/{name}.json")), state).unwrap();
    }

    fn release(image: &str, tag: &str) -> String {
        format!("{{\"revision\":\"{tag}\",\"tag\":\"{tag}\",\"deployed_at_unix\":1,\"services\":[{{\"name\":\"api\",\"image\":\"{image}:{tag}\"}}]}}")
    }

    fn run(&self, flags: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_yard"))
            .args([
                "--projects-dir",
                self.root.join("projects").to_str().unwrap(),
                "--state-dir",
                self.root.join("state").to_str().unwrap(),
                "images",
            ])
            .args(flags)
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

    fn log(&self) -> String {
        fs::read_to_string(self.root.join("docker.log")).unwrap_or_default()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn inventory_keeps_both_releases_and_never_claims_foreign_images() {
    let fixture = Fixture::new();
    let output = fixture.run(&[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    for image in [
        "demo-api:aaaaaaaaaaaa",
        "demo-api:bbbbbbbbbbbb",
        "other-api:cccccccccccc",
    ] {
        assert!(text.contains(&format!("keep {image}")), "{text}");
    }
    for image in ["demo-api:cccccccccccc", "demo-api:dddddddddddd"] {
        assert!(text.contains(&format!("candidate {image}")), "{text}");
    }
    assert!(text.contains("2048"), "{text}");
    assert!(!text.contains("foreign:"), "{text}");
    assert!(!text.contains("candidate other-api:"), "{text}");
    assert!(!fixture.log().contains("image rm"));
}

#[test]
fn prune_requires_both_flags_and_only_removes_candidates() {
    let fixture = Fixture::new();
    assert!(fixture.run(&["--prune"]).status.success());
    assert!(!fixture.log().contains("image rm"));
    assert!(!fixture.run(&["--yes"]).status.success());
    assert!(!fixture.log().contains("image rm"));
    let output = fixture.run(&["--prune", "--yes"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = fixture.log();
    assert!(log.contains("image rm demo-api:cccccccccccc"), "{log}");
    assert!(log.contains("image rm demo-api:dddddddddddd"), "{log}");
    assert!(!log.contains("image rm demo-api:aaaaaaaaaaaa"));
    assert!(!log.contains("image rm demo-api:bbbbbbbbbbbb"));
    assert!(!log.contains("image rm other-api:"));
    assert!(!log.contains("image rm foreign:"));
}

#[test]
fn running_image_is_never_removed_even_when_confirmed() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("running"), "").unwrap();
    let output = fixture.run(&["--prune", "--yes"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.log().contains("image rm demo-api:cccccccccccc"));
    assert!(fixture.log().contains("image rm demo-api:dddddddddddd"));
}

#[test]
fn container_starting_after_inventory_prevents_removal() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("late-running"), "").unwrap();
    assert!(fixture.run(&["--prune", "--yes"]).status.success());
    assert!(!fixture.log().contains("image rm demo-api:cccccccccccc"));
    assert!(fixture.log().contains("image rm demo-api:dddddddddddd"));
}

#[test]
fn invalid_state_or_inspection_blocks_all_deletion() {
    for failure in [
        "state",
        "fail-inspect",
        "fail-containers",
        "fail-container-inspect",
        "fail-list",
        "empty-inspect",
        "invalid-inspect",
    ] {
        let fixture = Fixture::new();
        if failure == "state" {
            fs::write(fixture.root.join("state/other.json"), "not-json").unwrap();
        } else {
            fs::write(fixture.root.join(failure), "").unwrap();
            if failure == "fail-container-inspect" {
                fs::write(fixture.root.join("running"), "").unwrap();
            }
        }
        let output = fixture.run(&["--prune", "--yes"]);
        assert!(
            !output.status.success(),
            "{failure}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            !fixture.log().contains("image rm"),
            "{failure}: {}",
            fixture.log()
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("refus"),
            "{failure}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn docker_removal_error_stops_before_next_candidate() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("fail-remove"), "").unwrap();
    assert!(!fixture.run(&["--prune", "--yes"]).status.success());
    assert!(fixture.log().contains("image rm demo-api:cccccccccccc"));
    assert!(!fixture.log().contains("image rm demo-api:dddddddddddd"));
}

#[test]
fn incomplete_release_or_pending_deploy_blocks_prune() {
    for state in [
        r#"{"current":{"revision":"aaaaaaaaaaaa","tag":"aaaaaaaaaaaa","deployed_at_unix":1},"previous":null}"#,
        r#"{"current":{"revision":"aaaaaaaaaaaa","tag":"aaaaaaaaaaaa","deployed_at_unix":1,"services":[{"name":"api","image":"demo-api:aaaaaaaaaaaa"}]},"previous":null,"pending":{"revision":"dddddddddddd","tag":"dddddddddddd","deployed_at_unix":1}}"#,
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("state/demo.json"), state).unwrap();
        let output = fixture.run(&["--prune", "--yes"]);
        assert!(!output.status.success());
        assert!(!fixture.log().contains("image rm"));
        assert!(String::from_utf8_lossy(&output.stderr).contains("demo"));
    }
}
