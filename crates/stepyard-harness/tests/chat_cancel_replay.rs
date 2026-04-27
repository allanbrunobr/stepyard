//! Integration tests pinning the chat-step cancel + shutdown contracts.
//!
//! PR 5c commit 4 of Task #31 — test-first. Companion to
//! `chat_timeout_replay.rs`. The agent and cmd paths already race the
//! exec future against `cancel_token` (a `CancelToken` flip via
//! `engine.cancel()` mid-step) and `shutdown_tx` (the broadcast fired
//! by the SIGINT/SIGTERM handler in `src/signal.rs`); the chat path's
//! bare `.await` at `engine.rs:1631` ignores both. Operators get a
//! workflow that ignores Ctrl-C until the provider responds — even
//! when the response is "never" because the API key was wrong and the
//! provider is wedged on a TLS handshake.
//!
//! These tests pin the two race winners. `cancel_token` flipped
//! mid-step → `Ok(StepOutcome::Cancelled)`, no `StepCompleted`, no
//! assistant turn. `shutdown_tx.send(())` mid-step →
//! `Err(EngineError::StepFailed{TerminationReason::SignalReceived(_)})`,
//! a `signal_received` event, a `step_failed` event, no
//! `StepCompleted`, no assistant turn. Both reuse existing
//! `TerminationReason` variants — commit 4's API decision was to keep
//! the global taxonomy alone and surface the chat-specific message
//! text via new `ChatExecError` variants whose `Display` provides the
//! `step_failed.error` payload.
//!
//! `compute_progress` only promotes pending chat turns into
//! `chat_sessions[bucket]` on a terminal `StepCompleted{step_type:
//! "chat"}`; the absence of that event on either path is the
//! atomicity guarantee.
//!
//! Skipped gracefully when `STEPYARD_HARNESS_DATABASE_URL` is unset.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    ChatClient, ChatClientError, ChatCompletionRequest, ChatCompletionResponse, Engine,
    EngineError, HarnessConfig, Signal, Step, StepExecutor, StepOutcome, Workflow,
};
use stepyard_sandbox_orchestrator::{ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle};
use stepyard_session::{migrate, Session, SessionEvent};
use tokio::sync::broadcast;
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

/// Slow chat client — same shape as `chat_timeout_replay::SlowChatClient`,
/// duplicated rather than shared because the integration test files
/// build as separate test crates and cross-file `use` would require an
/// extra mod-path or a `tests/common/` helper.
#[derive(Debug)]
struct SlowChatClient {
    reply: String,
    delay: Duration,
}

impl SlowChatClient {
    fn new(reply: impl Into<String>, delay: Duration) -> Self {
        Self {
            reply: reply.into(),
            delay,
        }
    }
}

#[async_trait]
impl ChatClient for SlowChatClient {
    async fn complete(
        &self,
        _req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ChatClientError> {
        tokio::time::sleep(self.delay).await;
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

fn event_role(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("role").and_then(|v| v.as_str())
}

fn event_signal(ev: &SessionEvent) -> Option<&str> {
    ev.payload.get("signal").and_then(|v| v.as_str())
}

// ---------------------------------------------------------------------------
// Cancel mid-step: a cancel-token flip during the slow provider call must
// race the exec future and win — `engine.step()` returns
// `Ok(StepOutcome::Cancelled)`, the session log carries `step_started`
// and the user turn but neither `step_completed` nor an assistant
// `chat_message_appended`. Mirrors the agent path at engine.rs:1356-1369.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn chat_step_cancel_token_wins_against_slow_provider() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred-cancel".into())
            .await
            .expect("session");

        let mut step = Step::chat("ask", "Hello");
        step.chat_session = Some("shared".into());
        let wf = Workflow::new("chat-cancel", vec![step]);

        let config = HarnessConfig {
            chat_client: Some(Arc::new(SlowChatClient::new(
                "should never be appended",
                Duration::from_secs(5),
            ))),
            ..Default::default()
        };

        let mut engine =
            Engine::with_executor(config, session, wf, lifecycle(), unreachable_executor());

        // The agent path's cancel future polls `is_cancelled()` every
        // 100ms (engine.rs:1268-1272). 200ms gives one full poll cycle
        // of headroom after the flip — well below the 5s provider sleep.
        let token = engine.cancel_token();
        let trigger = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            token.cancel();
        });

        let outcome = engine.step().await.expect("cancel arm returns Ok");
        trigger.await.expect("trigger task");

        assert_eq!(
            outcome,
            StepOutcome::Cancelled,
            "cancel mid-step must surface as Ok(Cancelled), got {outcome:?}"
        );

        let evs = events(&engine).await;
        assert_event_log_lacks_chat_completion(&evs);
        assert_no_assistant_turn(&evs);
        assert_user_turn_present(&evs);
        assert_step_failed_for_ask(&evs);
    });
}

// ---------------------------------------------------------------------------
// Shutdown broadcast mid-step: SIGINT/SIGTERM fired while the provider is
// in flight must race in via `shutdown_tx`. Engine emits
// `signal_received` then `step_failed`, returns
// `Err(EngineError::StepFailed{TerminationReason::SignalReceived(_)})`,
// and never lets the assistant turn or `step_completed` land. Mirrors
// the agent path at engine.rs:1303-1322.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn chat_step_shutdown_broadcast_wins_against_slow_provider() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred-shutdown".into())
            .await
            .expect("session");

        let mut step = Step::chat("ask", "Hello");
        step.chat_session = Some("shared".into());
        let wf = Workflow::new("chat-shutdown", vec![step]);

        // Hand-built broadcast + signal slot — same shape as
        // `signal_replay.rs`. `default_shutdown_tx()` would give a
        // private channel we can't fire from the test.
        let (tx, _) = broadcast::channel::<()>(16);
        let shutdown_tx = Arc::new(tx);
        let shutdown_signal: Arc<OnceLock<String>> = Arc::new(OnceLock::new());

        let config = HarnessConfig {
            shutdown_tx: shutdown_tx.clone(),
            shutdown_signal: shutdown_signal.clone(),
            chat_client: Some(Arc::new(SlowChatClient::new(
                "should never be appended",
                Duration::from_secs(5),
            ))),
            ..Default::default()
        };

        let mut engine =
            Engine::with_executor(config, session, wf, lifecycle(), unreachable_executor());

        // Mirror `install_handlers`' write-before-send ordering: set
        // the signal name first so the engine sees the populated slot
        // when the broadcast wakes its `recv()`.
        let tx_for_fire = shutdown_tx.clone();
        let slot_for_fire = shutdown_signal.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = slot_for_fire.set("sigterm".into());
            let _ = tx_for_fire.send(());
        });

        let result = engine.step().await;
        match result {
            Err(EngineError::StepFailed {
                reason: stepyard_core::TerminationReason::SignalReceived(signal),
                ..
            }) => {
                assert_eq!(
                    signal, Signal::Sigterm,
                    "TerminationReason::SignalReceived must carry the populated signal slot"
                );
            }
            other => panic!(
                "shutdown broadcast must surface as Err(StepFailed{{SignalReceived(\"sigterm\")}}); \
                 got {other:?}"
            ),
        }

        let evs = events(&engine).await;
        assert_event_log_lacks_chat_completion(&evs);
        assert_no_assistant_turn(&evs);
        assert_user_turn_present(&evs);
        assert_step_failed_for_ask(&evs);

        let signal_event = evs
            .iter()
            .find(|ev| event_kind(ev) == Some("signal_received"))
            .expect("signal_received event missing from replay");
        assert_eq!(
            event_signal(signal_event),
            Some("sigterm"),
            "signal_received must carry signal=\"sigterm\""
        );
    });
}

fn assert_event_log_lacks_chat_completion(evs: &[SessionEvent]) {
    for ev in evs {
        if event_kind(ev) == Some("step_completed") && event_step_name(ev) == Some("ask") {
            panic!(
                "chat step must not emit step_completed on cancel/shutdown — that would let \
                 compute_progress promote the staged user turn into chat_sessions and break \
                 atomicity. event: {:?}",
                ev.payload
            );
        }
    }
}

fn assert_no_assistant_turn(evs: &[SessionEvent]) {
    for ev in evs {
        if event_kind(ev) == Some("chat_message_appended") && event_role(ev) == Some("assistant") {
            panic!(
                "chat step must not emit an assistant chat_message_appended on cancel/shutdown; \
                 the provider call was raced and never returned. event: {:?}",
                ev.payload
            );
        }
    }
}

fn assert_user_turn_present(evs: &[SessionEvent]) {
    let user_turns: Vec<_> = evs
        .iter()
        .filter(|ev| {
            event_kind(ev) == Some("chat_message_appended") && event_role(ev) == Some("user")
        })
        .collect();
    assert_eq!(
        user_turns.len(),
        1,
        "exactly one user turn must appear in the log (pre-call emit at engine.rs:1619); \
         got {}",
        user_turns.len()
    );
}

fn assert_step_failed_for_ask(evs: &[SessionEvent]) {
    let failed = evs
        .iter()
        .find(|ev| event_kind(ev) == Some("step_failed") && event_step_name(ev) == Some("ask"))
        .expect("step_failed for `ask` missing from replay");
    assert_eq!(
        event_step_type(failed),
        Some("chat"),
        "step_failed must carry step_type=\"chat\""
    );
}
