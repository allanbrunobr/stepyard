//! Integration test pinning the chat-step timeout contract.
//!
//! PR 5c commit 4 of Task #31 — test-first. Commit 2 landed the
//! happy-path order (`StepStarted → ChatMessageAppended* →
//! StepCompleted`) and commit 3/3b wired the production seam, but the
//! provider call at `engine.rs` is a bare `.await` with no
//! `tokio::time::timeout` and no `tokio::select!` against the
//! cancel/shutdown plumbing the cmd and agent paths already run
//! (engine.rs:1252-1373 is the model). A slow provider therefore
//! ignores `step.timeout`, never surfaces `StepTimeoutFired`, and —
//! crucially — never returns control to the engine's terminal arms.
//!
//! This test pins the contract commit 4 establishes:
//!
//! ```text
//! StepStarted (chat)
//!   └── ChatMessageAppended (role=user, content=<rendered prompt>)
//!         └── StepTimeoutFired { configured_ms }
//!               └── StepFailed (chat, error contains "timeout"/configured_ms)
//! ```
//!
//! No `StepCompleted` for the chat step, no assistant turn — because
//! `compute_progress` only promotes pending chat turns into
//! `chat_sessions[bucket]` when a terminal `StepCompleted{step_type:
//! "chat"}` lands (engine.rs around line 1677). The absence of
//! `StepCompleted` here IS the atomicity guarantee: the staged user
//! turn never makes it into the replayable session bucket.
//!
//! The test is DB-gated like every other harness integration test —
//! skipped gracefully when `STEPYARD_HARNESS_DATABASE_URL` is unset so
//! `cargo test` on a developer box without Postgres still runs the
//! unit suite.

use std::collections::HashMap;
use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    ChatClient, ChatClientError, ChatCompletionRequest, ChatCompletionResponse, Engine,
    EngineError, HarnessConfig, Step, StepExecutor, Workflow,
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
/// step executor — if this fires, the chat dispatch fell through to
/// the cmd path. Mirrors `chat_replay.rs::UnreachableExecutor`.
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

/// Blocking chat client: enters the provider call and never resolves.
/// The timeout arm of `tokio::select!` must drop this future and
/// surface the configured step timeout.
#[derive(Debug)]
struct BlockingChatClient;

#[async_trait]
impl ChatClient for BlockingChatClient {
    async fn complete(
        &self,
        _req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ChatClientError> {
        pending().await
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

fn event_role(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("role").and_then(|v| v.as_str())
}

fn event_content(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("content").and_then(|v| v.as_str())
}

fn event_configured_ms(ev: &SessionEvent) -> Option<u64> {
    ev.payload.get("configured_ms").and_then(|v| v.as_u64())
}

// ---------------------------------------------------------------------------
// Timeout contract: a slow provider + a short `step.timeout` MUST race
// the same `tokio::select!` cmd and agent run. The timeout arm wins
// before the provider's sleep resolves, the engine emits
// StepTimeoutFired then StepFailed, returns
// `Err(EngineError::StepFailed { TerminationReason::StepTimeout {
// configured_ms } })`, and never emits a StepCompleted or assistant
// turn — keeping `chat_sessions[bucket]` empty by construction (the
// staged user turn lives in `pending_chat_turns` and the promotion
// gate only opens on terminal StepCompleted with step_type=chat).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn chat_step_timeout_emits_started_user_turn_timeout_failed_in_order() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred-timeout".into())
            .await
            .expect("session");

        // 50ms timeout vs a provider future that never resolves: the
        // timeout arm must win and drop the provider call.
        let mut step = Step::chat("ask", "Hello").with_timeout(Duration::from_millis(50));
        step.chat_session = Some("shared".into());
        let wf = Workflow::new("chat-timeout", vec![step]);

        let config = HarnessConfig {
            chat_client: Some(Arc::new(BlockingChatClient)),
            ..Default::default()
        };

        let mut engine =
            Engine::with_executor(config, session, wf, lifecycle(), unreachable_executor());

        // step() returns the raw Result so we can inspect the
        // EngineError variant. resume() wraps multi-step orchestration
        // and would obscure the `TerminationReason::StepTimeout`
        // payload we want to pin.
        let result = engine.step().await;

        match result {
            Err(EngineError::StepFailed {
                reason: stepyard_core::TerminationReason::StepTimeout { configured_ms },
                ..
            }) => {
                assert_eq!(
                    configured_ms, 50,
                    "configured_ms must echo step.timeout (50ms)"
                );
            }
            other => panic!(
                "chat timeout must surface as Err(StepFailed{{StepTimeout{{configured_ms:50}}}}); \
                 got {other:?}"
            ),
        }

        let evs = events(&engine).await;

        let mut started_idx: Option<usize> = None;
        let mut user_turn_idx: Option<usize> = None;
        let mut timeout_idx: Option<usize> = None;
        let mut failed_idx: Option<usize> = None;

        for (idx, ev) in evs.iter().enumerate() {
            match event_kind(ev) {
                Some("step_started") if event_step_name(ev) == Some("ask") => {
                    assert_eq!(
                        event_step_type(ev),
                        Some("chat"),
                        "step_started must carry step_type=\"chat\""
                    );
                    assert!(
                        started_idx.is_none(),
                        "second step_started for `ask` at event {idx}"
                    );
                    started_idx = Some(idx);
                }
                Some("chat_message_appended") => {
                    let role = event_role(ev).expect("chat turn missing `role`");
                    let content = event_content(ev).expect("chat turn missing `content`");
                    assert_ne!(
                        role, "assistant",
                        "no assistant turn must be emitted on the timeout path; \
                         got content={content:?} — atomicity broken"
                    );
                    assert_eq!(
                        role, "user",
                        "only the pre-call user turn may appear in the log on timeout"
                    );
                    assert_eq!(
                        content, "Hello",
                        "user turn must carry the rendered prompt verbatim"
                    );
                    assert!(
                        user_turn_idx.is_none(),
                        "second user turn at event {idx} — engine retried the timed-out call"
                    );
                    user_turn_idx = Some(idx);
                }
                Some("step_timeout_fired") => {
                    assert_eq!(
                        event_configured_ms(ev),
                        Some(50),
                        "step_timeout_fired must echo the configured ms"
                    );
                    assert!(
                        timeout_idx.is_none(),
                        "second step_timeout_fired at event {idx}"
                    );
                    timeout_idx = Some(idx);
                }
                Some("step_failed") if event_step_name(ev) == Some("ask") => {
                    assert_eq!(
                        event_step_type(ev),
                        Some("chat"),
                        "step_failed must carry step_type=\"chat\""
                    );
                    let error = ev
                        .payload
                        .get("error")
                        .and_then(|v| v.as_str())
                        .expect("step_failed missing `error`");
                    assert!(
                        error.contains("timeout") || error.contains("timed out"),
                        "step_failed.error must mention timeout, got {error:?}"
                    );
                    assert!(
                        error.contains("50"),
                        "step_failed.error must mention configured_ms=50, got {error:?}"
                    );
                    assert!(
                        failed_idx.is_none(),
                        "second step_failed for `ask` at event {idx}"
                    );
                    failed_idx = Some(idx);
                }
                Some("step_completed") if event_step_name(ev) == Some("ask") => {
                    panic!(
                        "chat timeout MUST NOT emit step_completed for `ask`; \
                         compute_progress would then promote the staged user turn \
                         and break atomicity. event: {:?}",
                        ev.payload
                    );
                }
                _ => {}
            }
        }

        let started = started_idx.expect("step_started for `ask` missing");
        let user_turn = user_turn_idx.expect("user turn missing");
        let timeout = timeout_idx.expect("step_timeout_fired missing");
        let failed = failed_idx.expect("step_failed for `ask` missing");

        assert!(
            started < user_turn,
            "user turn (idx={user_turn}) must come after step_started (idx={started})"
        );
        assert!(
            user_turn < timeout,
            "step_timeout_fired (idx={timeout}) must come after the user turn (idx={user_turn}); \
             reordering breaks the atomicity contract"
        );
        assert!(
            timeout < failed,
            "step_failed (idx={failed}) must come after step_timeout_fired (idx={timeout})"
        );
    });
}
