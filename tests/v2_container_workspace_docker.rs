//! P1-9 (analysis 2026-11-09, BUG-HARNESS-02 / SEG-RUST-08) — end to end on
//! `--engine v2` with the Docker container workspace: the project is copied
//! into the session container, steps run there, and the changes are copied
//! back to the host when the run ends.
//!
//! Needs a Docker daemon with the `stepyard-sandbox:latest` image and a
//! PostgreSQL at `DATABASE_URL`, so the tests are `#[ignore]`d like the
//! other Docker tests: `STEPYARD_TEST_DOCKER=1 cargo test --test
//! v2_container_workspace_docker -- --ignored`.

use std::path::Path;
use std::process::Command as StdCommand;
use std::time::Duration;

use assert_cmd::Command;
use uuid::Uuid;

fn git(dir: &Path, args: &[&str]) {
    let out = StdCommand::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A throwaway git project with one committed file per path in `tracked`.
fn project(tracked: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.email", "t@example.invalid"]);
    git(dir.path(), &["config", "user.name", "t"]);
    std::fs::write(dir.path().join("README.md"), "project\n").unwrap();
    for rel in tracked {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // YAML files must stay parseable: `.stepyard/defaults.yaml` is read by
        // the CLI before the run starts.
        let body = if rel.ends_with(".yaml") {
            "vars: {}\n".to_string()
        } else {
            format!("tracked {rel}\n")
        };
        std::fs::write(&path, body).unwrap();
    }
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-qm", "init"]);
    dir
}

fn run_v2_docker(dir: &Path, yaml: &str) -> Option<std::process::Output> {
    if std::env::var("STEPYARD_TEST_DOCKER").as_deref() != Ok("1") {
        eprintln!("[skip] STEPYARD_TEST_DOCKER!=1");
        return None;
    }
    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL is required with STEPYARD_TEST_DOCKER=1");
    let wf = dir.join(".wf.yaml");
    std::fs::write(&wf, yaml).unwrap();
    Some(
        Command::cargo_bin("stepyard")
            .unwrap()
            .args([
                "execute",
                wf.to_str().unwrap(),
                "--engine",
                "v2",
                "--sandbox-runtime",
                "docker",
            ])
            .current_dir(dir)
            .env("DATABASE_URL", url)
            .env(
                "STEPYARD_TENANT",
                format!("v2-docker-ws-{}", Uuid::new_v4()),
            )
            .env("STEPYARD_NO_FILE_LOGS", "1")
            .timeout(Duration::from_secs(300))
            .output()
            .expect("run stepyard"),
    )
}

/// BUG-HARNESS-02: a cmd timeout used to `docker rm -f` the session
/// container *before* the copy-back, so everything earlier steps changed in
/// /workspace was lost.
#[test]
#[ignore = "needs a Docker daemon and Postgres; STEPYARD_TEST_DOCKER=1 cargo test -- --ignored"]
fn a_cmd_timeout_still_copies_earlier_changes_back_to_the_host() {
    let dir = project(&[]);
    let yaml = r#"
name: timeout-keeps-work
steps:
  - name: make
    type: cmd
    run: "echo kept > made-by-step-one.txt"
  - name: hang
    type: cmd
    run: "sleep 60"
    config:
      timeout: "3s"
"#;
    let Some(out) = run_v2_docker(dir.path(), yaml) else {
        return;
    };
    assert!(
        !out.status.success(),
        "the timed-out run must fail: {out:?}"
    );
    let made = dir.path().join("made-by-step-one.txt");
    assert!(
        made.exists(),
        "work done before the timeout must reach the host; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read_to_string(made).unwrap(), "kept\n");
}

/// SEG-RUST-08: copy-in excludes `target`, `node_modules`, `.stepyard`, …
/// by path component, but the container's git index still lists tracked
/// files under them, so `git status` reported them as deleted and copy-out
/// removed them from the host.
#[test]
#[ignore = "needs a Docker daemon and Postgres; STEPYARD_TEST_DOCKER=1 cargo test -- --ignored"]
fn tracked_files_under_excluded_directories_survive_the_copy_back() {
    let tracked = [
        ".stepyard/defaults.yaml",
        "src/target/mod.rs",
        "web/node_modules/keep.js",
    ];
    let dir = project(&tracked);
    let yaml = r#"
name: excluded-paths
steps:
  - name: touch
    type: cmd
    run: "echo changed > README.md"
"#;
    let Some(out) = run_v2_docker(dir.path(), yaml) else {
        return;
    };
    assert!(
        out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
        "changed\n",
        "the real change must still be copied back"
    );
    for rel in tracked {
        assert!(
            dir.path().join(rel).exists(),
            "{rel} was deleted from the host by the copy-back"
        );
    }
}

/// Same as the cmd timeout, for a timed-out `parallel` sub-step (the third
/// place that destroyed the container before the copy-back).
#[test]
#[ignore = "needs a Docker daemon and Postgres; STEPYARD_TEST_DOCKER=1 cargo test -- --ignored"]
fn a_parallel_sub_step_timeout_still_copies_earlier_changes_back() {
    let dir = project(&[]);
    let yaml = r#"
name: parallel-timeout-keeps-work
steps:
  - name: make
    type: cmd
    run: "echo kept > made-before-parallel.txt"
  - name: fan
    type: parallel
    steps:
      - name: hang
        type: cmd
        run: "sleep 60"
        config:
          timeout: "3s"
"#;
    let Some(out) = run_v2_docker(dir.path(), yaml) else {
        return;
    };
    assert!(
        !out.status.success(),
        "the timed-out run must fail: {out:?}"
    );
    assert!(
        dir.path().join("made-before-parallel.txt").exists(),
        "work done before the parallel timeout must reach the host; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// BUG-MOTOR-01: copy-out compared the container's working tree with its own
/// `HEAD`, so a file a step committed inside the container (fix-test.yaml
/// commits before pushing) looked unchanged and never reached the host.
#[test]
#[ignore = "needs a Docker daemon and Postgres; STEPYARD_TEST_DOCKER=1 cargo test -- --ignored"]
fn work_committed_inside_the_container_is_copied_back() {
    let dir = project(&[]);
    let yaml = r#"
name: commit-inside
steps:
  - name: commit
    type: cmd
    run: "echo committed > committed.txt && git add committed.txt && git -c user.name=t -c user.email=t@example.invalid commit -qm inside"
  - name: also_uncommitted
    type: cmd
    run: "echo loose > loose.txt"
"#;
    let Some(out) = run_v2_docker(dir.path(), yaml) else {
        return;
    };
    assert!(
        out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("committed.txt"))
            .ok()
            .as_deref(),
        Some("committed\n"),
        "a file committed inside the container must reach the host"
    );
    assert!(
        dir.path().join("loose.txt").exists(),
        "uncommitted work still copied"
    );
}
