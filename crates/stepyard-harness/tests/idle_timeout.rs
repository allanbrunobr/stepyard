//! Story 5.2 — when the sandbox backend reports an output-idle timeout, the
//! engine must persist `IdleTimeoutFired` before sandbox teardown and surface
//! `TerminationReason::IdleTimeout`.
//!
//! Skipped gracefully if `STEPYARD_HARNESS_DATABASE_URL` is unset.

use std::sync::Arc;
use std::time::Duration;

use stepyard_harness::{Engine, EngineError, HarnessConfig, Step, TerminationReason, Workflow};
use stepyard_sandbox_orchestrator::{MockCall, MockLifecycle, SandboxError, SandboxLifecycle};
use stepyard_session::{migrate, Session, SessionStatus};
use sqlx::postgres::PgPoolOptions;
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

#[tokio::test(flavor = "current_thread")]
async fn idle_timeout_emits_event_destroys_sandbox_and_returns_step_failed() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };

    let idle_ms = 30_000;
    let workflow = Workflow::new(
        "idle-timeout-wf".to_string(),
        vec![Step::cmd("quiet-step".to_string(), "sleep forever".to_string())
            .with_idle_timeout(Duration::from_millis(idle_ms))],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    mock.set_exec_with_options_error(SandboxError::IdleTimeout { idle_ms })
        .await;

    let session = Session::new(
        &pool,
        Uuid::new_v4(),
        format!("idle-timeout-{}", Uuid::new_v4()),
    )
    .await
    .expect("new session");
    let session_uuid = *session.id().as_uuid();
    let session_id = session.id();

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine = Engine::new(HarnessConfig::default(), session, workflow, lifecycle);

    match engine.step().await {
        Err(EngineError::StepFailed { step_index, reason }) => {
            assert_eq!(step_index, 0);
            match reason {
                TerminationReason::IdleTimeout { idle_ms: got } => assert_eq!(got, idle_ms),
                other => panic!("expected IdleTimeout, got {other:?}"),
            }
        }
        other => panic!("expected Err(StepFailed), got {other:?}"),
    }

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
            "idle_timeout_fired",
            "step_failed",
        ],
        "expected workflow_started -> step_started -> idle_timeout_fired -> step_failed, got {tags:?}"
    );

    let fired = events
        .iter()
        .find(|e| e.payload.get("event").and_then(|v| v.as_str()) == Some("idle_timeout_fired"))
        .expect("idle_timeout_fired present");
    assert_eq!(fired.payload["step_index"], 0);
    assert_eq!(fired.payload["idle_threshold_ms"], idle_ms);

    let calls = mock.calls().await;
    assert!(
        calls
            .iter()
            .any(|c| matches!(c, MockCall::ExecWithOptions { opts, .. } if opts.idle_timeout == Some(Duration::from_millis(idle_ms)))),
        "exec_with_options should receive idle timeout, got {calls:?}"
    );

    let destroys: Vec<&MockCall> = calls
        .iter()
        .filter(|c| matches!(c, MockCall::Destroy { .. }))
        .collect();
    assert_eq!(destroys.len(), 1, "idle timeout must destroy once");
    let MockCall::Destroy { id } = destroys[0] else {
        unreachable!("filtered for Destroy above")
    };
    assert_eq!(*id.as_uuid(), session_uuid);

    let reloaded = Session::load(&pool, session_id)
        .await
        .expect("reload session");
    assert_eq!(reloaded.status(), SessionStatus::Failed);
}
