//! Story 1.4 — a step configured with `timeout: N` (ms) must be aborted
//! after N ms of wall clock and the engine must:
//!
//! 1. Emit [`Event::StepTimeoutFired`] with `{step_index, configured_ms}`
//!    BEFORE tearing the sandbox down (D5 emit-before-IO ordering).
//! 2. Emit [`Event::StepFailed`] so `progress_from_log()` treats the step
//!    as terminal on replay (architecture.md §D9 + NFR13). Without this a
//!    reloaded engine could advance past a timed-out step.
//! 3. Call `lifecycle.destroy_by_session(session_uuid)` so the backend
//!    removes the real container (DockerLifecycle override).
//! 4. Flip the session status to `failed` (via `finalise_fail`).
//! 5. Return `Err(EngineError::StepFailed { reason: StepTimeout { configured_ms } })`
//!    carrying the D9 termination taxonomy.
//!
//! Uses a custom [`StepExecutor`] that parks forever on `std::future::pending`
//! so the timeout branch is the only way out of the `tokio::select!`. The
//! `MockLifecycle::exec` path would return instantly and short-circuit the
//! timeout — see architecture.md §D5 / Story 1.4 Dev Notes.
//!
//! Runs on real tokio time with a small `configured_ms`. An earlier revision
//! used `start_paused = true` but that fights sqlx: the pool's connect
//! timeout is a tokio timer and never resolves while the clock is paused.
//! The invariant under test (timeout arm wins → emit → destroy → fail) is
//! independent of the specific duration, so a small real value proves it.
//!
//! Skipped gracefully if `STEPYARD_HARNESS_DATABASE_URL` is unset.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    Engine, EngineError, HarnessConfig, Step, StepExecutor, TerminationReason, Workflow,
};
use stepyard_sandbox_orchestrator::{
    ExecOutput, MockCall, MockLifecycle, SandboxError, SandboxLifecycle,
};
use stepyard_session::{migrate, Session, SessionStatus};
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("reach DB");
    migrate(&pool).await.expect("migrations ok");
    Some(pool)
}

/// Executor whose `execute` future never resolves. The step can only leave
/// the select via the cancel or timeout branches.
struct BlockingExecutor;

#[async_trait]
impl StepExecutor for BlockingExecutor {
    async fn execute(&self, _session_id: Uuid, _step: &Step) -> Result<ExecOutput, SandboxError> {
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
async fn step_timeout_emits_event_destroys_sandbox_and_returns_step_failed() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };

    let tenant = format!("step-timeout-{}", Uuid::new_v4());
    // Small real-time timeout — keeps the test fast while still exercising
    // the real timeout arm. < 100ms ensures the cancel_fut's 100ms poll sleep
    // never gets a chance to yield before the timeout arm fires.
    let configured_ms: u64 = 50;
    let workflow = Workflow::new(
        "timeout-wf".to_string(),
        vec![
            Step::cmd("slow-step".to_string(), "sleep forever".to_string())
                .with_timeout(Duration::from_millis(configured_ms)),
        ],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let session_uuid = *session.id().as_uuid();
    let session_id = session.id();

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine = Engine::with_executor(
        HarnessConfig::default(),
        session,
        workflow,
        lifecycle,
        Arc::new(BlockingExecutor),
    );

    // BlockingExecutor parks forever on pending(); cancel is not signalled;
    // so the only arm of the select that can resolve is the timeout sleep.
    let outcome = engine.step().await;

    // ── AC1: typed error with the termination taxonomy ─────────────────
    match outcome {
        Err(EngineError::StepFailed { step_index, reason }) => {
            assert_eq!(step_index, 0);
            match reason {
                TerminationReason::StepTimeout { configured_ms: got } => {
                    assert_eq!(got, configured_ms)
                }
                other => panic!("expected StepTimeout, got {other:?}"),
            }
        }
        other => panic!("expected Err(StepFailed), got {other:?}"),
    }

    // ── AC2: event ordering (emit-before-IO, D5) ───────────────────────
    // The session log records events in append order. Both the structural
    // fact (StepTimeoutFired) and the terminal fact (StepFailed) must be
    // persisted BEFORE the sandbox destroy IO. Observing both events plus
    // the MockLifecycle Destroy call below guarantees the emit-before-IO
    // ordering and that progress_from_log() will see the step as terminal
    // on replay (architecture.md §D9 + NFR13).
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
            "step_timeout_fired",
            "step_failed",
        ],
        "expected workflow_started → step_started → step_timeout_fired → step_failed, got {tags:?}"
    );

    // The StepTimeoutFired payload carries step_index + configured_ms.
    let fired = events
        .iter()
        .find(|e| e.payload.get("event").and_then(|v| v.as_str()) == Some("step_timeout_fired"))
        .expect("step_timeout_fired present");
    assert_eq!(fired.payload["step_index"], 0);
    assert_eq!(fired.payload["configured_ms"], configured_ms);

    // The StepFailed payload carries the step name and a human-readable
    // error derived from the timeout (no PII / command text leak).
    let failed = events
        .iter()
        .find(|e| e.payload.get("event").and_then(|v| v.as_str()) == Some("step_failed"))
        .expect("step_failed present");
    assert_eq!(failed.payload["step_name"], "slow-step");
    assert!(
        failed.payload["error"]
            .as_str()
            .unwrap_or("")
            .contains("timed out"),
        "step_failed.error should mention the timeout, got {:?}",
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
        "timeout must call destroy exactly once, got {calls:?}"
    );
    let MockCall::Destroy { id } = destroys[0] else {
        unreachable!("filtered for Destroy above")
    };
    assert_eq!(
        *id.as_uuid(),
        session_uuid,
        "destroy must receive the session UUID, not a random one"
    );

    // ── AC4: session status transitioned to failed ─────────────────────
    let reloaded = Session::load(&pool, session_id)
        .await
        .expect("reload session");
    assert_eq!(
        reloaded.status(),
        SessionStatus::Failed,
        "timeout path must flip session to failed"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn step_with_no_timeout_is_not_affected_by_timeout_branch() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };

    // No timeout field set. MockLifecycle.exec returns instantly, so the
    // step should complete via the Done arm of the select. This guards
    // against a regression where the `None => pending::<()>()` branch is
    // accidentally replaced with something that resolves.
    let tenant = format!("no-timeout-{}", Uuid::new_v4());
    let workflow = Workflow::new(
        "no-timeout-wf".to_string(),
        vec![Step::cmd("fast".to_string(), "echo hi".to_string())],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine = Engine::new(HarnessConfig::default(), session, workflow, lifecycle);

    let outcome = engine.step().await.expect("step runs");
    assert!(
        matches!(outcome, stepyard_harness::StepOutcome::StepCompleted { .. }),
        "step without timeout should complete normally, got {outcome:?}"
    );
}
