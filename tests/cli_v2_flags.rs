//! P1-6 (analysis 2026-11-09, BUG-CLI-07 / MANUT-07 / ARQ-02) — `--engine v2`
//! (the default) used to accept and silently ignore v1-only flags: `--repo`
//! ran the workflow on the current directory, `--resume` started over and
//! `--json` printed decorative text. They are now refused before any sandbox,
//! image or database work, so these tests need neither Docker nor Postgres.

use std::time::Duration;

use assert_cmd::Command;
use predicates::str::contains;

fn stepyard() -> Command {
    let mut cmd = Command::cargo_bin("stepyard").unwrap();
    cmd.timeout(Duration::from_secs(60))
        .env_remove("DATABASE_URL")
        .env_remove("STEPYARD_HARNESS_DATABASE_URL")
        .env_remove("STEPYARD_SESSION_DATABASE_URL")
        .env("STEPYARD_NO_FILE_LOGS", "1");
    cmd
}

#[test]
fn v2_refuses_flags_it_would_otherwise_ignore() {
    for flag in [
        &["--repo", "acme/app"][..],
        &["--resume", "greet"][..],
        &["--json"][..],
        &["--dry-run"][..],
    ] {
        let mut args = vec!["execute", "workflows/hello-world-cmd.yaml", "--no-sandbox"];
        args.extend_from_slice(flag);
        stepyard()
            .args(&args)
            .assert()
            .failure()
            .stderr(contains(format!("does not support {}", flag[0])))
            .stderr(contains("--engine v1"));
    }
}

#[test]
fn v2_lists_every_unsupported_flag_at_once() {
    stepyard()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--engine",
            "v2",
            "--repo",
            "acme/app",
            "--json",
        ])
        .assert()
        .failure()
        .stderr(contains("does not support --repo, --json"));
}

#[test]
fn timeout_flag_warns_that_it_is_ignored() {
    // The run itself fails later (no database here); the warning comes first.
    stepyard()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--no-sandbox",
            "--timeout",
            "5",
        ])
        .assert()
        .stderr(contains(
            "warning: --timeout is not implemented and is ignored",
        ));
}
