//! P1-7 (analysis 2026-11-09, BUG-CLI-02 / MANUT-01 / MANUT-02) — end to end
//! through the real binary on `--engine v2`: `config.fail_on_error: false`
//! lets a cmd step exit non-zero without failing the run (its exit code stays
//! visible to a gate), and `config.timeout` is enforced. Before the fix the
//! v1→v2 adapter dropped both.
//!
//! Needs a reachable PostgreSQL at `DATABASE_URL` (like `cli_execute.rs`);
//! without it each test prints `[skip]` and returns.

use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::str::contains;
use uuid::Uuid;

fn run(yaml: &str) -> Option<assert_cmd::assert::Assert> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("[skip] DATABASE_URL not set");
        return None;
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wf.yaml");
    std::fs::write(&path, yaml).unwrap();
    let assert = Command::cargo_bin("stepyard")
        .unwrap()
        .args([
            "execute",
            path.to_str().unwrap(),
            "--no-sandbox",
            "--engine",
            "v2",
            "--verbose",
        ])
        .env("DATABASE_URL", url)
        .env(
            "STEPYARD_TENANT",
            format!("cli-v2-config-{}", Uuid::new_v4()),
        )
        .env("STEPYARD_NO_FILE_LOGS", "1")
        .timeout(Duration::from_secs(120))
        .assert();
    Some(assert)
}

const TOLERATED: &str = r#"
name: tolerated-failure
steps:
  - name: lint
    type: cmd
    run: "echo lint-output; exit 3"
    config:
      fail_on_error: false
  - name: saw_exit_code
    type: gate
    condition: "{{ steps.lint.exit_code == 3 }}"
    on_pass: continue
    on_fail: fail
    message: "the gate must see the real exit code"
  - name: after
    type: cmd
    run: "echo reached-after"
"#;

#[test]
fn fail_on_error_false_lets_the_run_continue_and_keeps_the_exit_code() {
    let Some(assert) = run(TOLERATED) else { return };
    assert.success().stdout(contains("reached-after"));
}

#[test]
fn without_fail_on_error_false_a_non_zero_exit_still_fails_the_run() {
    let Some(assert) = run(&TOLERATED.replace(
        "      fail_on_error: false\n",
        "      fail_on_error: true\n",
    )) else {
        return;
    };
    assert.failure();
}

#[test]
fn a_parallel_sub_step_with_fail_on_error_false_does_not_abort_its_siblings() {
    let yaml = r#"
name: tolerated-parallel
steps:
  - name: fan
    type: parallel
    steps:
      - name: flaky
        type: cmd
        run: "exit 1"
        config:
          fail_on_error: false
      - name: steady
        type: cmd
        run: "echo steady-ok"
  - name: after
    type: cmd
    run: "echo reached-after"
"#;
    let Some(assert) = run(yaml) else { return };
    assert.success().stdout(contains("reached-after"));
}

#[test]
fn config_timeout_is_enforced_on_cmd_steps() {
    let yaml = r#"
name: timed
steps:
  - name: slow
    type: cmd
    run: "sleep 30"
    config:
      timeout: "1s"
"#;
    let started = Instant::now();
    let Some(assert) = run(yaml) else { return };
    assert.failure();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "a 1s timeout must not wait for the 30s sleep (took {:?})",
        started.elapsed()
    );
}
