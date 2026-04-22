//! Integration tests for the `agent` step executor. PR 5a commit 3b of Task #31.
//!
//! Every test shells out to a mock Claude CLI under
//! `tests/fixtures/mock_claude.sh` that drains stdin (avoiding an EPIPE
//! race on slow CI runners — see task #51) and then emits a single
//! fixed stream-JSON `result` event so the harness's parse loop runs
//! end-to-end. Tests that need to assert on the exact argv the harness
//! passed to the CLI thread `MOCK_CLAUDE_ARGV_FILE` through each step's
//! `env:` map — the mock writes every argv element one-per-line to that
//! file before draining stdin. Each step gets a DISTINCT argv file: the
//! fixture truncates on open, so sharing a path across two agent steps
//! would clobber step 1's capture and hide a whole class of bugs (argv
//! wrongly attached to the wrong step).
//!
//! Requires Postgres via `STEPYARD_HARNESS_DATABASE_URL`; mirrors the
//! DB skip-gate macro used in sister tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{Engine, HarnessConfig, Step, StepExecutor, StepOutcome, Workflow};
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

/// Executor that panics if invoked. Agent steps never call the sandbox
/// step executor — they spawn the Claude CLI directly. If this fires,
/// dispatch is wrong.
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
            "agent dispatch must never invoke the step executor; got step `{}`",
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

/// Repo-root path of the mock Claude fixture. `CARGO_MANIFEST_DIR` is
/// the crate root (`crates/stepyard-harness`) at build time, so this
/// resolves deterministically whether tests run from the workspace root
/// or the crate directory.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_claude.sh")
}

fn read_argv_file(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("argv file {}: {e}", path.display()))
        .lines()
        .map(str::to_string)
        .collect()
}

/// Build an agent step pointed at the mock fixture with a per-step
/// argv-capture sidecar. The `agent_command` pin keeps tests hermetic
/// even when a real `claude` binary is on the developer's PATH.
fn agent_step_with_argv_capture(name: &str, prompt: &str, argv_file: &Path) -> Step {
    let mut step = Step::agent(name, prompt);
    step.agent_command = Some(fixture_path().to_string_lossy().into_owned());
    step.env.insert(
        "MOCK_CLAUDE_ARGV_FILE".into(),
        argv_file.to_string_lossy().into_owned(),
    );
    step
}

/// Agent step without argv capture — for tests that don't assert on argv.
fn agent_step(name: &str, prompt: &str) -> Step {
    let mut step = Step::agent(name, prompt);
    step.agent_command = Some(fixture_path().to_string_lossy().into_owned());
    step
}

// ---------------------------------------------------------------------------
// Happy path: a single agent step runs the mock CLI end-to-end. Asserts both
// halves of the PR's metadata design — event-level tokens/cost/session_id
// AND the snapshot shape that cross-step refs read via `steps.*.stdout`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_happy_path_populates_event_metadata_and_snapshot() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let wf = Workflow::new("agent-happy", vec![agent_step("ask", "Hello")]);

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
            .find(|e| event_kind(e) == Some("step_completed") && event_step_name(e) == Some("ask"))
            .expect("agent step_completed event");

        // Event-level metadata — the whole point of Option B (tokens /
        // cost / session_id at the event layer rather than buried in the
        // snapshot). Fixture emits input_tokens=10, output_tokens=20,
        // cost_usd=0.001, session_id="mock-session-123".
        assert_eq!(
            done.payload.get("agent_session_id").and_then(|v| v.as_str()),
            Some("mock-session-123")
        );
        assert_eq!(
            done.payload.get("input_tokens").and_then(|v| v.as_u64()),
            Some(10)
        );
        assert_eq!(
            done.payload.get("output_tokens").and_then(|v| v.as_u64()),
            Some(20)
        );
        let cost = done
            .payload
            .get("cost_usd")
            .and_then(|v| v.as_f64())
            .expect("cost_usd");
        // `0.001` is not exact in f64 — tolerance check to dodge
        // representation flake. Anything below 1e-9 is well inside what
        // serde_json→f64→serde_json preserves.
        assert!(
            (cost - 0.001).abs() < 1e-9,
            "cost_usd should be ≈0.001, got {cost}"
        );

        // Snapshot shape — stdout carries the response; stderr is elided
        // by `skip_serializing_if` on the empty string; exit_code=0 is
        // the unified cmd-shape for non-cmd kinds.
        let output = done.payload.get("output").expect("output on agent");
        assert_eq!(
            output.get("stdout").and_then(|v| v.as_str()),
            Some("Task completed successfully")
        );
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
// Explicit resume: step 2 names `resume: plan`; argv must be
// `--resume <plan-sid>` and MUST NOT contain `--fork-session`. The explicit
// target short-circuits the default-shared path that would otherwise also
// emit --fork-session.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_explicit_resume_emits_resume_only() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        let argv_plan = tmp.path().join("argv_plan.txt");
        let argv_refine = tmp.path().join("argv_refine.txt");

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let mut plan = agent_step_with_argv_capture("plan", "Plan", &argv_plan);
        let mut refine = agent_step_with_argv_capture("refine", "Refine", &argv_refine);
        refine.resume = Some("plan".into());
        // Silence the unused-mut lint for the first step — we only
        // assert on `refine`'s argv but capture `plan`'s anyway so a
        // regression that clobbers step 1's argv surfaces visibly.
        let _ = &mut plan;

        let wf = Workflow::new("agent-resume", vec![plan, refine]);

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            unreachable_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let argv = read_argv_file(&argv_refine);
        // The fixture always writes session_id=mock-session-123, so
        // `resume: plan` resolves through the session-id map to that.
        assert!(
            argv.windows(2)
                .any(|w| w == ["--resume", "mock-session-123"]),
            "expected `--resume mock-session-123` in refine argv, got {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "--fork-session"),
            "explicit resume must NOT emit --fork-session; got {argv:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Explicit fork_session: step 2 names `fork_session: plan`; argv must be
// `--fork-session --resume <plan-sid>`. This is the v2 semantic fix over v1
// (which emitted bare `--resume <id>`). Pinned with the same test name
// convention as the unit test (`explicit_fork_session_emits_fork_session_and_resume`).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_explicit_fork_session_emits_fork_session_and_resume() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        let argv_plan = tmp.path().join("argv_plan.txt");
        let argv_branch = tmp.path().join("argv_branch.txt");

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let plan = agent_step_with_argv_capture("plan", "Plan", &argv_plan);
        let mut branch = agent_step_with_argv_capture("branch", "Try alternative", &argv_branch);
        branch.fork_session = Some("plan".into());

        let wf = Workflow::new("agent-fork", vec![plan, branch]);

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            unreachable_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let argv = read_argv_file(&argv_branch);
        // Must contain the 3-element contiguous sequence, in order, as
        // the v2 semantic fix. v1 parity would have emitted only
        // `--resume <id>` here, which is why the runner (and this test)
        // pin the new shape explicitly.
        let has_triple = argv.windows(3).any(|w| {
            w == ["--fork-session", "--resume", "mock-session-123"]
        });
        assert!(
            has_triple,
            "expected `--fork-session --resume mock-session-123` contiguous in branch argv, got {argv:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Shared default post-crash: a two-step workflow where step 1 runs and the
// engine is dropped (simulated crash). A fresh engine reloads the session,
// and step 2 runs with no explicit session config — its argv MUST include
// `--fork-session --resume <step1-sid>`, proving that
// `Progress::first_agent_session_id` was reconstructed from the log and
// threaded into the argv builder's default-shared path.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_shared_default_survives_restart_via_log_replay() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        let argv_plan = tmp.path().join("argv_plan.txt");
        let argv_refine = tmp.path().join("argv_refine.txt");

        let tenant = Uuid::new_v4();
        let session = Session::new(&pool, tenant, "edenred".into())
            .await
            .expect("session");
        let session_id = session.id();

        let plan = agent_step_with_argv_capture("plan", "Plan", &argv_plan);
        let refine = agent_step_with_argv_capture("refine", "Refine", &argv_refine);
        let wf = Workflow::new("agent-shared-crash", vec![plan, refine]);

        // Phase 1: single step only. `engine.step()` returns after the
        // first StepCompleted so the log holds exactly one agent
        // completion when we drop the engine.
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf.clone(),
            lifecycle(),
            unreachable_executor(),
        );
        let first = engine.step().await.expect("step 1");
        assert!(
            matches!(first, StepOutcome::StepCompleted { ref step_name } if step_name == "plan"),
            "got {first:?}"
        );
        drop(engine);

        // Phase 2: reload and resume. step 2 runs for the first time here.
        let session2 = Session::load(&pool, session_id).await.expect("reload");
        let mut engine2 = Engine::with_executor(
            HarnessConfig::default(),
            session2,
            wf,
            lifecycle(),
            unreachable_executor(),
        );
        assert_eq!(
            engine2.resume().await.expect("resume after crash"),
            StepOutcome::WorkflowCompleted
        );

        // The argv builder's default-shared path must have read
        // first_agent_session_id from the log scan and emitted the
        // fork+resume pair.
        let argv = read_argv_file(&argv_refine);
        let has_triple = argv.windows(3).any(|w| {
            w == ["--fork-session", "--resume", "mock-session-123"]
        });
        assert!(
            has_triple,
            "shared default after crash must emit `--fork-session --resume <first-sid>`; got {argv:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Isolated: `agent_session: isolated` opts out entirely, even when a
// first-wins session_id is available in the log. argv must contain no
// `--resume` and no `--fork-session`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_isolated_session_emits_no_session_args() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();
        let argv_plan = tmp.path().join("argv_plan.txt");
        let argv_solo = tmp.path().join("argv_solo.txt");

        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("session");

        let plan = agent_step_with_argv_capture("plan", "Plan", &argv_plan);
        let mut solo = agent_step_with_argv_capture("solo", "Alone", &argv_solo);
        solo.agent_session = Some(stepyard_harness::AgentSessionMode::Isolated);

        let wf = Workflow::new("agent-isolated", vec![plan, solo]);

        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf,
            lifecycle(),
            unreachable_executor(),
        );
        assert_eq!(
            engine.resume().await.expect("resume"),
            StepOutcome::WorkflowCompleted
        );

        let argv = read_argv_file(&argv_solo);
        assert!(
            !argv.iter().any(|a| a == "--resume"),
            "isolated session must NOT emit --resume; got {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "--fork-session"),
            "isolated session must NOT emit --fork-session; got {argv:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// Replay skips a completed agent step: after phase 1 marks the step
// completed in the log, phase 2 must NOT re-spawn the CLI — we prove this
// by pointing `agent_command` at a binary that was valid in phase 1 but
// gets deleted between phases. A re-entry would surface as a Spawn
// AgentExecError (ENOENT); a correct skip reaches WorkflowCompleted
// cleanly. Mirrors `tests/template_replay.rs` replay pattern.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_replay_skips_completed_step() {
    db_test!(pool, {
        let tmp = tempfile::tempdir().unwrap();

        // Copy the repo-tracked fixture into the tempdir so we can delete
        // it between phases without affecting parallel tests.
        let live_fixture = tmp.path().join("mock_claude.sh");
        std::fs::copy(fixture_path(), &live_fixture).expect("copy fixture");
        // `fs::copy` does NOT preserve the +x bit on every platform, so
        // set it explicitly — otherwise phase 1 fails with ENOEXEC.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&live_fixture).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&live_fixture, perms).unwrap();
        }

        let tenant = Uuid::new_v4();
        let session = Session::new(&pool, tenant, "edenred".into())
            .await
            .expect("session");
        let session_id = session.id();

        let mut step = Step::agent("once", "Hi");
        step.agent_command = Some(live_fixture.to_string_lossy().into_owned());
        let wf = Workflow::new("agent-replay-skip", vec![step]);

        // Phase 1: run the step, drop the engine.
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            wf.clone(),
            lifecycle(),
            unreachable_executor(),
        );
        let first = engine.step().await.expect("step 1");
        assert!(
            matches!(first, StepOutcome::StepCompleted { ref step_name } if step_name == "once"),
            "got {first:?}"
        );
        drop(engine);

        // Delete the fixture — if replay re-enters the agent executor,
        // Spawn fails with ENOENT and the test surfaces the regression.
        std::fs::remove_file(&live_fixture).unwrap();

        // Phase 2: reload. progress_from_log sees the completed event
        // and advances past it; resume() reaches WorkflowCompleted
        // without touching the now-missing fixture.
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
