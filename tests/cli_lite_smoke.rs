//! End-to-end smoke for the Stepyard Lite profile.
//!
//! This test runs the real CLI binary with the SQLite feature profile, local
//! sandbox runtime, and file-log mirror enabled. It intentionally lives in the
//! root crate because it verifies the assembled user path rather than one
//! individual library layer.

#![cfg(feature = "sqlite")]

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use assert_cmd::Command;
use serde_json::Value;
use sqlx::sqlite::SqlitePoolOptions;
use uuid::Uuid;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("workflows")
        .join(name)
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = StdCommand::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_minimal_git_repo(path: &Path) {
    run_git(path, &["init"]);
    run_git(
        path,
        &["config", "user.email", "lite-smoke@example.invalid"],
    );
    run_git(path, &["config", "user.name", "Lite Smoke"]);
    std::fs::write(path.join("README.md"), "lite smoke\n").expect("write README");
    run_git(path, &["add", "README.md"]);
    run_git(path, &["commit", "-m", "initial"]);
}

fn only_jsonl_file(log_dir: &Path) -> PathBuf {
    let entries: Vec<PathBuf> = std::fs::read_dir(log_dir)
        .unwrap_or_else(|e| panic!("read log dir `{}`: {e}", log_dir.display()))
        .map(|entry| entry.expect("log dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one session JSONL file in {}",
        log_dir.display()
    );
    entries.into_iter().next().expect("one jsonl file")
}

fn read_jsonl(path: &Path) -> Vec<Value> {
    let body = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read jsonl `{}`: {e}", path.display()));
    body.lines()
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|e| panic!("invalid jsonl line `{line}`: {e}"))
        })
        .collect()
}

async fn read_sqlite_payloads(db_path: &Path) -> Vec<Value> {
    let url = format!("sqlite://{}", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap_or_else(|e| panic!("connect sqlite `{url}`: {e}"));

    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT payload
        FROM session_events
        ORDER BY seq ASC
        "#,
    )
    .fetch_all(&pool)
    .await
    .expect("read sqlite session events");

    rows.into_iter()
        .map(|(payload,)| {
            serde_json::from_str::<Value>(&payload)
                .unwrap_or_else(|e| panic!("invalid sqlite payload `{payload}`: {e}"))
        })
        .collect()
}

fn run_lite_workflow(
    cwd: &Path,
    db_path: &Path,
    tenant: &str,
    sandbox_runtime: Option<&str>,
    extra_args: &[&str],
    no_file_logs_env: Option<&str>,
    sandbox_env: Option<&str>,
) {
    let workflow_path = fixture("hello-world-cmd.yaml");
    let mut args = vec![
        "execute",
        workflow_path.to_str().expect("workflow path is utf-8"),
        "--engine",
        "v2",
    ];
    if let Some(runtime) = sandbox_runtime {
        args.push("--sandbox-runtime");
        args.push(runtime);
    }
    args.extend_from_slice(extra_args);

    let mut command = Command::cargo_bin("stepyard").expect("stepyard binary");
    command
        .args(args)
        .current_dir(cwd)
        .env("STEPYARD_SQLITE_PATH", db_path)
        .env("STEPYARD_TENANT", tenant);

    match no_file_logs_env {
        Some(value) => {
            command.env("STEPYARD_NO_FILE_LOGS", value);
        }
        None => {
            command.env_remove("STEPYARD_NO_FILE_LOGS");
        }
    }
    match sandbox_env {
        Some(value) => {
            command.env("STEPYARD_SANDBOX", value);
        }
        None => {
            command.env_remove("STEPYARD_SANDBOX");
        }
    }

    command.assert().success();
}

#[tokio::test]
async fn sqlite_lite_cli_runs_local_sandbox_and_writes_file_log() {
    let cwd = tempfile::tempdir().expect("tempdir");
    init_minimal_git_repo(cwd.path());

    let db_path = cwd.path().join("sessions.db");
    let tenant = format!("lite-smoke-{}", Uuid::new_v4());

    run_lite_workflow(
        cwd.path(),
        &db_path,
        &tenant,
        Some("local"),
        &[],
        None,
        None,
    );

    assert!(
        db_path.is_file(),
        "expected sqlite DB at {}",
        db_path.display()
    );

    let log_path = only_jsonl_file(&cwd.path().join(".stepyard").join("logs"));
    let log_events = read_jsonl(&log_path);
    let sqlite_payloads = read_sqlite_payloads(&db_path).await;

    assert_eq!(
        log_events.len(),
        sqlite_payloads.len(),
        "file log should mirror every persisted session event"
    );
    assert!(
        log_events
            .iter()
            .all(|event| event.get("session_id").is_some()),
        "each JSONL line should be a serialized SessionEvent"
    );
    let log_payloads: Vec<Value> = log_events
        .iter()
        .map(|event| event["payload"].clone())
        .collect();
    assert_eq!(
        log_payloads, sqlite_payloads,
        "file log payloads should match sqlite payloads exactly"
    );

    let events: Vec<&str> = sqlite_payloads
        .iter()
        .filter_map(|payload| payload.get("event").and_then(Value::as_str))
        .collect();
    assert_eq!(events.first(), Some(&"workflow_started"));
    assert_eq!(events.last(), Some(&"workflow_completed"));
    assert!(
        events.contains(&"step_completed"),
        "expected at least one completed cmd step in sqlite payloads: {events:?}"
    );
    assert!(
        sqlite_payloads.iter().any(|payload| {
            payload
                .pointer("/output/stdout")
                .and_then(Value::as_str)
                .is_some_and(|stdout| stdout.contains("Hello from v2 engine"))
        }),
        "expected cmd stdout snapshot in sqlite payloads"
    );
}

#[tokio::test]
async fn sqlite_lite_cli_no_file_logs_flag_suppresses_file_mirror_only() {
    let cwd = tempfile::tempdir().expect("tempdir");
    init_minimal_git_repo(cwd.path());

    let db_path = cwd.path().join("sessions.db");
    let tenant = format!("lite-smoke-{}", Uuid::new_v4());

    run_lite_workflow(
        cwd.path(),
        &db_path,
        &tenant,
        Some("local"),
        &["--no-file-logs"],
        None,
        None,
    );

    assert!(
        db_path.is_file(),
        "expected sqlite DB at {}",
        db_path.display()
    );
    assert!(
        !cwd.path().join(".stepyard").join("logs").exists(),
        "--no-file-logs should suppress the JSONL mirror"
    );
    assert!(
        read_sqlite_payloads(&db_path).await.iter().any(|payload| {
            payload
                .get("event")
                .and_then(Value::as_str)
                .is_some_and(|event| event == "workflow_completed")
        }),
        "SQLite event store should still record workflow completion"
    );
}

#[tokio::test]
async fn sqlite_lite_cli_no_file_logs_env_suppresses_file_mirror_only() {
    let cwd = tempfile::tempdir().expect("tempdir");
    init_minimal_git_repo(cwd.path());

    let db_path = cwd.path().join("sessions.db");
    let tenant = format!("lite-smoke-{}", Uuid::new_v4());

    run_lite_workflow(
        cwd.path(),
        &db_path,
        &tenant,
        Some("local"),
        &[],
        Some("1"),
        None,
    );

    assert!(
        db_path.is_file(),
        "expected sqlite DB at {}",
        db_path.display()
    );
    assert!(
        !cwd.path().join(".stepyard").join("logs").exists(),
        "STEPYARD_NO_FILE_LOGS=1 should suppress the JSONL mirror"
    );
    assert!(
        read_sqlite_payloads(&db_path).await.iter().any(|payload| {
            payload
                .get("event")
                .and_then(Value::as_str)
                .is_some_and(|event| event == "workflow_completed")
        }),
        "SQLite event store should still record workflow completion"
    );
}

#[tokio::test]
async fn sqlite_lite_cli_profile_default_uses_local_runtime() {
    let cwd = tempfile::tempdir().expect("tempdir");
    init_minimal_git_repo(cwd.path());

    let db_path = cwd.path().join("sessions.db");
    let tenant = format!("lite-smoke-{}", Uuid::new_v4());

    run_lite_workflow(cwd.path(), &db_path, &tenant, None, &[], None, None);

    assert!(
        read_sqlite_payloads(&db_path).await.iter().any(|payload| {
            payload
                .get("event")
                .and_then(Value::as_str)
                .is_some_and(|event| event == "workflow_completed")
        }),
        "SQLite profile should default to LocalShell and complete without Docker"
    );
}

#[tokio::test]
async fn sqlite_lite_cli_stepyard_sandbox_env_selects_local_runtime() {
    let cwd = tempfile::tempdir().expect("tempdir");
    init_minimal_git_repo(cwd.path());

    let db_path = cwd.path().join("sessions.db");
    let tenant = format!("lite-smoke-{}", Uuid::new_v4());

    run_lite_workflow(
        cwd.path(),
        &db_path,
        &tenant,
        None,
        &[],
        None,
        Some("local"),
    );

    assert!(
        read_sqlite_payloads(&db_path).await.iter().any(|payload| {
            payload
                .get("event")
                .and_then(Value::as_str)
                .is_some_and(|event| event == "workflow_completed")
        }),
        "STEPYARD_SANDBOX=local should select LocalShell and complete without Docker"
    );
}
