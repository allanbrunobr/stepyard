//! End-to-end integration tests for `stepyard execute --engine v2` (Story 2.4).
//!
//! These tests shell out to the built `stepyard` binary via `assert_cmd` and
//! assume a live PostgreSQL reachable at `DATABASE_URL`. Without it, each
//! test prints a skip line and returns — matching the pattern used by the
//! harness stress test in `crates/stepyard-harness/tests/`.

use assert_cmd::Command;
use uuid::Uuid;

fn db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

/// Happy path: cmd-only workflow runs through v2 harness to completion.
#[test]
fn v2_executes_cmd_only_workflow_to_completion() {
    let Some(url) = db_url() else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    // Per-run tenant so concurrent/back-to-back test invocations do not see
    // each other's rows (pitfall #3 from PROMPT_STORY_2_4.md).
    let tenant = format!("cli-execute-{}", Uuid::new_v4());

    Command::cargo_bin("stepyard")
        .unwrap()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--no-sandbox",
            "--engine",
            "v2",
        ])
        .env("DATABASE_URL", &url)
        .env("STEPYARD_TENANT", &tenant)
        .assert()
        .success();
}

/// Rejection path: v2 does not support `--dry-run` yet.
#[test]
fn v2_rejects_dry_run() {
    let Some(url) = db_url() else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let tenant = format!("cli-execute-{}", Uuid::new_v4());

    Command::cargo_bin("stepyard")
        .unwrap()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--no-sandbox",
            "--engine",
            "v2",
            "--dry-run",
        ])
        .env("DATABASE_URL", &url)
        .env("STEPYARD_TENANT", &tenant)
        .assert()
        .failure()
        .stderr(predicates::str::contains("does not support --dry-run"));
}

/// Legacy v1 path still works when explicitly requested.
#[test]
fn v1_engine_still_works_when_explicit() {
    let Some(url) = db_url() else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let tenant = format!("cli-execute-{}", Uuid::new_v4());

    Command::cargo_bin("stepyard")
        .unwrap()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--no-sandbox",
            "--engine",
            "v1",
        ])
        .env("DATABASE_URL", &url)
        .env("STEPYARD_TENANT", &tenant)
        .assert()
        .success();
}

/// Default `--engine` value is `v2` — no flag must run the harness path.
#[test]
fn default_engine_is_v2_and_still_works() {
    let Some(url) = db_url() else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let tenant = format!("cli-execute-{}", Uuid::new_v4());

    Command::cargo_bin("stepyard")
        .unwrap()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--no-sandbox",
        ])
        .env("DATABASE_URL", &url)
        .env("STEPYARD_TENANT", &tenant)
        .assert()
        .success();
}
