//! P2 / pre-PR contradiction (2026-11-10): dashboard dispatch runs `--engine v1`
//! and sends `--var BRANCH=…`; docs/REMOTE.md told workflows to read
//! `{{ vars.BRANCH }}`, which only v2 exposed. Measured before the fix: v1
//! failed with "Variable `vars.BRANCH` not found". Both names must render.
//!
//! Needs `DATABASE_URL` (v1 records a session); prints `[skip]` without it.

use std::time::Duration;

use assert_cmd::Command;
use predicates::str::contains;
use uuid::Uuid;

#[test]
fn v1_renders_both_vars_and_args_for_cli_variables() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wf = dir.path().join("wf.yaml");
    std::fs::write(
        &wf,
        "name: probe\nsteps:\n  - name: p\n    type: cmd\n    run: \"echo v={{ vars.BRANCH | shell_quote }} a={{ args.BRANCH | shell_quote }}\"\n",
    )
    .unwrap();
    Command::cargo_bin("stepyard")
        .unwrap()
        .args([
            "execute",
            wf.to_str().unwrap(),
            "--engine",
            "v1",
            "--no-sandbox",
            "--verbose",
            "--var",
            "BRANCH=feat/x",
            "--",
            "t",
        ])
        .env("DATABASE_URL", url)
        .env("STEPYARD_TENANT", format!("cli-v1-vars-{}", Uuid::new_v4()))
        .env("STEPYARD_NO_FILE_LOGS", "1")
        .timeout(Duration::from_secs(120))
        .assert()
        .success()
        .stdout(contains("v=feat/x a=feat/x"));
}
