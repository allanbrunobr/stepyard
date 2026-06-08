//! Integration tests for the `template` step executor. PR 4 of Task #31.
//!
//! Requires a PostgreSQL reachable via `STEPYARD_HARNESS_DATABASE_URL`. Tests
//! skip (without failing) when the env var is not set — mirrors
//! `tests/gate_replay.rs` / `tests/scope_replay.rs` so CI without a database
//! sidecar stays green.

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

/// Executor that panics if invoked. Template steps never call the executor —
/// they read a file and render via Tera. If this fires, dispatch is wrong.
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
            "template dispatch must never invoke the step executor; got step `{}`",
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

/// Build a workflow whose `prompts_dir` points at `dir` and whose top-level
/// steps come from `steps`. The string-path conversion uses `display()`
/// (lossy on non-UTF-8 paths), which is fine for test tempdirs.
fn workflow_with_prompts_dir(name: &str, dir: &std::path::Path, steps: Vec<Step>) -> Workflow {
    let mut wf = Workflow::new(name, steps);
    wf.prompts_dir = Some(dir.to_string_lossy().into_owned());
    wf
}

// ---------------------------------------------------------------------------
// Happy-path: template renders against the harness render context.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_renders_against_target_and_vars() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("greet.md.tera"),
            "Hello {{ target }} from {{ vars.who }}",
        )
        .unwrap();

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let wf = workflow_with_prompts_dir(
            "template-basic",
            tmp.path(),
            vec![Step::template("greet", None)],
        );

        let mut vars = HashMap::new();
        vars.insert("who".into(), "bruno".into());
        let rc = RunContext {
            target: "edenred".into(),
            vars,
        };
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            unreachable_executor(),
        )
        .with_run_context(rc);

        let outcome = engine.resume().await.expect("resume");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        // The StepCompleted event for the template step carries the
        // rendered text on `output.stdout`, with exit_code=0 and an empty
        // stderr — the unified cmd-shape snapshot.
        let evs = events(&engine).await;
        let done = evs
            .iter()
            .find(|e| {
                event_kind(e) == Some("step_completed") && event_step_name(e) == Some("greet")
            })
            .expect("template step_completed event");
        let output = done.payload.get("output").expect("output on template");
        assert_eq!(
            output.get("stdout").and_then(|v| v.as_str()),
            Some("Hello edenred from bruno")
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
// Cross-step ref: a gate after the template reads `{{ steps.tmpl.stdout }}`.
// Proves the unified output shape makes template output referenceable with
// no new event-schema variant.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_output_reachable_via_cross_step_ref() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        // Render to the literal string `ok`, which `evaluate_bool` accepts
        // as truthy. A trailing newline would not — keep it bare.
        std::fs::write(tmp.path().join("tmpl.md.tera"), "ok").unwrap();

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let mut gate = Step::gate("check", "{{ steps.tmpl.stdout }}");
        gate.on_pass = Some("continue".into());
        gate.on_fail = Some("fail".into());

        let wf = workflow_with_prompts_dir(
            "template-xref",
            tmp.path(),
            vec![Step::template("tmpl", None), gate],
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

        // Both steps completed — the gate could only resolve truthy if
        // the template's stdout landed in the outputs map.
        let evs = events(&engine).await;
        let names: Vec<&str> = evs
            .iter()
            .filter(|e| event_kind(e) == Some("step_completed"))
            .filter_map(event_step_name)
            .collect();
        assert_eq!(names, vec!["tmpl", "check"]);
    });
}

// ---------------------------------------------------------------------------
// Error path: prompt file missing → structured StepFailed, session failed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_missing_file_emits_step_failed() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        // No file written — resolution succeeds lexically but read_to_string
        // fails with ENOENT.

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let wf = workflow_with_prompts_dir(
            "template-missing",
            tmp.path(),
            vec![Step::template("absent", None)],
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
                assert_eq!(step_name, "absent");
                assert!(error.contains("not found"), "error={error}");
                assert!(
                    error.contains("absent.md.tera"),
                    "error should name the missing file: {error}"
                );
            }
            other => panic!("expected StepFailed, got {other:?}"),
        }

        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Failed);
    });
}

// ---------------------------------------------------------------------------
// Error path: prompt resolves to a path with `..` → rejected lexically with
// no filesystem access, before read_to_string. The template file DOES exist
// relative to the rendered path, so a missing guardrail would let the read
// succeed and silently exfiltrate the file's content.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_rejects_path_traversal() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        let prompts_dir = tmp.path().join("prompts");
        std::fs::create_dir_all(&prompts_dir).unwrap();
        // Place a "secret" template alongside `prompts/`. `../secret.md.tera`
        // would resolve to it if we did lexical joining without guarding,
        // proving the guard actually prevents escape.
        std::fs::write(tmp.path().join("secret.md.tera"), "leaked").unwrap();

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // The rendered prompt field is `../secret`, which `resolve_template_path`
        // rejects at the `Component::ParentDir` check.
        let wf = workflow_with_prompts_dir(
            "template-traversal",
            &prompts_dir,
            vec![Step::template("exfil", Some("../secret".into()))],
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
                assert_eq!(step_name, "exfil");
                assert!(
                    error.contains("rejected") && error.contains("relative path"),
                    "error should mention the traversal guard: {error}"
                );
                assert!(
                    !error.contains("leaked"),
                    "error must not leak template content: {error}"
                );
            }
            other => panic!("expected StepFailed, got {other:?}"),
        }

        let reloaded = Session::load(&pool, engine.session().id()).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Failed);
    });
}

// ---------------------------------------------------------------------------
// Two-pass render: the `prompt` field itself is a Tera expression that
// resolves to a subdirectory-qualified basename; the file under that
// basename is then rendered. Matches v1 `src/steps/template_step.rs:32-36`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_dynamic_prompt_renders_basename() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        let subdir = tmp.path().join("fix-lint");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("react.md.tera"), "fixing {{ vars.stack }}").unwrap();

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        // Step name is `tmpl`, but the `prompt` field renders to
        // `fix-lint/react`, so we load `prompts/fix-lint/react.md.tera`.
        let wf = workflow_with_prompts_dir(
            "template-dynamic",
            tmp.path(),
            vec![Step::template(
                "tmpl",
                Some("fix-lint/{{ vars.stack }}".into()),
            )],
        );

        let mut vars = HashMap::new();
        vars.insert("stack".into(), "react".into());
        let rc = RunContext {
            target: "app".into(),
            vars,
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
            .find(|e| event_kind(e) == Some("step_completed") && event_step_name(e) == Some("tmpl"))
            .expect("template step_completed");
        let stdout = done
            .payload
            .get("output")
            .and_then(|v| v.get("stdout"))
            .and_then(|v| v.as_str())
            .expect("template stdout");
        assert_eq!(stdout, "fixing react");
    });
}

// ---------------------------------------------------------------------------
// Replay: a session whose log already contains a StepCompleted for the
// template must NOT re-read the file. We simulate this by deleting the
// prompt file between phase 1 and phase 2 — if replay skipped the step,
// phase 2 completes cleanly; if it re-entered, the missing file would
// surface as a StepFailed. Mirrors the gate cross-step-refs replay test.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn template_replay_skips_completed_step() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("replay.md.tera");
        std::fs::write(&file, "once").unwrap();

        let tenant = Uuid::new_v4();
        let session = Session::new(&pool, tenant, "edenred".into())
            .await
            .expect("new session");
        let session_id = session.id();

        // Phase 1 — run the template step, drop the engine (== simulated
        // crash). The template completion is now in the log.
        let wf = workflow_with_prompts_dir(
            "template-replay",
            tmp.path(),
            vec![Step::template("replay", None)],
        );

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf.clone(),
            lifecycle(),
            unreachable_executor(),
        );
        let first = engine.step().await.expect("step 1");
        assert!(
            matches!(first, StepOutcome::StepCompleted { ref step_name } if step_name == "replay")
        );
        drop(engine);

        // Delete the prompt file — if replay re-entered the template
        // executor it would surface as FileNotFound, failing the test.
        std::fs::remove_file(&file).unwrap();

        // Phase 2 — reconstruct a fresh engine. progress_from_log sees the
        // existing StepCompleted and advances past it; resume() reaches
        // WorkflowCompleted without re-reading the template file.
        let session2 = Session::load(&pool, session_id).await.expect("reload");
        let mut engine2 = Engine::with_executor(
            HarnessConfig::default(),
            session2,
            wf,
            lifecycle(),
            unreachable_executor(),
        );
        let outcome = engine2.resume().await.expect("resume after crash");
        assert_eq!(outcome, StepOutcome::WorkflowCompleted);

        let reloaded = Session::load(&pool, session_id).await.unwrap();
        assert_eq!(reloaded.status(), SessionStatus::Completed);
    });
}
