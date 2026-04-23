//! Integration test pinning the chat-step runtime ordering contract.
//!
//! PR 5c commit 1 of Task #31 — test-first. The ordering
//!
//! ```text
//! StepStarted → ChatMessageAppended* → StepCompleted
//! ```
//!
//! is the atomic boundary that PR 5b's `compute_progress` pre-staging
//! logic (`engine.rs` pending_chat_turns flush) already relies on:
//! `ChatMessageAppended` is staged under `step_name` on emission and
//! only promoted into `chat_sessions` when the top-level `StepCompleted`
//! with `step_type == "chat"` commits. A runtime that emits turns after
//! the StepCompleted, or that emits a StepCompleted without any turns,
//! silently corrupts replayed chat history on process crash.
//!
//! This test is `#[ignore]`d in commit 1 because `chat_exec.rs` +
//! Engine dispatch land in PR 5c commit 2; running it today would just
//! hit the `step_type \`chat\` not yet supported` fallback at
//! `engine.rs:468-486`. Commit 2 removes the `#[ignore]` tag as its
//! green-bar. The contract this test encodes MUST NOT change even if
//! commit 2 reshapes the dispatch site.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{Engine, HarnessConfig, Step, StepExecutor, StepOutcome, Workflow};
use stepyard_sandbox_orchestrator::{ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle};
use stepyard_session::{migrate, Session, SessionEvent};
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

macro_rules! db_test {
    ($pool:ident, $body:block) => {{
        let Some($pool) = pool().await else {
            eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
            return;
        };
        $body
    }};
}

/// Executor that panics if invoked. Chat steps never call the sandbox
/// step executor — the rig-core runtime speaks HTTP in-process. If this
/// fires, the chat dispatch arm wrongly fell through to the cmd path.
#[derive(Default, Clone)]
struct UnreachableExecutor;

#[async_trait]
impl StepExecutor for UnreachableExecutor {
    async fn execute(&self, session_id: Uuid, step: &Step) -> Result<ExecOutput, SandboxError> {
        self.execute_with_env(session_id, step, &HashMap::new())
            .await
    }

    async fn execute_with_env(
        &self,
        _session_id: Uuid,
        step: &Step,
        _env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        panic!(
            "chat dispatch must never invoke the step executor; got step `{}`",
            step.name
        )
    }
}

fn lifecycle() -> Arc<dyn SandboxLifecycle> {
    Arc::new(MockLifecycle::new())
}

fn unreachable_executor() -> Arc<dyn StepExecutor> {
    Arc::new(UnreachableExecutor)
}

async fn events(engine: &Engine) -> Vec<SessionEvent> {
    engine.session().replay().await.expect("replay")
}

fn event_kind(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("event").and_then(|v| v.as_str())
}

fn event_step_name(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("step_name").and_then(|v| v.as_str())
}

fn event_step_type(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("step_type").and_then(|v| v.as_str())
}

// ---------------------------------------------------------------------------
// Ordering contract: a chat step's lifecycle MUST be StepStarted followed by
// one or more ChatMessageAppended entries, terminated by a single
// StepCompleted — all with step_name="ask". No StepFailed anywhere in the
// sequence. Atomicity here is load-bearing: compute_progress only promotes
// pending chat turns into chat_sessions when the terminal StepCompleted
// with step_type="chat" lands (engine.rs around line 1677), so a runtime
// that re-orders these events corrupts replayed history on crash/retry.
// ---------------------------------------------------------------------------
#[tokio::test]
#[ignore = "PR 5c commit 1: fails until chat_exec.rs + Engine dispatch land in commit 2"]
async fn chat_runtime_emits_step_started_then_appends_then_completed_in_order() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let wf = Workflow::new("chat-order", vec![Step::chat("ask", "Hello")]);

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            unreachable_executor(),
        );
        let outcome = engine.resume().await.expect("resume");
        assert_eq!(
            outcome,
            StepOutcome::WorkflowCompleted,
            "chat workflow must drive to WorkflowCompleted, got {outcome:?}"
        );

        let evs = events(&engine).await;

        let mut started_idx: Option<usize> = None;
        let mut completed_idx: Option<usize> = None;
        let mut chat_indices: Vec<usize> = Vec::new();
        for (idx, ev) in evs.iter().enumerate() {
            if event_step_name(ev) != Some("ask") {
                continue;
            }
            match event_kind(ev) {
                Some("step_started") => {
                    assert!(
                        started_idx.is_none(),
                        "second step_started for `ask` at event {idx}: {:?}",
                        ev.payload
                    );
                    assert_eq!(
                        event_step_type(ev),
                        Some("chat"),
                        "step_started must carry step_type=\"chat\""
                    );
                    started_idx = Some(idx);
                }
                Some("chat_message_appended") => chat_indices.push(idx),
                Some("step_completed") => {
                    assert!(
                        completed_idx.is_none(),
                        "second step_completed for `ask` at event {idx}"
                    );
                    assert_eq!(
                        event_step_type(ev),
                        Some("chat"),
                        "step_completed must carry step_type=\"chat\""
                    );
                    completed_idx = Some(idx);
                }
                Some("step_failed") => {
                    panic!(
                        "chat step must not StepFail on the happy path: {:?}",
                        ev.payload
                    );
                }
                _ => {}
            }
        }

        let started = started_idx.expect("step_started for `ask` missing from replay");
        let completed = completed_idx.expect("step_completed for `ask` missing from replay");
        assert!(
            !chat_indices.is_empty(),
            "chat step must emit at least one ChatMessageAppended between StepStarted and StepCompleted"
        );
        assert!(
            started < completed,
            "StepStarted (idx={started}) must precede StepCompleted (idx={completed})"
        );
        for turn in &chat_indices {
            assert!(
                started < *turn,
                "ChatMessageAppended at idx {turn} emitted before StepStarted at idx {started}"
            );
            assert!(
                *turn < completed,
                "ChatMessageAppended at idx {turn} emitted after StepCompleted at idx {completed}"
            );
        }
    });
}
