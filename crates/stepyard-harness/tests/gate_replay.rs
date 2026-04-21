//! Integration tests for the gate executor and the outputs-map rebuild
//! that survives a process crash. PR 2 of Task #31.
//!
//! Requires a PostgreSQL reachable via `STEPYARD_HARNESS_DATABASE_URL`.
//! Tests skip (without failing) when the env var is not set — mirrors
//! `tests/step_resume.rs` so CI without a database sidecar stays green.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    Engine, HarnessConfig, RunContext, Step, StepExecutor, StepOutcome, Workflow,
};
use stepyard_sandbox_orchestrator::{ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle};
use stepyard_session::{migrate, Session, SessionStatus};
use tokio::sync::Mutex;
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

/// Test-only executor that returns preset [`ExecOutput`]s keyed by step
/// name. Avoids widening `MockLifecycle::exec_with_env` just to drive the
/// PR 2 gate tests — the executor trait already exists so callers can
/// swap in their own backend for assertions like these.
#[derive(Default, Clone)]
struct ScriptedExecutor {
    responses: Arc<Mutex<HashMap<String, ExecOutput>>>,
}

impl ScriptedExecutor {
    fn new() -> Self {
        Self::default()
    }

    async fn preset(&self, step_name: &str, out: ExecOutput) {
        self.responses
            .lock()
            .await
            .insert(step_name.to_string(), out);
    }
}

#[async_trait]
impl StepExecutor for ScriptedExecutor {
    async fn execute(
        &self,
        _session_id: Uuid,
        step: &Step,
    ) -> Result<ExecOutput, SandboxError> {
        self.execute_with_env(_session_id, step, &HashMap::new()).await
    }

    async fn execute_with_env(
        &self,
        _session_id: Uuid,
        step: &Step,
        _env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        let mut responses = self.responses.lock().await;
        Ok(responses.remove(&step.name).unwrap_or(ExecOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        }))
    }
}

fn lifecycle() -> Arc<dyn SandboxLifecycle> {
    Arc::new(MockLifecycle::new())
}

fn build_then_gate(condition: &str, on_pass: &str, on_fail: &str) -> Workflow {
    let mut gate = Step::gate("check", condition);
    gate.on_pass = Some(on_pass.into());
    gate.on_fail = Some(on_fail.into());
    Workflow::new(
        "gate-flow",
        vec![Step::cmd("build", "echo build"), gate],
    )
}

/// Collect the `event` discriminator string for every session log entry.
async fn event_names(engine: &Engine) -> Vec<String> {
    engine
        .session()
        .replay()
        .await
        .expect("replay")
        .iter()
        .filter_map(|e| {
            e.payload
                .get("event")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect()
}

#[tokio::test]
async fn gate_passes_when_condition_renders_truthy_and_continues() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");
        let executor = Arc::new(ScriptedExecutor::new());
        executor
            .preset(
                "build",
                ExecOutput {
                    stdout: "ready\n".into(),
                    stderr: String::new(),
                    exit_code: 0,
                },
            )
            .await;

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            build_then_gate("{{ steps.build.exit_code }} == 0", "continue", "fail"),
            lifecycle(),
            executor,
        );

        let outcome = engine.resume().await.expect("resume");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let names = event_names(&engine).await;
        assert!(
            names.windows(2).any(|w| w
                == [
                    "step_started".to_string(),
                    "step_completed".to_string()
                ]),
            "names={names:?}"
        );
        assert_eq!(names.first().map(String::as_str), Some("workflow_started"));
        assert_eq!(names.last().map(String::as_str), Some("workflow_completed"));

        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn gate_fails_when_on_fail_is_fail() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");
        let executor = Arc::new(ScriptedExecutor::new());
        executor
            .preset(
                "build",
                ExecOutput {
                    stdout: "done\n".into(),
                    stderr: String::new(),
                    exit_code: 0,
                },
            )
            .await;

        // Condition renders to `1 == 2` — Tera `one_off` returns that
        // string verbatim, which `evaluate_bool` rejects as non-boolean.
        // Use a condition that clearly renders to `false` via stdout.
        let wf = {
            let mut gate = Step::gate("check", "{{ steps.build.stdout }}");
            gate.on_pass = Some("continue".into());
            gate.on_fail = Some("fail".into());
            gate.message = Some("build output was not truthy".into());
            Workflow::new(
                "gate-fail",
                vec![
                    Step::cmd(
                        "build",
                        "echo no", // stdout will be "no\n" → falsy
                    ),
                    gate,
                ],
            )
        };
        // Make the scripted executor report a falsy stdout for `build`.
        executor
            .preset(
                "build",
                ExecOutput {
                    stdout: "no\n".into(),
                    stderr: String::new(),
                    exit_code: 0,
                },
            )
            .await;

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            executor,
        );

        let outcome = engine.resume().await.expect("resume");
        match outcome {
            StepOutcome::StepFailed { ref step_name, ref error } => {
                assert_eq!(step_name, "check");
                assert!(
                    error.contains("build output was not truthy"),
                    "error={error}"
                );
            }
            other => panic!("expected StepFailed, got {other:?}"),
        }

        let names = event_names(&engine).await;
        assert!(
            names.iter().any(|n| n == "step_failed"),
            "names={names:?}"
        );

        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Failed);
    });
}

#[tokio::test]
async fn gate_cross_step_refs_survive_process_crash_and_resume() {
    db_test!(pool, {
        // Phase 1: run the cmd step, drop the engine (== simulated crash).
        let tenant = Uuid::new_v4();
        let session = Session::new(&pool, tenant, "edenred".into())
            .await
            .expect("new session");
        let session_id = session.id();
        let executor = Arc::new(ScriptedExecutor::new());
        executor
            .preset(
                "build",
                ExecOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                },
            )
            .await;

        let wf = build_then_gate("{{ steps.build.exit_code }} == 0", "continue", "fail");

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf.clone(),
            lifecycle(),
            executor,
        );
        let first = engine.step().await.expect("step 1");
        assert!(matches!(first, StepOutcome::StepCompleted { ref step_name } if step_name == "build"));
        drop(engine); // crash boundary — all state now lives in the log.

        // Phase 2: reconstruct a fresh engine over the same session. The
        // outputs map must be rebuilt from the log so the gate sees
        // `steps.build.exit_code`.
        let session2 = Session::load(&pool, session_id).await.expect("reload");
        let executor2 = Arc::new(ScriptedExecutor::new());
        let mut engine2 = Engine::with_executor(
            HarnessConfig::default(),
            session2,
            wf,
            lifecycle(),
            executor2,
        );
        let outcome = engine2.resume().await.expect("resume after crash");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}

#[tokio::test]
async fn gate_renders_target_from_run_context() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");
        let executor = Arc::new(ScriptedExecutor::new());

        let wf = {
            let mut gate = Step::gate("check", "{{ target }}");
            gate.on_pass = Some("continue".into());
            gate.on_fail = Some("fail".into());
            Workflow::new("target-gate", vec![gate])
        };

        let rc = RunContext {
            target: "ok".into(), // evaluate_bool accepts "ok" as truthy
            vars: HashMap::new(),
        };
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            executor,
        )
        .with_run_context(rc);

        let outcome = engine.resume().await.expect("resume");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);
    });
}

#[tokio::test]
async fn gate_missing_condition_is_structured_step_failure() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");
        let executor = Arc::new(ScriptedExecutor::new());

        // Bypass the adapter (which rejects missing condition at parse
        // time) by constructing the harness Step directly. This exercises
        // the engine-side defensive path.
        let gate = Step {
            name: "naked".into(),
            kind: stepyard_harness::StepKind::Gate,
            command: String::new(),
            timeout: None,
            env: HashMap::new(),
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: None,
            max_iterations: None,
            initial_value: None,
            items: None,
            parallel: None,
            outputs: None,
            prompt: None,
        };
        let wf = Workflow::new("naked-gate", vec![gate]);

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            executor,
        );
        let outcome = engine.resume().await.expect("resume");
        match outcome {
            StepOutcome::StepFailed { error, .. } => {
                assert!(error.contains("missing `condition:`"), "error={error}");
            }
            other => panic!("expected StepFailed, got {other:?}"),
        }
    });
}

#[tokio::test]
async fn gate_non_boolean_condition_is_structured_step_failure() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");
        let executor = Arc::new(ScriptedExecutor::new());

        let wf = Workflow::new(
            "bad-bool-gate",
            vec![Step::gate("check", "maybe later")],
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            executor,
        );
        let outcome = engine.resume().await.expect("resume");
        match outcome {
            StepOutcome::StepFailed { error, .. } => {
                assert!(
                    error.contains("truthy/falsy token"),
                    "error={error}"
                );
            }
            other => panic!("expected StepFailed, got {other:?}"),
        }
    });
}

#[tokio::test]
async fn malformed_output_in_log_fails_replay_loudly() {
    // Absent `output` must stay OK (older log entries + gate completions
    // both look that way). A *present but malformed* `output` must fail
    // loudly — the outputs map now participates in replay correctness,
    // so silently dropping it would let a gate in the rerun see a
    // different context than the gate in the original run.
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Poison the log with a step_completed payload whose `output`
        // field is a string instead of a StepOutputSnapshot object.
        session
            .append(serde_json::json!({
                "event": "step_completed",
                "step_index": 0,
                "step_name": "build",
                "step_type": "cmd",
                "duration_ms": 1,
                "timestamp": "2025-01-01T00:00:00Z",
                "sandboxed": true,
                "output": "not-a-snapshot-object",
            }))
            .await
            .expect("poison log");

        let executor = Arc::new(ScriptedExecutor::new());
        let wf = build_then_gate("{{ steps.build.exit_code }} == 0", "continue", "fail");
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            executor,
        );

        let err = engine
            .step()
            .await
            .expect_err("malformed output must surface as an engine error");
        let msg = err.to_string();
        assert!(
            msg.contains("malformed `output`"),
            "expected replay to fail loudly, got: {msg}"
        );
    });
}
