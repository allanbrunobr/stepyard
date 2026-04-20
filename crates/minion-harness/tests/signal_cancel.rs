//! Story 2.3 — a shutdown broadcast received while the engine is mid-step
//! must:
//!
//! 1. Emit [`Event::SignalReceived`] with `{ signal: "sigterm" }` BEFORE
//!    tearing the sandbox down (D5 emit-before-IO).
//! 2. Call `lifecycle.destroy(&session_uuid)` (tolerantly — NFR12).
//! 3. Flip the session status to `cancelled` via `finalise_cancel`.
//! 4. Return `Err(EngineError::StepFailed { reason: SignalReceived("sigterm") })`
//!    carrying the D9 termination taxonomy.
//!
//! Uses the [`BlockingExecutor`] precedent from `step_timeout.rs` so the
//! broadcast arm is the only way the `tokio::select!` in `Engine::step` can
//! resolve.
//!
//! Runs on real tokio time — the sqlx `PgPool` connect timeout is a tokio
//! timer that never resolves while the clock is paused, so we cannot use
//! `start_paused`. Same Rule 7a deviation already documented in
//! `step_timeout.rs`.
//!
//! Skipped gracefully if `MINION_HARNESS_DATABASE_URL` is unset.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use minion_harness::{
    Engine, EngineError, HarnessConfig, Step, StepExecutor, TerminationReason, Workflow,
};
use minion_sandbox_orchestrator::{
    ExecOutput, MockCall, MockLifecycle, SandboxError, SandboxLifecycle,
};
use minion_session::{migrate, Session, SessionStatus};
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

/// Executor whose `execute` future never resolves. The step can only leave
/// the select via the cancel, timeout, or shutdown-broadcast branches —
/// this test engineers the last to fire first.
struct BlockingExecutor;

#[async_trait]
impl StepExecutor for BlockingExecutor {
    async fn execute(
        &self,
        _session_id: Uuid,
        _step: &Step,
    ) -> Result<ExecOutput, SandboxError> {
        std::future::pending::<()>().await;
        unreachable!("pending never resolves")
    }

    async fn execute_with_env(
        &self,
        session_id: Uuid,
        step: &Step,
        _env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        self.execute(session_id, step).await
    }
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_broadcast_emits_signal_received_destroys_sandbox_and_returns_step_failed() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };

    let tenant = format!("signal-cancel-{}", Uuid::new_v4());
    let workflow = Workflow::new(
        "signal-wf".to_string(),
        vec![Step::cmd(
            "blocked-step".to_string(),
            "sleep forever".to_string(),
        )],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let session_uuid = *session.id().as_uuid();
    let session_id = session.id();

    let (tx, _) = broadcast::channel::<()>(16);
    let shutdown_tx = Arc::new(tx);
    let shutdown_signal: Arc<OnceLock<String>> = Arc::new(OnceLock::new());

    let config = HarnessConfig {
        tenant_id: "signal-test".into(),
        shutdown_tx: shutdown_tx.clone(),
        shutdown_signal: shutdown_signal.clone(),
        ..HarnessConfig::default()
    };

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine = Engine::with_executor(
        config,
        session,
        workflow,
        lifecycle,
        Arc::new(BlockingExecutor),
    );

    // Fire the broadcast shortly after `step()` starts, mirroring
    // `install_handlers`' write-before-send ordering: populate the signal
    // slot first, then fire the broadcast. If the send happened before
    // the set, the select arm could read an empty `OnceLock` and fall back
    // to `"unknown"` — that fallback exists as a safety net, not as a
    // guarantee the test should tolerate.
    let tx_for_fire = shutdown_tx.clone();
    let slot_for_fire = shutdown_signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = slot_for_fire.set("sigterm".into());
        let _ = tx_for_fire.send(());
    });

    let outcome = engine.step().await;

    // ── AC1: typed error with the termination taxonomy ─────────────────
    match outcome {
        Err(EngineError::StepFailed { step_index, reason }) => {
            assert_eq!(step_index, 0);
            match reason {
                TerminationReason::SignalReceived(signal) => {
                    assert_eq!(signal, "sigterm");
                }
                other => panic!("expected SignalReceived, got {other:?}"),
            }
        }
        other => panic!("expected Err(StepFailed), got {other:?}"),
    }

    // ── AC2: event ordering (emit-before-IO, D5) ───────────────────────
    // The session log records events in append order. A `signal_received`
    // entry existing is evidence the emit completed; the MockLifecycle
    // Destroy call existing is evidence the IO happened. The engine emits
    // the event via `session.append(...).await` BEFORE it calls
    // `lifecycle.destroy(...).await` inside `finalise_cancel` — this is
    // enforced statically by the code path in `Engine::step`. Observing
    // both side-effects here guarantees neither was skipped.
    let events = engine.session().replay().await.expect("replay");
    let tags: Vec<&str> = events
        .iter()
        .filter_map(|e| e.payload.get("event").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(
        tags,
        vec![
            "workflow_started",
            "step_started",
            "signal_received",
            "step_failed",
        ],
        "expected workflow_started → step_started → signal_received → step_failed, got {tags:?}"
    );

    let received = events
        .iter()
        .find(|e| e.payload.get("event").and_then(|v| v.as_str()) == Some("signal_received"))
        .expect("signal_received present");
    assert_eq!(received.payload["signal"], "sigterm");

    // The trailing step_failed entry is what progress_from_log() treats
    // as terminal on reload — without it a restarted engine could advance
    // past a signalled session (architecture.md §D9 + NFR13).
    let failed = events
        .iter()
        .find(|e| e.payload.get("event").and_then(|v| v.as_str()) == Some("step_failed"))
        .expect("step_failed present");
    assert_eq!(failed.payload["step_name"], "blocked-step");
    assert!(
        failed.payload["error"]
            .as_str()
            .unwrap_or("")
            .contains("Signal"),
        "step_failed.error should mention the signal, got {:?}",
        failed.payload["error"]
    );

    // ── AC3: sandbox teardown via destroy(session_uuid) ────────────────
    let calls = mock.calls().await;
    let destroys: Vec<&MockCall> = calls
        .iter()
        .filter(|c| matches!(c, MockCall::Destroy { .. }))
        .collect();
    assert_eq!(
        destroys.len(),
        1,
        "signal must call destroy exactly once, got {calls:?}"
    );
    let MockCall::Destroy { id } = destroys[0] else {
        unreachable!("filtered for Destroy above")
    };
    assert_eq!(
        *id.as_uuid(),
        session_uuid,
        "destroy must receive the session UUID, not a random one"
    );

    // ── AC4: session status transitioned to cancelled ─────────────────
    let reloaded = Session::load(&pool, session_id)
        .await
        .expect("reload session");
    assert_eq!(
        reloaded.status(),
        SessionStatus::Cancelled,
        "signal path must flip session to cancelled"
    );
}
