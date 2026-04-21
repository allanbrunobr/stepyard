//! Integration tests for the `script` step executor. PR 4 of Task #31
//! (commit 2).
//!
//! Requires a PostgreSQL reachable via `STEPYARD_HARNESS_DATABASE_URL`. Tests
//! skip (without failing) when the env var is not set — mirrors
//! `tests/template_replay.rs` / `tests/gate_replay.rs` so CI without a
//! database sidecar stays green.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    Engine, HarnessConfig, RunContext, Step, StepExecutor, StepOutcome, Workflow,
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

/// Executor that panics if invoked. Script steps never call the executor —
/// they evaluate Rhai in-process. If this fires, dispatch is wrong.
#[derive(Default, Clone)]
struct UnreachableExecutor;

#[async_trait]
impl StepExecutor for UnreachableExecutor {
    async fn execute(&self, session_id: Uuid, step: &Step) -> Result<ExecOutput, SandboxError> {
        self.execute_with_env(session_id, step, &HashMap::new()).await
    }

    async fn execute_with_env(
        &self,
        _session_id: Uuid,
        step: &Step,
        _env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        panic!(
            "script dispatch must never invoke the step executor; got step `{}`",
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

// ---------------------------------------------------------------------------
// Happy-path: a script returns a string, which lands on the StepCompleted
// event as `output.stdout`. `stderr` is elided (empty), `exit_code` is 0 —
// the unified cmd-shape snapshot that cross-step refs consume.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_renders_value_into_stdout() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let wf = Workflow::new(
            "script-basic",
            vec![Step::script("compute", r#""hello " + "rhai""#)],
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            unreachable_executor(),
        );
        let outcome = engine.resume().await.expect("resume");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let evs = events(&engine).await;
        let done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed") && event_step_name(e) == Some("compute")
            })
            .expect("script step_completed event");
        let output = done.payload.get("output").expect("output on script");
        assert_eq!(
            output.get("stdout").and_then(|v| v.as_str()),
            Some("hello rhai")
        );
        // `StepOutputSnapshot` elides empty stderr via `skip_serializing_if`,
        // so the JSON payload must NOT have a `stderr` field when it's "".
        assert!(
            output.get("stderr").is_none(),
            "empty stderr must be elided from the snapshot payload, got {output:?}"
        );
        assert_eq!(output.get("exit_code").and_then(|v| v.as_i64()), Some(0));

        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

// ---------------------------------------------------------------------------
// Cross-step ref via the outputs map: a cmd step emits stdout, a later
// script step reads it through `ctx_get("step.stdout")`. Proves the flat
// snapshot the script executor builds is keyed the same way the scope
// runner names outputs (v1 parity).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_ctx_get_reads_prior_step_output() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Fake cmd executor that returns "42" on stdout. The script below
        // reads that value via ctx_get and checks it.
        #[derive(Clone)]
        struct FixedStdoutExecutor;
        #[async_trait]
        impl StepExecutor for FixedStdoutExecutor {
            async fn execute(
                &self,
                session_id: Uuid,
                step: &Step,
            ) -> Result<ExecOutput, SandboxError> {
                self.execute_with_env(session_id, step, &HashMap::new()).await
            }
            async fn execute_with_env(
                &self,
                _session_id: Uuid,
                _step: &Step,
                _env: &HashMap<String, String>,
            ) -> Result<ExecOutput, SandboxError> {
                Ok(ExecOutput {
                    stdout: "42".into(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            }
        }

        let wf = Workflow::new(
            "script-xref",
            vec![
                Step::cmd("prev", "echo 42"),
                Step::script("use_prev", r#"ctx_get("prev.stdout")"#),
            ],
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            Arc::new(FixedStdoutExecutor),
        );
        let outcome = engine.resume().await.expect("resume");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let evs = events(&engine).await;
        let done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed") && event_step_name(e) == Some("use_prev")
            })
            .expect("script step_completed");
        let stdout = done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("script stdout");
        assert_eq!(stdout, "42");
    });
}

// ---------------------------------------------------------------------------
// Target is exposed via `ctx_get("target")`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_ctx_get_reads_target() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let wf = Workflow::new(
            "script-target",
            vec![Step::script("show", r#"ctx_get("target")"#)],
        );

        let rc = RunContext {
            target: "edenred".into(),
            vars: HashMap::new(),
        };
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            unreachable_executor(),
        )
        .with_run_context(rc);
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let evs = events(&engine).await;
        let done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed") && event_step_name(e) == Some("show")
            })
            .expect("script step_completed");
        let stdout = done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("script stdout");
        assert_eq!(stdout, "edenred");
    });
}

// ---------------------------------------------------------------------------
// Error path: a runtime Rhai error (`throw`) bubbles up as a structured
// StepFailed and flips the session to Failed. The error message is
// surfaced through the thiserror wrapper — operators see why the script
// blew up, not just "script error".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_runtime_error_emits_step_failed() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let wf = Workflow::new(
            "script-error",
            vec![Step::script("boom", r#"throw "kaboom""#)],
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            unreachable_executor(),
        );
        let outcome = engine.resume().await.expect("resume");
        match outcome {
            StepOutcome::StepFailed { step_name, error } => {
                assert_eq!(step_name, "boom");
                assert!(
                    error.to_lowercase().contains("script"),
                    "error should classify as script: {error}"
                );
                assert!(
                    error.contains("kaboom"),
                    "error should surface the Rhai message: {error}"
                );
            }
            other => panic!("expected StepFailed, got {other:?}"),
        }

        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Failed);
    });
}

// ---------------------------------------------------------------------------
// Replay: a session whose log already contains a StepCompleted for the
// script must NOT re-evaluate. We simulate this by swapping the Rhai
// source between phase 1 and phase 2 — if replay re-entered, the second
// run would either produce a different stdout or blow up on the new
// source; if it skipped, phase 2 reaches WorkflowCompleted unchanged.
// Mirrors the template replay test.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn script_replay_skips_completed_step() {
    db_test!(pool, {
        let tenant = Uuid::new_v4();
        let session = Session::new(&pool, tenant, "edenred".into())
            .await
            .expect("new session");
        let session_id = session.id();

        // Phase 1 — run the script with a valid source, drop the engine
        // (simulated crash). The completion event is now in the log.
        let wf1 = Workflow::new(
            "script-replay",
            vec![Step::script("once", r#""first""#)],
        );
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf1,
            lifecycle(),
            unreachable_executor(),
        );
        let first = engine.step().await.expect("step 1");
        assert!(
            matches!(first, StepOutcome::StepCompleted { ref step_name } if step_name == "once")
        );
        drop(engine);

        // Phase 2 — swap the source for one that would blow up at parse
        // time if it were re-evaluated. progress_from_log must advance
        // past the completed step and reach WorkflowCompleted without
        // re-entering the script executor.
        let wf2 = Workflow::new(
            "script-replay",
            vec![Step::script("once", r#"!!! invalid rhai !!!"#)],
        );
        let session2 = Session::load(&pool, session_id).await.expect("reload");
        let mut engine2 = Engine::with_executor(
            HarnessConfig::default(),
            session2,
            wf2,
            lifecycle(),
            unreachable_executor(),
        );
        let outcome = engine2.resume().await.expect("resume after crash");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}
