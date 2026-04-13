//! End-to-end integration tests for `minion execute --engine v2` (Story 2.4).
//!
//! These tests shell out to the built `minion` binary via `assert_cmd` and
//! assume a live PostgreSQL reachable at `DATABASE_URL`. Without it, each
//! test prints a skip line and returns — matching the pattern used by the
//! harness stress test in `crates/minion-harness/tests/`.

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

    Command::cargo_bin("minion")
        .unwrap()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--no-sandbox",
            "--engine",
            "v2",
        ])
        .env("DATABASE_URL", &url)
        .env("MINION_TENANT", &tenant)
        .assert()
        .success();
}

/// Rejection path: a workflow with a gate step cannot run on v2 yet; the CLI
/// must exit non-zero and name the unsupported step type on stderr.
#[test]
fn v2_rejects_workflow_with_unsupported_step_type() {
    let Some(url) = db_url() else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let tenant = format!("cli-execute-{}", Uuid::new_v4());

    Command::cargo_bin("minion")
        .unwrap()
        .args([
            "execute",
            "workflows/hello-world.yaml",
            "--no-sandbox",
            "--engine",
            "v2",
        ])
        .env("DATABASE_URL", &url)
        .env("MINION_TENANT", &tenant)
        .assert()
        .failure()
        .stderr(predicates::str::contains("not yet supported"));
}

/// Default `--engine` value is `v1` — no flag must still run the legacy
/// path. We pick a cmd-only workflow to keep the smoke fast and avoid AI
/// step requirements.
#[test]
fn default_engine_is_v1_and_still_works() {
    let Some(url) = db_url() else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let tenant = format!("cli-execute-{}", Uuid::new_v4());

    Command::cargo_bin("minion")
        .unwrap()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--no-sandbox",
        ])
        .env("DATABASE_URL", &url)
        .env("MINION_TENANT", &tenant)
        .assert()
        .success();
}
