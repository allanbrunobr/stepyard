//! Workspace preparation ordering tests.
//!
//! Requires a PostgreSQL reachable via `STEPYARD_HARNESS_DATABASE_URL`.
//! Without it, the test skips cleanly so local/CI runs without a database
//! stay green.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{Engine, HarnessConfig, Step, StepExecutor, StepOutcome, Workflow};
use stepyard_sandbox_orchestrator::{
    BranchStrategy, ExecOutput, MockLifecycle, SandboxError, SandboxLifecycle, Workspace,
    WorkspaceError, WorkspaceManager,
};
use stepyard_session::{migrate, Session, SessionId};
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

#[derive(Default)]
struct OkExecutor;

#[async_trait]
impl StepExecutor for OkExecutor {
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
        Ok(ExecOutput {
            stdout: "ok\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
}

#[derive(Debug)]
struct AssertingWorkspaceManager {
    pool: sqlx::PgPool,
    planned_path: PathBuf,
    prepared: AtomicBool,
}

#[async_trait]
impl WorkspaceManager for AssertingWorkspaceManager {
    async fn prepare(
        &self,
        session_id: &SessionId,
        strategy: &BranchStrategy,
    ) -> Result<Workspace, WorkspaceError> {
        let session = Session::load(&self.pool, *session_id)
            .await
            .expect("session exists during prepare");
        let events = session.replay().await.expect("replay before prepare");
        let names: Vec<&str> = events
            .iter()
            .filter_map(|e| e.payload.get("event").and_then(|v| v.as_str()))
            .collect();

        assert_eq!(
            names,
            vec!["workflow_started", "workspace_prepared", "branch_created"],
            "workspace events must be persisted before prepare IO"
        );
        assert_eq!(
            events[1].payload["path"],
            self.planned_path.display().to_string()
        );
        assert_eq!(events[1].payload["strategy"], "named_branch");
        assert_eq!(events[2].payload["branch"], "feat/workspace");
        assert_eq!(events[2].payload["base"], "HEAD");
        assert!(
            matches!(strategy, BranchStrategy::NamedBranch { name } if name == "feat/workspace")
        );

        self.prepared.store(true, Ordering::SeqCst);
        Ok(Workspace {
            path: self.planned_path.clone(),
            branch: Some("feat/workspace".to_string()),
            session_id: *session_id,
        })
    }

    async fn finalize(
        &self,
        _workspace: &Workspace,
        _outcome: stepyard_sandbox_orchestrator::WorkflowOutcome,
    ) -> Result<stepyard_sandbox_orchestrator::FinalizeReport, WorkspaceError> {
        unimplemented!("not used by prepare test")
    }

    async fn prune(&self) -> Result<stepyard_sandbox_orchestrator::PruneReport, WorkspaceError> {
        unimplemented!("not used by prepare test")
    }
}

#[tokio::test]
async fn engine_emits_workspace_events_before_prepare_io() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "workspace-prepare".into())
            .await
            .expect("new session");
        let planned_path = PathBuf::from("/tmp/stepyard-test-workspace");
        let manager = Arc::new(AssertingWorkspaceManager {
            pool: pool.clone(),
            planned_path: planned_path.clone(),
            prepared: AtomicBool::new(false),
        });
        let prepared = manager.clone();
        let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
        let executor = Arc::new(OkExecutor);
        let mut engine = Engine::with_executor(
            HarnessConfig::default(),
            session,
            Workflow::new("workspace", vec![Step::cmd("one", "echo ok")]),
            lifecycle,
            executor,
        )
        .with_workspace_manager(
            manager,
            BranchStrategy::NamedBranch {
                name: "feat/workspace".to_string(),
            },
            planned_path,
        );

        let outcome = engine.step().await.expect("step succeeds");
        assert!(matches!(outcome, StepOutcome::StepCompleted { .. }));
        assert!(prepared.prepared.load(Ordering::SeqCst));

        let events = engine.session().replay().await.expect("replay");
        let names: Vec<&str> = events
            .iter()
            .filter_map(|e| e.payload.get("event").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                "workflow_started",
                "workspace_prepared",
                "branch_created",
                "step_started",
                "step_completed"
            ]
        );
    });
}
