//! Regression: after a signal-path cancel (SIGINT/SIGTERM broadcast fired
//! mid-step), a reloaded engine must NOT advance the workflow.
//! `progress_from_log()` must see the `step_failed` event the signal path
//! emits — this closes the "signalled session advanced after reload" gap,
//! symmetric with `step_timeout_replay.rs` and `step_cancel_replay.rs`
//! (architecture.md §D9 + NFR13).
//!
//! Skipped gracefully if `MINION_HARNESS_DATABASE_URL` is unset.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use minion_harness::{
    Engine, EngineError, HarnessConfig, Step, StepExecutor, StepOutcome, Workflow,
};
use minion_sandbox_orchestrator::{ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle};
use minion_session::{migrate, Session};
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

/// Executor whose `execute` future never resolves — forces the broadcast
/// branch to be the only way out of `Engine::step`.
struct BlockingExecutor;

#[async_trait]
impl StepExecutor for BlockingExecutor {
    async fn execute(&self, _session_id: Uuid, _step: &Step) -> Result<ExecOutput, SandboxError> {
        std::future::pending::<()>().await;
        unreachable!("pending never resolves")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reloaded_signalled_session_refuses_to_advance() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };

    let tenant = format!("reload-signal-{}", Uuid::new_v4());
    let workflow = Workflow::new(
        "reload-signal-wf".to_string(),
        vec![Step::cmd("blocked".to_string(), "sleep forever".to_string())],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let session_id = session.id();

    let (tx, _) = broadcast::channel::<()>(16);
    let shutdown_tx = Arc::new(tx);
    let shutdown_signal: Arc<OnceLock<String>> = Arc::new(OnceLock::new());

    let config = HarnessConfig {
        tenant_id: "reload-signal".into(),
        shutdown_tx: shutdown_tx.clone(),
        shutdown_signal: shutdown_signal.clone(),
        ..HarnessConfig::default()
    };

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine1 = Engine::with_executor(
        config,
        session,
        workflow.clone(),
        lifecycle.clone(),
        Arc::new(BlockingExecutor),
    );

    // Fire the broadcast shortly after `step()` starts, mirroring
    // `install_handlers`' write-before-send ordering.
    let tx_for_fire = shutdown_tx.clone();
    let slot_for_fire = shutdown_signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = slot_for_fire.set("sigterm".into());
        let _ = tx_for_fire.send(());
    });

    // First run: broadcast fires, engine emits signal_received + step_failed,
    // session flipped to cancelled, StepFailed returned.
    let first = engine1.step().await;
    assert!(
        matches!(first, Err(EngineError::StepFailed { .. })),
        "first step must return StepFailed, got {first:?}"
    );

    let events_before = engine1
        .session()
        .replay()
        .await
        .expect("replay engine1")
        .len();
    drop(engine1);

    // Reload the session and build a fresh engine — fresh shutdown broadcast
    // (never fires on reload), fresh cancel token. Terminality must live in
    // the log.
    let (tx2, _) = broadcast::channel::<()>(16);
    let config2 = HarnessConfig {
        tenant_id: "reload-signal".into(),
        shutdown_tx: Arc::new(tx2),
        shutdown_signal: Arc::new(OnceLock::new()),
        ..HarnessConfig::default()
    };
    let reloaded = Session::load(&pool, session_id)
        .await
        .expect("reload session");
    let mut engine2 = Engine::with_executor(
        config2,
        reloaded,
        workflow,
        lifecycle,
        Arc::new(BlockingExecutor),
    );

    // The reloaded engine's step() must hit the progress.has_failure
    // branch and return StepOutcome::StepFailed without executing anything
    // new. The fresh broadcast and cancel token are irrelevant — terminality
    // lives in the log, not the token.
    let outcome = engine2.step().await.expect("reloaded step returns");
    match outcome {
        StepOutcome::StepFailed { error, .. } => {
            assert!(
                error.contains("previously failed"),
                "expected 'workflow previously failed' error, got {error:?}"
            );
        }
        other => panic!("reloaded step must be StepFailed, got {other:?}"),
    }

    // No new events were appended — log is idempotent across reloads of
    // a terminal (signalled) session.
    let events_after = engine2
        .session()
        .replay()
        .await
        .expect("replay engine2")
        .len();
    assert_eq!(
        events_before, events_after,
        "reloaded step must not emit new events (before={events_before}, after={events_after})"
    );
}
