//! Story 2.4 — startup reconcile integration test.
//!
//! Seed scenario:
//! 1. A PG session with `status = 'running'` whose container does NOT exist
//!    on the Docker daemon.
//! 2. An orphan Docker container with name `minion-session-<uuid>` whose
//!    UUID has NO matching session row.
//!
//! After `reconcile(&pool, &lifecycle)`:
//! - The seeded session is `failed`; its event log contains
//!   `SignalReceived { signal: "crash_recovery" }`.
//! - The orphan container is destroyed.
//! - A second `reconcile()` call observes no running sessions, no orphan
//!   containers, and does not append duplicate `crash_recovery` events
//!   (NFR12 idempotency).
//!
//! Skipped gracefully (printed `[skip]` + `return`) when:
//! * `MINION_HARNESS_DATABASE_URL` is unset.
//! * the `docker` CLI is missing or cannot reach the daemon.
//!
//! Every `tokio::process::Command` is wrapped in `tokio::time::timeout(..)`
//! per Rule 7b — `tokio::process::Command` has no `.timeout(..)` method of
//! its own, so the `tokio::time::timeout` wrap is the semantic equivalent
//! of `assert_cmd`'s `.timeout(..)`.
//!
//! A RAII `ContainerCleanup` guard tears down every spawned orphan
//! container on drop — including on panic — so a failing test never leaves
//! Docker daemon pollution behind.

use std::time::Duration;

use minion_harness::startup::reconcile;
use minion_sandbox_orchestrator::DockerLifecycle;
use minion_session::{migrate, Session, SessionStatus};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

const CMD_TIMEOUT: Duration = Duration::from_secs(30);

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("MINION_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("reach DB");
    migrate(&pool).await.expect("migrations ok");
    Some(pool)
}

async fn docker_available() -> bool {
    match tokio::time::timeout(
        CMD_TIMEOUT,
        tokio::process::Command::new("docker")
            .args(["ps"])
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => output.status.success(),
        _ => false,
    }
}

/// Synchronous cleanup so it fires in `Drop` (sync context) even when the
/// test panics. Uses `std::process::Command` because the tokio runtime is
/// likely shutting down by the time Drop runs.
struct ContainerCleanup {
    names: Vec<String>,
}

impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        for name in &self.names {
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", name])
                .output();
        }
    }
}

async fn spawn_orphan(name: &str) {
    let output = tokio::time::timeout(
        CMD_TIMEOUT,
        tokio::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                name,
                "alpine:latest",
                "sleep",
                "600",
            ])
            .output(),
    )
    .await
    .expect("docker run did not time out")
    .expect("spawn docker run");
    assert!(
        output.status.success(),
        "docker run failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );
}

async fn container_alive(name: &str) -> bool {
    let output = tokio::time::timeout(
        CMD_TIMEOUT,
        tokio::process::Command::new("docker")
            .args([
                "ps",
                "--filter",
                &format!("name=^/{name}$"),
                "--format",
                "{{.Names}}",
            ])
            .output(),
    )
    .await
    .expect("docker ps did not time out")
    .expect("docker ps ok");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().any(|line| line.trim() == name)
}

#[tokio::test(flavor = "current_thread")]
async fn reconcile_flips_orphan_session_and_destroys_orphan_container() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };
    if !docker_available().await {
        eprintln!("[skip] Docker daemon unavailable");
        return;
    }

    // ── seed ────────────────────────────────────────────────────────────
    let tenant = format!("reconcile-{}", Uuid::new_v4());
    let seeded = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("new session");
    let seeded_id = seeded.id();

    let orphan_uuid = Uuid::new_v4();
    let orphan_name = format!("minion-session-{orphan_uuid}");
    // RAII: guarantees the orphan container is torn down even if reconcile
    // panics or an assertion blows up mid-way.
    let _guard = ContainerCleanup {
        names: vec![orphan_name.clone()],
    };
    spawn_orphan(&orphan_name).await;
    assert!(
        container_alive(&orphan_name).await,
        "orphan container should exist after spawn"
    );

    // ── act: first reconcile ────────────────────────────────────────────
    let lifecycle = DockerLifecycle::default();
    let report = reconcile(&pool, &lifecycle).await.expect("reconcile ok");

    // The global DB may hold leftover `running` sessions from other tests
    // or concurrent runs — we only assert on the lower bound that reflects
    // our own seeded session / orphan container.
    assert!(
        report.sessions_reconciled >= 1,
        "should flip at least the seeded running session, got {}",
        report.sessions_reconciled,
    );
    assert!(
        report.containers_pruned >= 1,
        "should prune at least the orphan container, got {}",
        report.containers_pruned,
    );

    // ── assert: session side effect ─────────────────────────────────────
    let reloaded = Session::load(&pool, seeded_id).await.expect("reload");
    assert_eq!(
        reloaded.status(),
        SessionStatus::Failed,
        "seeded session should be failed after reconcile",
    );

    let events = reloaded.replay().await.expect("replay");
    let crash_events: Vec<_> = events
        .iter()
        .filter(|e| {
            e.payload.get("event").and_then(|v| v.as_str()) == Some("signal_received")
                && e.payload.get("signal").and_then(|v| v.as_str()) == Some("crash_recovery")
        })
        .collect();
    assert_eq!(
        crash_events.len(),
        1,
        "seeded session log should contain exactly one signal_received/crash_recovery event, got {events:?}",
    );

    // ── assert: container side effect ───────────────────────────────────
    assert!(
        !container_alive(&orphan_name).await,
        "orphan container should be destroyed",
    );

    // ── act: second reconcile (idempotency) ─────────────────────────────
    let report2 = reconcile(&pool, &lifecycle)
        .await
        .expect("reconcile idempotent");

    // The direct observable of "no state change": both counters are zero.
    // The first reconcile drained every `running` session and every orphan
    // `minion-session-*` container, so the second pass has nothing to do.
    assert_eq!(
        report2.sessions_reconciled, 0,
        "idempotent reconcile must reconcile zero sessions, got {}",
        report2.sessions_reconciled,
    );
    assert_eq!(
        report2.containers_pruned, 0,
        "idempotent reconcile must prune zero containers, got {}",
        report2.containers_pruned,
    );

    // Per-row confirmation: seeded session still `failed`, event log still
    // has exactly one `signal_received/crash_recovery` entry.
    let reloaded2 = Session::load(&pool, seeded_id).await.expect("reload idempotent");
    assert_eq!(
        reloaded2.status(),
        SessionStatus::Failed,
        "idempotent reconcile must leave seeded session in `failed`",
    );
    let events2 = reloaded2.replay().await.expect("replay idempotent");
    let crash_events2: Vec<_> = events2
        .iter()
        .filter(|e| {
            e.payload.get("event").and_then(|v| v.as_str()) == Some("signal_received")
                && e.payload.get("signal").and_then(|v| v.as_str()) == Some("crash_recovery")
        })
        .collect();
    assert_eq!(
        crash_events2.len(),
        1,
        "idempotent reconcile must not append a second crash_recovery event",
    );
}
