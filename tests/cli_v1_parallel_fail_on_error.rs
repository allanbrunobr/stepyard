//! P1-8 (analysis 2026-11-09, BUG-MOTOR-08) — end to end on `--engine v1`:
//! a `parallel` sub-step that exits non-zero fails the run under the
//! embedded `cmd.fail_on_error: true` default. It used to be recorded as
//! success because the parallel dispatcher ignored the resolved config.
//!
//! Needs `DATABASE_URL` (v1 records the session in PostgreSQL); prints
//! `[skip]` and returns without it.

use std::time::Duration;

use assert_cmd::Command;
use uuid::Uuid;

fn run_v1(yaml: &str) -> Option<assert_cmd::assert::Assert> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("[skip] DATABASE_URL not set");
        return None;
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wf.yaml");
    std::fs::write(&path, yaml).unwrap();
    Some(
        Command::cargo_bin("stepyard")
            .unwrap()
            .args([
                "execute",
                path.to_str().unwrap(),
                "--no-sandbox",
                "--engine",
                "v1",
            ])
            .env("DATABASE_URL", url)
            .env(
                "STEPYARD_TENANT",
                format!("cli-v1-parallel-{}", Uuid::new_v4()),
            )
            .env("STEPYARD_NO_FILE_LOGS", "1")
            .timeout(Duration::from_secs(120))
            .assert(),
    )
}

const FAN: &str = r#"
name: v1-parallel
steps:
  - name: fan
    type: parallel
    steps:
      - name: steady
        type: cmd
        run: "echo ok"
      - name: broken
        type: cmd
        run: "exit 101"
"#;

#[test]
fn v1_parallel_sub_step_exiting_non_zero_fails_the_run() {
    let Some(assert) = run_v1(FAN) else { return };
    assert.failure();
}

#[test]
fn v1_parallel_sub_step_with_fail_on_error_false_is_tolerated() {
    let yaml = FAN.replace(
        "        run: \"exit 101\"\n",
        "        run: \"exit 101\"\n        config:\n          fail_on_error: false\n",
    );
    let Some(assert) = run_v1(&yaml) else { return };
    assert.success();
}
