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
//! Commit 1 of PR 5c landed this test behind `#[ignore]` as the red
//! bar; commit 2 (the runtime seam in `chat_exec.rs` + Engine
//! dispatch) removes the tag and wires a `MockChatClient` so the
//! contract can be pinned without linking rig-core. The runtime
//! residuals commit 1 held for commit 2 — per-turn `session`,
//! `role`, `content` assertions — are enforced here. The contract
//! this test encodes MUST NOT change even if a later commit
//! reshapes the dispatch site.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    ChatClient, ChatClientError, ChatCompletionRequest, ChatCompletionResponse, Engine,
    HarnessConfig, Step, StepExecutor, StepOutcome, Workflow,
};
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

/// Canned-response chat client. Commit 2's dispatch seam speaks a
/// trait so tests don't depend on rig-core or an HTTP provider. The
/// mock ignores the incoming request fields and replies with the
/// pre-wired `reply`, which is enough to pin the emission order and
/// the `session` / `role` / `content` contract PR 5b's
/// `compute_progress` drain keys on.
#[derive(Debug)]
struct MockChatClient {
    reply: String,
}

impl MockChatClient {
    fn new(reply: impl Into<String>) -> Self {
        Self {
            reply: reply.into(),
        }
    }
}

#[async_trait]
impl ChatClient for MockChatClient {
    async fn complete(
        &self,
        _req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ChatClientError> {
        Ok(ChatCompletionResponse {
            content: self.reply.clone(),
            ..Default::default()
        })
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

fn event_session(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("session").and_then(|v| v.as_str())
}

fn event_role(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("role").and_then(|v| v.as_str())
}

fn event_content(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("content").and_then(|v| v.as_str())
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
async fn chat_runtime_emits_step_started_then_appends_then_completed_in_order() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let mut step = Step::chat("ask", "Hello");
        step.chat_session = Some("shared".into());
        let wf = Workflow::new("chat-order", vec![step]);

        // Inject the mock at the `chat_client` seam added in commit 2.
        // Commit 3 flips the CLI adapter to accept chat steps; in commit
        // 2 we drive the engine directly so the runtime contract can be
        // proven without opening the provider surface.
        let config = HarnessConfig {
            chat_client: Some(Arc::new(MockChatClient::new("Mock reply"))),
            ..Default::default()
        };

        let mut engine =
            Engine::with_executor(config, session, wf, lifecycle(), unreachable_executor());
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
                Some("chat_message_appended") => {
                    // PR 5c commit 2 residual from commit 1: the staging
                    // key (`session`) and the per-turn payload (`role`,
                    // `content`) are load-bearing for
                    // `compute_progress`. If any drift, replayed
                    // history diverges from the live-run view. Pin all
                    // three per-turn.
                    assert_eq!(
                        event_session(ev),
                        Some("shared"),
                        "chat_message_appended must carry the `chat_session` bucket"
                    );
                    let role = event_role(ev).expect("chat turn missing `role`");
                    let content = event_content(ev).expect("chat turn missing `content`");
                    if chat_indices.is_empty() {
                        assert_eq!(role, "user", "first chat turn must be the user prompt");
                        assert_eq!(
                            content, "Hello",
                            "first chat turn must carry the rendered prompt"
                        );
                    } else {
                        assert_eq!(
                            role, "assistant",
                            "second chat turn must be the assistant reply"
                        );
                        assert_eq!(
                            content, "Mock reply",
                            "second chat turn must carry the provider's reply verbatim"
                        );
                    }
                    chat_indices.push(idx);
                }
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
        assert_eq!(
            chat_indices.len(),
            2,
            "chat step must emit exactly user + assistant turns, got {} events",
            chat_indices.len()
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
