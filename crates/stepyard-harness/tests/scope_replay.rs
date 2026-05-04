//! Integration tests for the scope runner: `call` / `repeat` / `map` and the
//! replay model that lets a container step survive a process crash. PR 3 of
//! Task #31.
//!
//! Requires a PostgreSQL reachable via `STEPYARD_HARNESS_DATABASE_URL`. Tests
//! skip (without failing) when the env var is not set — mirrors
//! `tests/gate_replay.rs` so CI without a database sidecar stays green.

use std::collections::HashMap;
use std::future::pending;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    ChatClient, ChatClientError, ChatCompletionRequest, ChatCompletionResponse, Engine,
    EngineError, HarnessConfig, Scope, Step, StepExecutor, StepKind, StepOutcome,
    TerminationReason, Workflow,
};
use stepyard_sandbox_orchestrator::{ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle};
use stepyard_session::{migrate, Session, SessionEvent, SessionStatus};
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
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

/// Echo executor: interprets `echo <payload>` commands as `stdout=payload\n`,
/// `exit 0`; bare `false` → exit 1; anything else → empty stdout, exit 0.
///
/// Drives iteration-sensitive output via rendered templates
/// (`echo {{ scope.value }}`, `echo item_{{ scope.index }}`) without
/// per-step preset bookkeeping — a scripted-per-name executor would need
/// to distinguish same-named body steps across iterations, which the
/// engine intentionally does not.
#[derive(Default, Clone)]
struct EchoExecutor;

#[async_trait]
impl StepExecutor for EchoExecutor {
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
        let cmd = step.command.trim();
        if cmd == "false" {
            return Ok(ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 1,
            });
        }
        let stdout = cmd
            .strip_prefix("echo ")
            .map(|rest| format!("{rest}\n"))
            .unwrap_or_default();
        Ok(ExecOutput {
            stdout,
            stderr: String::new(),
            exit_code: 0,
        })
    }
}

fn lifecycle() -> Arc<dyn SandboxLifecycle> {
    Arc::new(MockLifecycle::new())
}

fn echo_executor() -> Arc<dyn StepExecutor> {
    Arc::new(EchoExecutor)
}

fn fixture_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_claude.sh")
}

fn agent_step(name: &str, prompt: &str) -> Step {
    let mut step = Step::agent(name, prompt);
    step.agent_command = Some(fixture_path().to_string_lossy().into_owned());
    step
}

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

/// Blocking executor used by the scoped-cmd timeout attribution test.
/// The body cmd's `timeout:` fires while this future stays pending, so
/// the `TimedOut` branch of `execute_cmd_with_select` is exercised
/// deterministically.
#[derive(Default, Clone)]
struct SleepExecutor;

#[async_trait]
impl StepExecutor for SleepExecutor {
    async fn execute(&self, session_id: Uuid, step: &Step) -> Result<ExecOutput, SandboxError> {
        self.execute_with_env(session_id, step, &HashMap::new())
            .await
    }

    async fn execute_with_env(
        &self,
        _session_id: Uuid,
        _step: &Step,
        _env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        pending().await
    }
}

fn sleep_executor() -> Arc<dyn StepExecutor> {
    Arc::new(SleepExecutor)
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

fn scope_context_of(ev: &SessionEvent) -> Option<(String, u64, u64)> {
    let sc = ev.payload.get("scope_context")?;
    let container = sc.get("container")?.as_str()?.to_string();
    let iteration = sc.get("iteration")?.as_u64()?;
    let position = sc.get("position")?.as_u64()?;
    Some((container, iteration, position))
}

fn workflow_with_scope(
    workflow_name: &str,
    top: Vec<Step>,
    scope_name: &str,
    body: Vec<Step>,
    outputs: Option<&str>,
) -> Workflow {
    let mut wf = Workflow::new(workflow_name, top);
    wf.scopes.insert(
        scope_name.into(),
        Scope {
            steps: body,
            outputs: outputs.map(str::to_string),
        },
    );
    wf
}

fn gate(name: &str, condition: &str, on_pass: &str, on_fail: &str) -> Step {
    let mut g = Step::gate(name, condition);
    g.on_pass = Some(on_pass.into());
    g.on_fail = Some(on_fail.into());
    g
}

#[tokio::test]
async fn call_runs_scope_once_and_output_is_referenceable_afterwards() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // `call` body produces "hi\n". The container's top-level
        // StepCompleted carries that synthetic stdout, and a subsequent
        // gate can reference `steps.greet.stdout` (gates are the only
        // site PR 3 renders templates at the top level — top-level cmd
        // command rendering lands in a later PR of #31).
        let wf = workflow_with_scope(
            "call-basic",
            vec![
                Step::call("greet", "greeter"),
                gate("after", "{{ steps.greet.stdout }}", "continue", "fail"),
            ],
            "greeter",
            vec![Step::cmd("body", "echo ok")],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        let outcome = engine.resume().await.expect("resume");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let evs = events(&engine).await;
        // Container completion must exist at top level (no scope_context)
        // and carry the synthetic output. The gate rendered its condition
        // from that output and resolved truthy (`ok` → continue).
        let container_done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("greet")
                    && scope_context_of(e).is_none()
            })
            .expect("container top-level completion");
        let output = container_done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("synthetic stdout");
        assert!(output.contains("ok"), "got {output}");

        // Gate completion also landed — referenceable output proved it.
        let after_done = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed") && event_step_name(e) == Some("after")
        });
        assert!(after_done, "gate after the container must have completed");
    });
}

#[tokio::test]
async fn call_scope_body_runs_agent_and_chat_steps() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let mut chat = Step::chat("ask", "Hello {{ steps.seed.stdout }}");
        chat.chat_session = Some("scoped-chat".into());
        let wf = workflow_with_scope(
            "call-agent-chat",
            vec![Step::call("run", "body")],
            "body",
            vec![
                Step::cmd("seed", "echo scoped"),
                agent_step("plan", "Plan with {{ steps.seed.stdout }}"),
                chat,
            ],
            None,
        );
        let config = HarnessConfig {
            chat_client: Some(Arc::new(MockChatClient::new("Scoped chat reply"))),
            ..Default::default()
        };

        let mut engine = Engine::with_executor(config, session, wf, lifecycle(), echo_executor());
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        let mut scoped_done: Vec<(u64, String, String)> = evs
            .iter()
            .filter_map(|e| {
                if event_kind(e)? != "step_completed" {
                    return None;
                }
                let (container, iter, pos) = scope_context_of(e)?;
                if container != "run" || iter != 0 {
                    return None;
                }
                Some((
                    pos,
                    event_step_name(e)?.to_string(),
                    e.payload.get("step_type")?.as_str()?.to_string(),
                ))
            })
            .collect();
        scoped_done.sort_by_key(|(pos, _, _)| *pos);
        assert_eq!(
            scoped_done,
            vec![
                (0, "seed".to_string(), "cmd".to_string()),
                (1, "plan".to_string(), "agent".to_string()),
                (2, "ask".to_string(), "chat".to_string())
            ]
        );

        let chat_turns: Vec<&SessionEvent> = evs
            .iter()
            .filter(|e| event_kind(e) == Some("chat_message_appended"))
            .collect();
        assert_eq!(
            chat_turns.len(),
            2,
            "scoped chat emits user + assistant turns"
        );
        assert_eq!(
            chat_turns[0]
                .payload
                .get("content")
                .and_then(|v| v.as_str()),
            Some("Hello scoped\n"),
            "scoped chat prompt must render against earlier scoped cmd output"
        );

        let container_done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("run")
                    && scope_context_of(e).is_none()
            })
            .expect("container top-level completion");
        let output = container_done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("synthetic stdout");
        assert_eq!(output, "Scoped chat reply");
    });
}

#[tokio::test]
async fn repeat_breaks_on_scoped_gate_and_top_level_counter_advances_once() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Scope body: cmd then gate. The gate fires `break` on iteration 1.
        // Top-level has only the `repeat` + a trailing cmd to prove the
        // counter advanced exactly once past the container.
        let wf = workflow_with_scope(
            "repeat-break",
            vec![
                Step::repeat("loop", "body"),
                Step::cmd("after", "echo done"),
            ],
            "body",
            vec![
                Step::cmd("work", "echo iter_{{ scope.index }}"),
                // `{{ scope.index }} == 1` only renders to a bool when the
                // template resolves via Tera's truthy vocabulary. Easier:
                // branch on `{{ scope.index >= 1 }}` which Tera renders
                // as `true`/`false`.
                gate("check", "{{ scope.index >= 1 }}", "break", "continue"),
            ],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        let outcome = engine.resume().await.expect("resume");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let evs = events(&engine).await;
        // Two iterations executed (index 0 → continue, index 1 → break).
        let scoped_completions: Vec<_> = evs
            .iter()
            .filter(|e| event_kind(e) == Some("step_completed") && scope_context_of(e).is_some())
            .collect();
        // Iter 0: work + check ; Iter 1: work + check → 4 scoped completions.
        assert_eq!(
            scoped_completions.len(),
            4,
            "expected 4 scoped completions, got {}",
            scoped_completions.len()
        );

        // Top-level container + trailing `after` both completed.
        let top_level_done_names: Vec<_> = evs
            .iter()
            .filter(|e| event_kind(e) == Some("step_completed") && scope_context_of(e).is_none())
            .filter_map(event_step_name)
            .collect();
        assert_eq!(top_level_done_names, vec!["loop", "after"]);
    });
}

#[tokio::test]
async fn repeat_skip_routes_to_next_iteration_without_failing() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Skip on iter 0, break on iter 1.
        let wf = workflow_with_scope(
            "repeat-skip",
            vec![Step::repeat("loop", "body")],
            "body",
            vec![
                gate("pre", "{{ scope.index >= 1 }}", "continue", "skip"),
                Step::cmd("work", "echo ran_{{ scope.index }}"),
                gate("post", "{{ scope.index >= 1 }}", "break", "continue"),
            ],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        // Iter 0: pre (skip) — no work, no post.
        // Iter 1: pre, work, post (break).
        let scoped_completions: Vec<((String, u64, u64), String)> = evs
            .iter()
            .filter_map(|e| {
                if event_kind(e)? != "step_completed" {
                    return None;
                }
                let ctx = scope_context_of(e)?;
                let name = event_step_name(e)?.to_string();
                Some((ctx, name))
            })
            .collect();

        let iter0_completions: Vec<&str> = scoped_completions
            .iter()
            .filter(|((_, it, _), _)| *it == 0)
            .map(|(_, name)| name.as_str())
            .collect();
        assert_eq!(iter0_completions, vec!["pre"]);

        let iter1_completions: Vec<&str> = scoped_completions
            .iter()
            .filter(|((_, it, _), _)| *it == 1)
            .map(|(_, name)| name.as_str())
            .collect();
        assert_eq!(iter1_completions, vec!["pre", "work", "post"]);
    });
}

#[tokio::test]
async fn repeat_max_iterations_without_break_completes_ok() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let mut loop_step = Step::repeat("loop", "body");
        loop_step.max_iterations = Some(3);

        // Body always continues — the cap ends the loop.
        let wf = workflow_with_scope(
            "repeat-max",
            vec![loop_step],
            "body",
            vec![Step::cmd("work", "echo iter_{{ scope.index }}")],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        // Three iterations ran; the container's top-level completion
        // landed successfully (not a failure).
        let scoped_work_completions = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("work")
                    && scope_context_of(e).is_some()
            })
            .count();
        assert_eq!(scoped_work_completions, 3);

        let container_ok = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("loop")
                && scope_context_of(e).is_none()
        });
        assert!(container_ok, "container must complete OK after max");

        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

/// A `repeat` whose body never breaks and whose YAML omits
/// `max_iterations` must fall back to the v1 default cap of 3.
/// Absent this default, the runner would spin indefinitely — see
/// `src/steps/repeat.rs:58` in the legacy path (`unwrap_or(3)`).
#[tokio::test]
async fn repeat_without_max_iterations_uses_default_cap() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // No `max_iterations` set → must cap at 3 by default.
        let loop_step = Step::repeat("loop", "body");
        assert!(
            loop_step.max_iterations.is_none(),
            "test precondition: Step::repeat must leave max_iterations unset"
        );

        let wf = workflow_with_scope(
            "repeat-default-cap",
            vec![loop_step],
            "body",
            vec![Step::cmd("work", "echo iter_{{ scope.index }}")],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        let scoped_work_completions = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("work")
                    && scope_context_of(e).is_some()
            })
            .count();
        assert_eq!(
            scoped_work_completions, 3,
            "default cap must stop the loop at 3 iterations"
        );

        let container_ok = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("loop")
                && scope_context_of(e).is_none()
        });
        assert!(container_ok, "container must complete OK after default cap");

        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn map_preserves_iteration_order() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let wf = workflow_with_scope(
            "map-order",
            vec![Step::map("fan", "body", r#"["a","b","c"]"#)],
            "body",
            vec![Step::cmd("emit", "echo {{ scope.value }}")],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        let ordered_iterations: Vec<u64> = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("emit")
                    && scope_context_of(e).is_some()
            })
            .filter_map(|e| scope_context_of(e).map(|(_, it, _)| it))
            .collect();
        assert_eq!(ordered_iterations, vec![0, 1, 2]);
    });
}

#[tokio::test]
async fn map_skip_advances_to_next_item_without_break() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Skip the first item via a gate; run work for the rest.
        let wf = workflow_with_scope(
            "map-skip",
            vec![Step::map("fan", "body", r#"["x","y","z"]"#)],
            "body",
            vec![
                gate("guard", "{{ scope.index >= 1 }}", "continue", "skip"),
                Step::cmd("emit", "echo {{ scope.value }}"),
            ],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        // Iteration 0 only runs `guard`; iterations 1 and 2 run both.
        let emit_iterations: Vec<u64> = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("emit")
                    && scope_context_of(e).is_some()
            })
            .filter_map(|e| scope_context_of(e).map(|(_, it, _)| it))
            .collect();
        assert_eq!(emit_iterations, vec![1, 2]);
    });
}

#[tokio::test]
async fn map_break_ends_loop_early() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Break at the end of the first iteration. Second item never runs.
        let wf = workflow_with_scope(
            "map-break",
            vec![Step::map("fan", "body", r#"["p","q","r"]"#)],
            "body",
            vec![
                Step::cmd("emit", "echo {{ scope.value }}"),
                gate("stop", "{{ scope.index >= 0 }}", "break", "continue"),
            ],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        let emit_iterations: Vec<u64> = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("emit")
                    && scope_context_of(e).is_some()
            })
            .filter_map(|e| scope_context_of(e).map(|(_, it, _)| it))
            .collect();
        assert_eq!(emit_iterations, vec![0]);
    });
}

/// Run `wf` on a fresh session and collect its full event log. Used as
/// the "pre-crash" side of a replay test — subsequent phases inject a
/// prefix of these events into a new session and verify the scope
/// runner can pick up where the log leaves off.
async fn capture_full_log(pool: &sqlx::PgPool, wf: Workflow) -> Vec<SessionEvent> {
    let session = Session::new(pool, Uuid::new_v4(), "edenred".into())
        .await
        .expect("session");
    let mut engine = Engine::with_executor(
        HarnessConfig::default(),
        session,
        wf,
        lifecycle(),
        echo_executor(),
    );
    engine.resume().await.expect("full run");
    engine.session().replay().await.expect("replay")
}

/// Index *after* the last event whose payload satisfies `pred`. Panics if
/// no match is found, so the caller's assumption about log shape stays
/// load-bearing.
fn cut_after(events: &[SessionEvent], pred: impl Fn(&SessionEvent) -> bool) -> usize {
    events
        .iter()
        .rposition(pred)
        .expect("boundary event not found")
        + 1
}

async fn seed_session_with_prefix(pool: &sqlx::PgPool, prefix: &[SessionEvent]) -> Session {
    let session = Session::new(pool, Uuid::new_v4(), "edenred".into())
        .await
        .expect("session");
    for ev in prefix {
        session
            .append(ev.payload.clone())
            .await
            .expect("seed event");
    }
    session
}

#[tokio::test]
async fn call_replay_picks_up_mid_scope_body() {
    db_test!(pool, {
        // Two body steps so we can cut between them. Resume must execute
        // `b` only — not re-run `a`.
        let wf = workflow_with_scope(
            "call-replay",
            vec![Step::call("greet", "body")],
            "body",
            vec![Step::cmd("a", "echo first"), Step::cmd("b", "echo second")],
            None,
        );

        let full = capture_full_log(&pool, wf.clone()).await;
        // Cut just after the scoped step_completed for position 0.
        let boundary = cut_after(&full, |e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("a")
                && scope_context_of(e).is_some_and(|(_, it, pos)| it == 0 && pos == 0)
        });

        let seeded = seed_session_with_prefix(&pool, &full[..boundary]).await;
        let session_id = seeded.id();
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            seeded,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        // Replay must run `b` exactly once and emit the container's
        // top-level completion. `a` must not reappear.
        let a_completions = evs
            .iter()
            .filter(|e| event_kind(e) == Some("step_completed") && event_step_name(e) == Some("a"))
            .count();
        let b_completions = evs
            .iter()
            .filter(|e| event_kind(e) == Some("step_completed") && event_step_name(e) == Some("b"))
            .count();
        assert_eq!(a_completions, 1, "a should not re-run");
        assert_eq!(b_completions, 1, "b must run post-replay");

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn repeat_replay_picks_up_after_completed_iteration() {
    db_test!(pool, {
        let mut loop_step = Step::repeat("loop", "body");
        loop_step.max_iterations = Some(3);
        let wf = workflow_with_scope(
            "repeat-replay",
            vec![loop_step],
            "body",
            vec![Step::cmd("work", "echo iter_{{ scope.index }}")],
            None,
        );

        let full = capture_full_log(&pool, wf.clone()).await;
        // Cut after iteration 0 fully completes — iteration 1 and 2 must
        // run fresh on resume.
        let boundary = cut_after(&full, |e| {
            event_kind(e) == Some("step_completed")
                && scope_context_of(e).is_some_and(|(_, it, _)| it == 0)
        });

        let seeded = seed_session_with_prefix(&pool, &full[..boundary]).await;
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            seeded,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        let work_iters: Vec<u64> = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("work")
                    && scope_context_of(e).is_some()
            })
            .filter_map(|e| scope_context_of(e).map(|(_, it, _)| it))
            .collect();
        // Iteration 0 was seeded; iterations 1 and 2 run post-replay.
        // No duplicate iter-0 completion.
        assert_eq!(work_iters, vec![0, 1, 2]);
    });
}

#[tokio::test]
async fn map_replay_picks_up_after_completed_iteration() {
    db_test!(pool, {
        let wf = workflow_with_scope(
            "map-replay",
            vec![Step::map("fan", "body", r#"["one","two","three"]"#)],
            "body",
            vec![Step::cmd("emit", "echo {{ scope.value }}")],
            None,
        );

        let full = capture_full_log(&pool, wf.clone()).await;
        let boundary = cut_after(&full, |e| {
            event_kind(e) == Some("step_completed")
                && scope_context_of(e).is_some_and(|(_, it, _)| it == 0)
        });

        let seeded = seed_session_with_prefix(&pool, &full[..boundary]).await;
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            seeded,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        let emit_iters: Vec<u64> = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("emit")
                    && scope_context_of(e).is_some()
            })
            .filter_map(|e| scope_context_of(e).map(|(_, it, _)| it))
            .collect();
        // All three iterations ultimately logged: iter 0 seeded, 1+2
        // from the replay path. In declaration order — map preserves it.
        assert_eq!(emit_iters, vec![0, 1, 2]);
    });
}

#[tokio::test]
async fn scoped_completions_do_not_advance_top_level_step_counter() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // call contains two body steps — four scoped events (2 started + 2
        // completed). The top-level counter must only advance by 1 (the
        // container itself), proven by the `after` cmd running afterwards
        // without the engine short-circuiting past it.
        let wf = workflow_with_scope(
            "counter-isolation",
            vec![Step::call("group", "body"), Step::cmd("after", "echo post")],
            "body",
            vec![Step::cmd("first", "echo 1"), Step::cmd("second", "echo 2")],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        let top_level_done_names: Vec<_> = evs
            .iter()
            .filter(|e| event_kind(e) == Some("step_completed") && scope_context_of(e).is_none())
            .filter_map(event_step_name)
            .collect();
        // Only `group` (container) and `after` count at top level, in
        // this order. No scoped body step advances the counter.
        assert_eq!(top_level_done_names, vec!["group", "after"]);

        // Sanity: scoped completions exist for the two body steps.
        let scoped_names: Vec<_> = evs
            .iter()
            .filter(|e| event_kind(e) == Some("step_completed") && scope_context_of(e).is_some())
            .filter_map(event_step_name)
            .collect();
        assert_eq!(scoped_names, vec!["first", "second"]);
    });
}

#[tokio::test]
async fn nested_container_inside_scope_fails_on_outer_container() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Outer `call` body contains an inner `call`. The adapter rejects
        // nested containers at YAML parse time — this test bypasses the
        // adapter by constructing Steps directly to exercise the engine's
        // defensive guard. The outer container must emit StepFailed with
        // no scoped event for the inner container.
        let inner = Step {
            name: "inner".into(),
            kind: StepKind::Call,
            command: String::new(),
            timeout: None,
            idle_timeout: None,
            env: HashMap::new(),
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: Some("other".into()),
            max_iterations: None,
            initial_value: None,
            items: None,
            parallel: None,
            outputs: None,
            prompt: None,
            model: None,
            system_prompt_append: None,
            permissions: None,
            resume: None,
            fork_session: None,
            agent_session: None,
            agent_command: None,
            chat_provider: None,
            max_tokens: None,
            temperature: None,
            api_key_env: None,
            base_url: None,
            chat_session: None,
            truncation: None,
        };
        let wf = workflow_with_scope(
            "nested-guard",
            vec![Step::call("outer", "body")],
            "body",
            vec![inner],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        let outcome = engine.resume().await.expect("resume");
        match outcome {
            StepOutcome::StepFailed { step_name, error } => {
                assert_eq!(step_name, "outer");
                assert!(
                    error.contains("nested container"),
                    "error should mention nested container, got: {error}"
                );
            }
            other => panic!("expected StepFailed on outer, got {other:?}"),
        }

        // No scoped event should be emitted for `inner`.
        let evs = events(&engine).await;
        let inner_events = evs
            .iter()
            .filter(|e| event_step_name(e) == Some("inner"))
            .count();
        assert_eq!(
            inner_events, 0,
            "no scoped events should be emitted for inner container"
        );

        // The outer container's failure is the session failure.
        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Failed);
    });
}

// ---------------------------------------------------------------------------
// Replay tests covering the per-iteration skip / break terminality fix in
// `ContainerReplayState`. Before the fix, a crash immediately after a scoped
// gate's `skip` or `break` event could resume inside the same iteration and
// run the body step that skip/break was supposed to prevent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn call_replay_after_gate_skip_does_not_run_later_scope_steps() {
    db_test!(pool, {
        // Scope body: gate(skip) then a cmd that must NOT run. The non-
        // replay path returns IterationOutcome::Completed on skip and
        // finalises the call container. If replay sees only the skip
        // event and no container top-level completion, it must treat
        // the iteration as terminal and proceed to synthesize the
        // container completion — not resume at the cmd.
        let wf = workflow_with_scope(
            "call-skip-replay",
            vec![Step::call("greet", "body")],
            "body",
            vec![
                gate("pre", "false", "continue", "skip"),
                Step::cmd("should_not_run", "echo boom"),
            ],
            None,
        );

        let full = capture_full_log(&pool, wf.clone()).await;
        // Cut just after the scoped gate skip event, before the container's
        // top-level StepCompleted would have been appended.
        let boundary = cut_after(&full, |e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("pre")
                && scope_context_of(e).is_some_and(|(_, it, pos)| it == 0 && pos == 0)
        });

        let seeded = seed_session_with_prefix(&pool, &full[..boundary]).await;
        let session_id = seeded.id();
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            seeded,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        // The cmd after the gate must NEVER appear — neither as
        // StepStarted nor StepCompleted.
        let touched = evs
            .iter()
            .any(|e| event_step_name(e) == Some("should_not_run"));
        assert!(
            !touched,
            "scope-body cmd after gate-skip must not run on replay"
        );

        // The container's top-level StepCompleted must have landed,
        // proving replay finalised the container rather than parking.
        let container_done = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("greet")
                && scope_context_of(e).is_none()
        });
        assert!(
            container_done,
            "container top-level StepCompleted must exist after replay"
        );

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn repeat_replay_after_gate_skip_advances_iteration() {
    db_test!(pool, {
        // Iter 0: gate skips (no work). Iter 1: gate continues, work runs,
        // post gate breaks. Cut just after iter 0's skip event — replay
        // must not re-enter iter 0's scope body at the cmd; it must
        // advance to iter 1 and let post break the loop.
        let wf = workflow_with_scope(
            "repeat-skip-replay",
            vec![Step::repeat("loop", "body")],
            "body",
            vec![
                gate("pre", "{{ scope.index >= 1 }}", "continue", "skip"),
                Step::cmd("work", "echo ran_{{ scope.index }}"),
                gate("post", "{{ scope.index >= 1 }}", "break", "continue"),
            ],
            None,
        );

        let full = capture_full_log(&pool, wf.clone()).await;
        // Boundary: immediately after the iter-0 pre (skip) event.
        let boundary = cut_after(&full, |e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("pre")
                && scope_context_of(e).is_some_and(|(_, it, pos)| it == 0 && pos == 0)
        });

        let seeded = seed_session_with_prefix(&pool, &full[..boundary]).await;
        let session_id = seeded.id();
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            seeded,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        // No `work` completion should exist for iteration 0 — skip on
        // pre must have ended iteration 0 both pre- and post-replay.
        let iter0_work = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("work")
                && scope_context_of(e).is_some_and(|(_, it, _)| it == 0)
        });
        assert!(
            !iter0_work,
            "iter 0 work must never run — gate pre skipped it"
        );

        // Iter 1's full body (pre+work+post) must be logged exactly once.
        let iter1_names: Vec<&str> = evs
            .iter()
            .filter_map(|e| {
                if event_kind(e)? != "step_completed" {
                    return None;
                }
                let (_, it, _) = scope_context_of(e)?;
                (it == 1).then_some(event_step_name(e)?)
            })
            .collect();
        assert_eq!(iter1_names, vec!["pre", "work", "post"]);

        // Container top-level completion must exist.
        let container_done = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("loop")
                && scope_context_of(e).is_none()
        });
        assert!(
            container_done,
            "repeat container must finalise after replay"
        );

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn map_replay_after_gate_skip_advances_item() {
    db_test!(pool, {
        // Two items. Gate skips iter 0; iter 1 runs emit. Cut just after
        // the iter-0 skip event so replay must advance to iter 1 without
        // re-entering iter 0's body.
        let wf = workflow_with_scope(
            "map-skip-replay",
            vec![Step::map("fan", "body", r#"["x","y"]"#)],
            "body",
            vec![
                gate("guard", "{{ scope.index >= 1 }}", "continue", "skip"),
                Step::cmd("emit", "echo {{ scope.value }}"),
            ],
            None,
        );

        let full = capture_full_log(&pool, wf.clone()).await;
        let boundary = cut_after(&full, |e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("guard")
                && scope_context_of(e).is_some_and(|(_, it, pos)| it == 0 && pos == 0)
        });

        let seeded = seed_session_with_prefix(&pool, &full[..boundary]).await;
        let session_id = seeded.id();
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            seeded,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        // Item 0's emit must never run; item 1's emit must run exactly once.
        let emit_iters: Vec<u64> = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("emit")
                    && scope_context_of(e).is_some()
            })
            .filter_map(|e| scope_context_of(e).map(|(_, it, _)| it))
            .collect();
        assert_eq!(emit_iters, vec![1]);

        // Container top-level completion must exist AND its synthetic
        // stdout must be a 2-element JSON array aligned with `items`:
        // iter 0 was skipped (empty slot `""`), iter 1 ran emit and
        // produced `"y\n"`. Before the collection-loop fix the first
        // missing output aborted the loop and the array collapsed to
        // `[]`, losing the real output at index 1.
        let container_done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("fan")
                    && scope_context_of(e).is_none()
            })
            .expect("map container must finalise after replay");
        let container_stdout = container_done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("container output.stdout");
        let parsed: Vec<String> =
            serde_json::from_str(container_stdout).expect("container stdout is a JSON array");
        assert_eq!(parsed, vec!["".to_string(), "y\n".to_string()]);

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn map_replay_after_gate_break_does_not_process_remaining_items() {
    db_test!(pool, {
        // Three items. Iter 0 runs emit then breaks. Iters 1,2 must never
        // run — not pre-crash, not on replay. Cut just after the iter-0
        // break event, before the container's top-level completion.
        let wf = workflow_with_scope(
            "map-break-replay",
            vec![Step::map("fan", "body", r#"["a","b","c"]"#)],
            "body",
            vec![
                Step::cmd("emit", "echo {{ scope.value }}"),
                gate("stop", "{{ scope.index >= 0 }}", "break", "continue"),
            ],
            None,
        );

        let full = capture_full_log(&pool, wf.clone()).await;
        let boundary = cut_after(&full, |e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("stop")
                && scope_context_of(e).is_some_and(|(_, it, pos)| it == 0 && pos == 1)
        });

        let seeded = seed_session_with_prefix(&pool, &full[..boundary]).await;
        let session_id = seeded.id();
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            seeded,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        // Iter 0's emit is the only scoped cmd completion; iters 1,2
        // never run — not even a StepStarted.
        let emit_iters: Vec<u64> = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("emit")
                    && scope_context_of(e).is_some()
            })
            .filter_map(|e| scope_context_of(e).map(|(_, it, _)| it))
            .collect();
        assert_eq!(emit_iters, vec![0]);

        // Defence-in-depth: no scoped event of any kind lands at iter>=1.
        let later_scoped = evs
            .iter()
            .any(|e| scope_context_of(e).is_some_and(|(_, it, _)| it >= 1));
        assert!(
            !later_scoped,
            "no scoped event must land after break on later iterations"
        );

        // Container top-level completion must exist.
        let container_done = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("fan")
                && scope_context_of(e).is_none()
        });
        assert!(
            container_done,
            "map container must finalise after break replay"
        );

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

// ---------------------------------------------------------------------------
// Issue #48 — repeat replay after a gate `break` whose iteration is the
// LAST one to run must surface the break iter's final cmd snap as the
// container's synthetic top-level stdout, not the prior iter's snap.
//
// Pre-fix, `run_repeat` seeds `last_output_final` from
// `last_output_per_iteration[next_iteration - 1]`. After rebuild on this
// log, `next_iteration == break_iteration`, so the seed reaches one
// iteration BEHIND the break iter. `synthesise_container_output` (no
// outputs template) then surfaces the prior iter's stdout — wrong.
//
// The discriminating assertion is on the container's top-level
// `step_completed.output.stdout`. A single value distinguishes the two
// paths; the post-replay execution otherwise produces the same scoped
// events as non-replay (which is itself pinned by
// `repeat_breaks_on_scoped_gate_and_top_level_counter_advances_once`).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn repeat_replay_after_gate_break_at_last_position_surfaces_break_iter_output() {
    db_test!(pool, {
        // Iter 0: work runs (stdout "iter_0\n"), stop continues. Iter 1:
        // work runs (stdout "iter_1\n"), stop fires `break` at the LAST
        // body position. Cut just after the iter-1 stop break event —
        // before the container's top-level step_completed lands. Replay
        // resumes, hits broke=true on rebuild, and synthesises the
        // container's output. That output's stdout must be "iter_1\n".
        let wf = workflow_with_scope(
            "repeat-break-replay",
            vec![Step::repeat("loop", "body")],
            "body",
            vec![
                Step::cmd("work", "echo iter_{{ scope.index }}"),
                gate("stop", "{{ scope.index >= 1 }}", "break", "continue"),
            ],
            None,
        );

        let full = capture_full_log(&pool, wf.clone()).await;
        let boundary = cut_after(&full, |e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("stop")
                && scope_context_of(e).is_some_and(|(_, it, pos)| it == 1 && pos == 1)
        });

        let seeded = seed_session_with_prefix(&pool, &full[..boundary]).await;
        let session_id = seeded.id();
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            seeded,
            wf,
            lifecycle(),
            echo_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;

        // DISCRIMINATOR: container's synthetic stdout must equal the break
        // iter's last cmd snap. Pre-fix this lands as "iter_0\n".
        let container_done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("loop")
                    && scope_context_of(e).is_none()
            })
            .expect("repeat container must finalise after break replay");
        let container_stdout = container_done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("synthetic stdout on container step_completed");
        assert_eq!(
            container_stdout, "iter_1\n",
            "replay's synthetic container stdout must equal the break iter's last cmd snap"
        );

        // Defence-in-depth: replay must not re-execute iter-1 body steps.
        // Both `work` and `stop` are present in the seeded prefix, so
        // re-execution would show two `step_started` entries for them.
        let work_started_iter1 = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_started")
                    && event_step_name(e) == Some("work")
                    && scope_context_of(e).is_some_and(|(_, it, _)| it == 1)
            })
            .count();
        assert_eq!(
            work_started_iter1, 1,
            "iter 1 work must not re-start on replay"
        );
        let stop_started_iter1 = evs
            .iter()
            .filter(|e| {
                event_kind(e) == Some("step_started")
                    && event_step_name(e) == Some("stop")
                    && scope_context_of(e).is_some_and(|(_, it, _)| it == 1)
            })
            .count();
        assert_eq!(
            stop_started_iter1, 1,
            "iter 1 stop must not re-start on replay"
        );

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

// ---------------------------------------------------------------------------
// Failure-attribution tests for scope containers. The scope-body step that
// raised the failure emits its own scoped `StepFailed`, but the log is the
// source of truth for post-crash progress reconstruction — so the container
// must also emit a top-level `StepFailed`. Without it,
// `progress_from_log().last_failed_step` attributes the failure to the
// scope-body step rather than the container, distorting dashboard/CLI
// visibility and replay.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scoped_body_failure_emits_container_step_failed_at_top_level() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Non-first container: a preamble cmd runs, THEN the call runs
        // a scope body whose cmd exits non-zero. Two facts must land in
        // the log: the scope-body step's scoped `StepFailed` AND the
        // container's top-level `StepFailed`. The in-memory outcome
        // already carries the container name; this test pins the log
        // shape so post-crash replay agrees with it.
        let wf = workflow_with_scope(
            "scoped-fail",
            vec![
                Step::cmd("preamble", "echo ready"),
                Step::call("greet", "greeter"),
            ],
            "greeter",
            vec![Step::cmd("bad", "false")],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            echo_executor(),
        );
        match engine.resume().await.expect("resume") {
            StepOutcome::StepFailed { step_name, .. } => {
                assert_eq!(step_name, "greet");
            }
            other => panic!("expected StepFailed(greet), got {other:?}"),
        }

        let evs = events(&engine).await;
        // `Event::StepFailed` has no `scope_context` field — scoped and
        // top-level failures are distinguished only by `step_name`. The
        // body's `bad` failure must land first, then the container
        // `greet`'s top-level failure. `progress_from_log().last_failed_step`
        // scans forward and keeps the last hit, so the container name
        // ends up authoritative (pre-fix it stopped at `bad`).
        let failed_in_order: Vec<&str> = evs
            .iter()
            .filter(|e| event_kind(e) == Some("step_failed"))
            .filter_map(event_step_name)
            .collect();
        assert_eq!(failed_in_order, vec!["bad", "greet"]);
        assert_eq!(failed_in_order.last().copied(), Some("greet"));
    });
}

#[tokio::test]
async fn scoped_cmd_timeout_in_non_first_container_attributes_step_index() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Preamble at top-level index 0, container at index 1. The
        // scoped body cmd has a 10ms timeout and the SleepExecutor
        // sleeps 200ms — timeout always wins.
        //
        // Pre-fix, run_call/run_repeat/run_map hardcoded
        // `EngineError::StepFailed.step_index = 0` in the TimedOut arm,
        // so the caller read the preamble's index instead of the
        // container's. This test pins both the returned error's index
        // and the `step_timeout_fired` event's index (the latter was
        // already correct via `top_level_position_of`, but is asserted
        // here as defence-in-depth).
        let mut body_cmd = Step::cmd("slow", "echo whatever");
        body_cmd.timeout = Some(Duration::from_millis(10));

        let wf = workflow_with_scope(
            "scoped-timeout",
            vec![
                Step::cmd("preamble", "echo ready"),
                Step::call("greet", "greeter"),
            ],
            "greeter",
            vec![body_cmd],
            None,
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            sleep_executor(),
        );
        match engine.resume().await.expect_err("expected timeout error") {
            EngineError::StepFailed { step_index, reason } => {
                assert_eq!(
                    step_index, 1,
                    "step_index must point at the container (preamble=0, greet=1)"
                );
                match reason {
                    TerminationReason::StepTimeout { configured_ms } => {
                        assert_eq!(configured_ms, 10);
                    }
                    other => panic!("expected StepTimeout, got {other:?}"),
                }
            }
            other => panic!("expected EngineError::StepFailed, got {other:?}"),
        }

        let evs = events(&engine).await;
        let timeout_evt = evs
            .iter()
            .find(|e| event_kind(e) == Some("step_timeout_fired"))
            .expect("step_timeout_fired event");
        let step_idx = timeout_evt
            .payload
            .get("step_index")
            .and_then(|v| v.as_u64())
            .expect("step_index field");
        assert_eq!(step_idx, 1);
    });
}
