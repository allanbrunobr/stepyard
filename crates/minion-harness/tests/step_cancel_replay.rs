//! Regression: after a fast-path cancel (cancel flipped before the step
//! boundary), a reloaded engine must NOT advance the workflow.
//! `progress_from_log()` must see the `step_failed` event the fast-path
//! emits — this closes the "cancelled session advanced after reload" gap,
//! symmetric with `step_timeout_replay.rs` (architecture.md §D9 + NFR13).
//!
//! Skipped gracefully if `MINION_HARNESS_DATABASE_URL` is unset.

use std::sync::Arc;

use minion_harness::{
    Engine, HarnessConfig, Step, StepOutcome, Workflow,
};
use minion_sandbox_orchestrator::{MockLifecycle, SandboxLifecycle};
use minion_session::{migrate, Session};
use sqlx::postgres::PgPoolOptions;
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
async fn reloaded_cancelled_session_refuses_to_advance() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };

    let tenant = format!("reload-cancel-{}", Uuid::new_v4());
    let workflow = Workflow::new(
        "reload-cancel-wf".to_string(),
        vec![Step::cmd("slow".to_string(), "sleep 60".to_string())],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let session_id = session.id();

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine1 = Engine::new(
        HarnessConfig::default(),
        session,
        workflow.clone(),
        lifecycle.clone(),
    );

    // Flip cancel before any step runs — the fast-path cancel branch in
    // Engine::step must emit StepFailed BEFORE finalise_cancel so replay
    // sees the workflow as terminal.
    engine1.cancel_token().cancel();

    let outcome = engine1.step().await.expect("first step returns");
    assert_eq!(outcome, StepOutcome::Cancelled);

    let events_before = engine1
        .session()
        .replay()
        .await
        .expect("replay engine1")
        .len();

    // Fast-path cancel emits exactly: workflow_started → step_failed.
    // `workflow_started` preserves the "every session's log begins with
    // workflow_started" invariant; `step_failed` is what progress_from_log
    // treats as terminal on reload. Any reordering or extra event between
    // them is a regression.
    let tags_before: Vec<String> = engine1
        .session()
        .replay()
        .await
        .expect("replay engine1 tags")
        .iter()
        .filter_map(|e| {
            e.payload
                .get("event")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        tags_before,
        vec!["workflow_started", "step_failed"],
        "fast-path cancel log shape must be workflow_started → step_failed, got {tags_before:?}"
    );

    drop(engine1);

    // Reload the session and build a fresh engine — same workflow, same
    // lifecycle, fresh cancel token. Models a process restart after the
    // cancel commit.
    let reloaded = Session::load(&pool, session_id)
        .await
        .expect("reload session");
    let mut engine2 = Engine::new(HarnessConfig::default(), reloaded, workflow, lifecycle);

    // The reloaded engine's step() must hit the progress.has_failure
    // branch and return StepOutcome::StepFailed without executing anything
    // new. The fresh cancel token is irrelevant — terminality lives in the
    // log, not the token.
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
    // a terminal (cancelled) session.
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
