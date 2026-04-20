//! Story 2.1 — Every Engine subscribes to the same shutdown broadcast.
//!
//! Given a single `Arc<broadcast::Sender<()>>` cloned into each
//! `HarnessConfig`, building N Engines must produce exactly N active
//! receivers on that Sender, and `shutdown_tx.send(()).unwrap()` must report
//! that the broadcast reached every receiver exactly once. This is the
//! infrastructure proof the D2/D4 architecture relies on (no DashMap, no
//! static registry — D1).
//!
//! Runs on real tokio time. The story AC's literal wording says
//! `#[tokio::test(start_paused = true)]`, but that fights sqlx: the pool's
//! connect timeout is a tokio timer and never resolves while the clock is
//! paused (same conflict Story 1.4's `step_timeout.rs` documents). This test
//! has zero time-dependent logic — no `tokio::time::sleep`, no timer races —
//! so the Rule 7a invariant (deterministic, no time-waste) is satisfied on
//! real time without `start_paused`.
//!
//! Skipped gracefully if `MINION_HARNESS_DATABASE_URL` is unset.

use std::sync::Arc;

use stepyard_harness::{Engine, HarnessConfig, Step, Workflow};
use stepyard_sandbox_orchestrator::{MockLifecycle, SandboxLifecycle};
use stepyard_session::{migrate, Session};
use sqlx::postgres::PgPoolOptions;
use tokio::sync::broadcast;
use uuid::Uuid;

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

#[tokio::test]
async fn every_engine_subscribes_to_shared_shutdown_tx() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };

    // `main()` would own this Sender for the whole process; the test
    // impersonates main — one channel, cloned into every HarnessConfig.
    let (raw_tx, _) = broadcast::channel::<()>(16);
    let shutdown_tx = Arc::new(raw_tx);

    assert_eq!(
        shutdown_tx.receiver_count(),
        0,
        "sanity: no subscribers before any Engine is built"
    );

    let tenant = format!("broadcast-plumbing-{}", Uuid::new_v4());
    let workflow = Workflow::new(
        "broadcast-plumbing".to_string(),
        vec![Step::cmd("noop".to_string(), "true".to_string())],
    );

    let lifecycle_a: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
    let session_a = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("session a");
    let config_a = HarnessConfig {
        tenant_id: tenant.clone(),
        shutdown_tx: shutdown_tx.clone(),
        ..HarnessConfig::default()
    };
    let _engine_a = Engine::new(config_a, session_a, workflow.clone(), lifecycle_a);

    let lifecycle_b: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
    let session_b = Session::new(&pool, Uuid::new_v4(), tenant.clone())
        .await
        .expect("session b");
    let config_b = HarnessConfig {
        tenant_id: tenant.clone(),
        shutdown_tx: shutdown_tx.clone(),
        ..HarnessConfig::default()
    };
    let _engine_b = Engine::new(config_b, session_b, workflow.clone(), lifecycle_b);

    // Each Engine::new / with_executor must have called
    // `config.shutdown_tx.subscribe()` exactly once — the Sender should now
    // report two active receivers.
    assert_eq!(
        shutdown_tx.receiver_count(),
        2,
        "two Engines must produce two receivers"
    );

    // Fire the broadcast. The `usize` returned by `send` is the count of
    // receivers that observed the message — Tokio's own definition of
    // "delivered" for a broadcast Sender. Asserting it equals the number of
    // Engines is the cleanest proof of "every Engine's receiver observes
    // exactly one message" without needing to reach into private fields.
    let delivered = shutdown_tx.send(()).expect("send succeeds with live receivers");
    assert_eq!(
        delivered, 2,
        "broadcast must reach every subscribed Engine exactly once"
    );
}
