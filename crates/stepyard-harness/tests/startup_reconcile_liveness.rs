//! P1-1 (analysis 2026-11-09, ARQ-01 / BUG-CLI-01 / BUG-HARNESS-01) — the
//! startup reconcile must not fail sessions whose owner is still alive.
//!
//! Before the fix, every `stepyard execute` marked EVERY `running` session in
//! the database as `failed` and destroyed its container, so a second process
//! against the same database killed the first one's work.
//!
//! These tests live in their own binary: cargo runs test binaries one after
//! another, so the global reconcile here cannot race the seeded sessions of
//! `startup_reconcile.rs`. Inside this file the assertions are on the status
//! of each test's own session, never on report counters, because the two
//! tests may reconcile each other's rows.
//!
//! They need `STEPYARD_HARNESS_DATABASE_URL`; without it they are reported as
//! ignored-by-skip (printed `[skip]`), like the other DB tests of this crate.
//! Phase 2 shells out to `docker ps`; when Docker is unreachable reconcile
//! only logs a warning, so no Docker daemon is required.

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use stepyard_harness::startup::{reconcile, SessionLease, LEASE_GRACE};
use stepyard_sandbox_orchestrator::DockerLifecycle;
use stepyard_session::{migrate, Session, SessionStatus};
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await
        .expect("reach DB");
    migrate(&pool).await.expect("migrations ok");
    Some(pool)
}

async fn backdate(pool: &sqlx::PgPool, id: Uuid) {
    sqlx::query("UPDATE sessions SET started_at = NOW() - INTERVAL '1 hour' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("backdate session");
}

async fn status_of(pool: &sqlx::PgPool, id: Uuid) -> SessionStatus {
    Session::load(pool, id.into())
        .await
        .expect("reload session")
        .status()
}

async fn crash_recovery_events(pool: &sqlx::PgPool, id: Uuid) -> usize {
    let session = Session::load(pool, id.into())
        .await
        .expect("reload session");
    session
        .replay()
        .await
        .expect("replay")
        .iter()
        .filter(|e| {
            e.payload.get("event").and_then(|v| v.as_str()) == Some("signal_received")
                && e.payload.get("signal").and_then(|v| v.as_str()) == Some("crash_recovery")
        })
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconcile_spares_a_session_whose_owner_holds_the_lease_and_fails_it_once_released() {
    let Some(pool) = pool().await else {
        assert!(
            std::env::var_os("STEPYARD_REQUIRE_DB").is_none(),
            "STEPYARD_HARNESS_DATABASE_URL not set but STEPYARD_REQUIRE_DB is set: a database test must not skip"
        );
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };
    let tenant = format!("liveness-{}", Uuid::new_v4());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let id = *session.id().as_uuid();
    // Past the grace window: only the lease can protect it now.
    backdate(&pool, id).await;

    let lease = SessionLease::acquire(&pool, id).await.expect("take lease");
    assert_eq!(lease.session_id(), id);

    let lifecycle = DockerLifecycle::default();
    let report = reconcile(&pool, &lifecycle).await.expect("reconcile ok");
    assert!(
        report.sessions_spared >= 1,
        "the leased session must be counted as spared, got {report:?}"
    );
    assert_eq!(
        status_of(&pool, id).await,
        SessionStatus::Running,
        "a session whose owner holds the lease must stay running"
    );
    assert_eq!(
        crash_recovery_events(&pool, id).await,
        0,
        "no crash_recovery event may be appended to a live session"
    );

    // A second lease on the same session is refused while the first lives.
    let second = SessionLease::acquire(&pool, id).await;
    assert!(
        second.is_err(),
        "the lease must be exclusive, got {second:?}"
    );

    // Owner goes away (release == what PostgreSQL does when the process dies).
    lease.release().await.expect("release lease");
    reconcile(&pool, &lifecycle).await.expect("reconcile ok");
    assert_eq!(
        status_of(&pool, id).await,
        SessionStatus::Failed,
        "once the owner is gone the session is reconciled to failed"
    );
    assert_eq!(crash_recovery_events(&pool, id).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconcile_spares_a_fresh_session_inside_the_grace_window() {
    let Some(pool) = pool().await else {
        assert!(
            std::env::var_os("STEPYARD_REQUIRE_DB").is_none(),
            "STEPYARD_HARNESS_DATABASE_URL not set but STEPYARD_REQUIRE_DB is set: a database test must not skip"
        );
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };
    assert!(
        LEASE_GRACE >= Duration::from_secs(10),
        "grace must cover slow startups"
    );
    let tenant = format!("liveness-{}", Uuid::new_v4());
    let mut session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let id = *session.id().as_uuid();

    // No lease yet (the owner is between INSERT and acquire): still spared.
    let lifecycle = DockerLifecycle::default();
    reconcile(&pool, &lifecycle).await.expect("reconcile ok");
    assert_eq!(
        status_of(&pool, id).await,
        SessionStatus::Running,
        "a session inside the grace window must not be reconciled"
    );
    assert_eq!(crash_recovery_events(&pool, id).await, 0);

    // Leave the shared database clean for the other test binaries.
    session.fail().await.expect("cleanup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_lease_releases_it_so_a_crashed_owner_is_detected() {
    let Some(pool) = pool().await else {
        assert!(
            std::env::var_os("STEPYARD_REQUIRE_DB").is_none(),
            "STEPYARD_HARNESS_DATABASE_URL not set but STEPYARD_REQUIRE_DB is set: a database test must not skip"
        );
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };
    let id = Uuid::new_v4();
    let lease = SessionLease::acquire(&pool, id).await.expect("take lease");
    // Simulates the process dying: no explicit release, the connection drops.
    drop(lease);
    // The server notices the closed socket asynchronously. Instead of polling,
    // block on the lock server-side: `pg_advisory_lock` returns as soon as the
    // dropped holder's backend goes away. The timeout bounds a regression.
    let mut conn = pool.acquire().await.expect("probe connection");
    let key = stepyard_harness::startup::session_lease_key(id);
    tokio::time::timeout(
        Duration::from_secs(20),
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(key)
            .execute(&mut *conn),
    )
    .await
    .expect("lease must become free after its holder drops it")
    .expect("pg_advisory_lock");
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .execute(&mut *conn)
        .await
        .expect("unlock");
}
