//! Integration tests for the v2 `parallel` container — PR 6 of Task #31.
//!
//! v1 parity (`src/steps/parallel.rs`):
//! * Every sub-step runs concurrently via `tokio::task::JoinSet`.
//! * The first sub-step error aborts the rest; the container surfaces that
//!   error as the top-level failure.
//! * On success, the synthetic top-level output is the LAST sub-step's
//!   output by **definition order** (not completion order) so that
//!   downstream `{{ steps.<parallel>.stdout }}` references are deterministic.
//!
//! Adapter side (Option A, see `src/cli/harness_adapter.rs`): the YAML
//! `steps:` list under a parallel step is synthesised into a hidden scope
//! named `__parallel_<top_level_index>`. The harness sees parallel as just
//! another scope-bodied container, so these tests build the synth scope
//! by hand.
//!
//! Requires PostgreSQL via `STEPYARD_HARNESS_DATABASE_URL` (mirrors
//! `tests/scope_replay.rs`); without it each test silently skips.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    Engine, HarnessConfig, Scope, Step, StepExecutor, StepOutcome, Workflow,
};
use stepyard_sandbox_orchestrator::{ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle};
use stepyard_session::{migrate, Session, SessionEvent};
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

/// Echo executor identical in shape to `tests/scope_replay.rs`.
#[derive(Default, Clone)]
struct EchoExecutor;

#[async_trait]
impl StepExecutor for EchoExecutor {
    async fn execute(&self, session_id: Uuid, step: &Step) -> Result<ExecOutput, SandboxError> {
        self.execute_with_env(session_id, step, &HashMap::new()).await
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

fn workflow_with_parallel(
    workflow_name: &str,
    parallel_name: &str,
    scope_name: &str,
    body: Vec<Step>,
) -> Workflow {
    let mut wf = Workflow::new(workflow_name, vec![Step::parallel(parallel_name, scope_name)]);
    wf.scopes.insert(
        scope_name.into(),
        Scope {
            steps: body,
            outputs: None,
        },
    );
    wf
}

#[tokio::test]
async fn parallel_with_one_cmd_sub_step_completes() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Single sub-step is the trivial smoke test: the scope runner
        // spawns a JoinSet with one task, drains it, and synthesises a
        // top-level StepCompleted whose output mirrors that single task.
        let wf = workflow_with_parallel(
            "parallel-single",
            "p",
            "__parallel_0",
            vec![Step::cmd("a", "echo hi")],
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

        // Scoped sub-step ran exactly once and reported completion under
        // the parallel container's scope_context.
        let scoped_a_done = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("a")
                && scope_context_of(e)
                    .is_some_and(|(c, _, _)| c == "p")
        });
        assert!(scoped_a_done, "scoped sub-step `a` must have completed under container `p`");

        // Top-level container completion must exist (no scope_context) and
        // carry the synthetic stdout from the last (only) sub-step.
        let container_done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("p")
                    && scope_context_of(e).is_none()
            })
            .expect("container top-level completion");
        let output = container_done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("synthetic stdout");
        assert!(output.contains("hi"), "got {output}");
    });
}

#[tokio::test]
async fn parallel_with_two_cmds_runs_both_and_synthesizes_definition_last() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Two sub-steps in declaration order [a, b]. v1 parity: the
        // synthetic top-level stdout MUST come from `b` (definition-
        // order last), not from whichever finishes first. With the
        // EchoExecutor both finish ~instantly so completion order is
        // racy, which is exactly the point — the contract is order-
        // independent because position lookup is definition-based.
        let wf = workflow_with_parallel(
            "parallel-pair",
            "p",
            "__parallel_0",
            vec![Step::cmd("a", "echo aaa"), Step::cmd("b", "echo bbb")],
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

        // Both scoped sub-steps reported step_completed under container
        // `p` at iteration 0 with positions {0, 1}.
        let mut scoped_positions: Vec<u64> = evs
            .iter()
            .filter_map(|e| {
                if event_kind(e) != Some("step_completed") {
                    return None;
                }
                let (c, iter, pos) = scope_context_of(e)?;
                if c == "p" && iter == 0 {
                    Some(pos)
                } else {
                    None
                }
            })
            .collect();
        scoped_positions.sort();
        assert_eq!(
            scoped_positions,
            vec![0, 1],
            "both sub-steps must log step_completed under `p`"
        );

        // Top-level synthetic stdout must be from `b` (definition-last),
        // never `a`. Mirrors v1's `nested_steps.last()` lookup.
        let container_done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed")
                    && event_step_name(e) == Some("p")
                    && scope_context_of(e).is_none()
            })
            .expect("container top-level completion");
        let output = container_done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("synthetic stdout");
        assert!(
            output.contains("bbb"),
            "synthetic stdout must come from definition-last sub-step `b`; got {output}"
        );
        assert!(
            !output.contains("aaa"),
            "synthetic stdout must NOT include earlier sub-step `a`; got {output}"
        );
    });
}

#[tokio::test]
async fn parallel_with_one_failure_fails_container() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Three sub-steps; middle one is `false` (exit_code 1). v1
        // semantics: first failure wins. The container must surface a
        // top-level StepFailed and MUST NOT emit a top-level
        // StepCompleted. With instant executor, the surviving sub-steps
        // typically still complete (abort_all is a no-op against
        // already-finished tasks) — the test asserts the container-
        // level state machine, not whether sibling tasks were physically
        // killed mid-flight.
        let wf = workflow_with_parallel(
            "parallel-with-failure",
            "p",
            "__parallel_0",
            vec![
                Step::cmd("a", "echo aaa"),
                Step::cmd("b", "false"),
                Step::cmd("c", "echo ccc"),
            ],
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
            StepOutcome::StepFailed { step_name, .. } => {
                assert_eq!(step_name, "p", "container `p` must be the failed step");
            }
            other => panic!("expected StepOutcome::StepFailed for `p`, got {other:?}"),
        }

        let evs = events(&engine).await;

        // Container must NOT have a top-level step_completed.
        let container_completed = evs.iter().any(|e| {
            event_kind(e) == Some("step_completed")
                && event_step_name(e) == Some("p")
                && scope_context_of(e).is_none()
        });
        assert!(
            !container_completed,
            "container `p` must NOT emit a top-level step_completed when a sub-step failed"
        );

        // Top-level step_failed for `p` mentions sub-step `b`.
        let container_failed = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_failed")
                    && event_step_name(e) == Some("p")
                    && scope_context_of(e).is_none()
            })
            .expect("container top-level step_failed");
        let err = container_failed
            .payload
            .get("error")
            .and_then(|v| v.as_str())
            .expect("error string");
        assert!(
            err.contains("`b`"),
            "container error should mention failing sub-step `b`; got: {err}"
        );
    });
}
