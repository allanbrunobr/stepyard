//! Story 2.5 — `minion session list --status` integration test.
//!
//! Seeds PG with one session in each lifecycle status (running / completed /
//! failed / cancelled), then spawns the real `minion` binary with varying
//! filter arguments and asserts stdout + exit codes.
//!
//! Skipped gracefully (printed `[skip]` + `return`) when:
//! * `MINION_HARNESS_DATABASE_URL` is unset, or
//! * the workspace `target/debug/minion` (or release) binary is not built —
//!   `cargo build --bin minion` fixes it.
//!
//! Every `tokio::process::Command` is wrapped in `tokio::time::timeout(..)`
//! per Rule 7b — `tokio::process::Command` has no `.timeout(..)` of its own,
//! so the wrap is the semantic equivalent of `assert_cmd`'s `.timeout(..)`.
//! `assert_cmd::cargo_bin` cannot be used here: it reads the
//! `CARGO_BIN_EXE_<name>` env var which cargo only sets in the crate that
//! declares `[[bin]]`, and `minion-harness` does not. We fall back to the
//! canonical workspace-root `target/debug/minion` path, following the
//! precedent in `crates/minion-harness/tests/signal_handler.rs`.

use std::path::PathBuf;
use std::time::Duration;

use stepyard_session::{migrate, Session};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const CMD_TIMEOUT: Duration = Duration::from_secs(30);

fn minion_bin() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    [
        manifest.join("../../target/debug/minion"),
        manifest.join("../../target/release/minion"),
    ]
    .into_iter()
    .find(|p| p.exists())
}

async fn pool_and_url() -> Option<(sqlx::PgPool, String)> {
    let url = std::env::var("MINION_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("reach DB");
    migrate(&pool).await.expect("migrations ok");
    Some((pool, url))
}

/// Invoke `minion <args>` with `DATABASE_URL` set, bounded by `CMD_TIMEOUT`.
async fn run_minion(bin: &PathBuf, db_url: &str, args: &[&str]) -> std::process::Output {
    tokio::time::timeout(
        CMD_TIMEOUT,
        tokio::process::Command::new(bin)
            .args(args)
            .env("DATABASE_URL", db_url)
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .expect("minion did not exit before timeout")
    .expect("minion spawn")
}

#[tokio::test(flavor = "current_thread")]
async fn session_list_filters_by_status_and_since() {
    let Some(bin) = minion_bin() else {
        eprintln!("[skip] workspace minion binary not built: `cargo build --bin minion`");
        return;
    };
    let Some((pool, db_url)) = pool_and_url().await else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };

    // ── seed: one session per status, all same tenant so we can assert
    // presence/absence by UUID without cross-test noise ───────────────────
    let tenant = format!("list-cli-{}", Uuid::new_v4());

    let running = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("running session");
    let running_id = running.id().as_uuid().to_string();

    let mut completed = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("completed session");
    completed.complete().await.expect("mark completed");
    let completed_id = completed.id().as_uuid().to_string();

    let mut failed = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("failed session");
    failed.fail().await.expect("mark failed");
    let failed_id = failed.id().as_uuid().to_string();

    let mut cancelled = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("cancelled session");
    cancelled.cancel().await.expect("mark cancelled");
    let cancelled_id = cancelled.id().as_uuid().to_string();

    // ── `--status running` → only running rows appear ─────────────────────
    let out = run_minion(&bin, &db_url, &["session", "list", "--status", "running"]).await;
    assert!(
        out.status.success(),
        "exit 0 expected, status={:?}, stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&running_id),
        "running session UUID should appear in stdout; stdout={stdout}",
    );
    for id in [&completed_id, &failed_id, &cancelled_id] {
        assert!(
            !stdout.contains(id),
            "non-running session {id} must not appear under --status running; stdout={stdout}",
        );
    }

    // ── `--status completed` → only completed rows appear ─────────────────
    let out = run_minion(&bin, &db_url, &["session", "list", "--status", "completed"]).await;
    assert!(out.status.success(), "exit 0 expected for --status completed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&completed_id),
        "completed session UUID should appear; stdout={stdout}",
    );
    assert!(
        !stdout.contains(&running_id),
        "running session UUID must not appear under --status completed",
    );

    // ── invalid `--status foobar` → clap parse error, exit code 2 ─────────
    let out = run_minion(&bin, &db_url, &["session", "list", "--status", "foobar"]).await;
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap parse error should yield exit code 2; status={:?}, stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );

    // ── invalid `--since <garbage>` → clap parse error at parse time ──────
    // AC: `invalid durations produce a clap-layer error at parse time (not
    // runtime)`. `humantime::Duration::from_str` surfaces the error through
    // clap's value parsing, so the DB is never touched.
    let out = run_minion(
        &bin,
        &db_url,
        &["session", "list", "--status", "running", "--since", "notaduration"],
    )
    .await;
    assert_eq!(
        out.status.code(),
        Some(2),
        "invalid --since must yield clap exit code 2; status={:?}, stderr={}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );

    // ── `--since 1h` filters out old sessions ─────────────────────────────
    // Seed an additional running session and backdate its `started_at` to
    // two hours ago. A `--since 1h` query must include the recent `running`
    // session (seeded above) and must exclude this backdated one.
    let old_running = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("old running session");
    let old_running_id = old_running.id().as_uuid().to_string();
    sqlx::query("UPDATE sessions SET started_at = NOW() - INTERVAL '2 hours' WHERE id = $1")
        .bind(old_running.id().as_uuid())
        .execute(&pool)
        .await
        .expect("backdate started_at");

    let out = run_minion(
        &bin,
        &db_url,
        &["session", "list", "--status", "running", "--since", "1h"],
    )
    .await;
    assert!(out.status.success(), "exit 0 expected for --since 1h");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&running_id),
        "recent running session must appear under --since 1h; stdout={stdout}",
    );
    assert!(
        !stdout.contains(&old_running_id),
        "backdated running session must be filtered out by --since 1h; stdout={stdout}",
    );

    // ── ordering: started_at DESC ─────────────────────────────────────────
    // Seed a newer running session after the original. Under DESC order,
    // the newer UUID must appear before the original in stdout.
    let newer = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("newer running session");
    let newer_id = newer.id().as_uuid().to_string();

    let out = run_minion(&bin, &db_url, &["session", "list", "--status", "running"]).await;
    assert!(out.status.success(), "exit 0 expected for ordering check");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let newer_pos = stdout.find(&newer_id).expect("newer session present");
    let older_pos = stdout.find(&running_id).expect("original session present");
    assert!(
        newer_pos < older_pos,
        "started_at DESC: newer session should precede older in stdout; \
         newer_pos={newer_pos}, older_pos={older_pos}, stdout={stdout}",
    );
}
