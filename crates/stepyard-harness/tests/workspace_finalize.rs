//! Workspace finalization ordering tests.
//!
//! Requires a PostgreSQL reachable via `STEPYARD_HARNESS_DATABASE_URL`.
//! Without it, the tests skip cleanly so local/CI runs without a database
//! stay green.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{
    Engine, EngineError, HarnessConfig, Step, StepExecutor, StepOutcome, Workflow,
};
use stepyard_sandbox_orchestrator::{
    BranchStrategy, ExecOutput, FinalizeReport, MockLifecycle, SandboxError, SandboxLifecycle,
    WorkflowOutcome, Workspace, WorkspaceError, WorkspaceManager,
};
use stepyard_session::{migrate, Session, SessionEvent, SessionId};
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
struct FinalizeAssertingWorkspaceManager {
    pool: sqlx::PgPool,
    planned_path: PathBuf,
    conflict: bool,
}

#[async_trait]
impl WorkspaceManager for FinalizeAssertingWorkspaceManager {
    async fn prepare(
        &self,
        session_id: &SessionId,
        strategy: &BranchStrategy,
    ) -> Result<Workspace, WorkspaceError> {
        assert!(matches!(strategy, BranchStrategy::MergeToHead { target } if target == "main"));
        Ok(Workspace {
            path: self.planned_path.clone(),
            branch: Some(format!("stepyard/session-{}", session_id.as_uuid())),
            merge_target: Some("main".to_string()),
            session_id: *session_id,
        })
    }

    async fn finalize(
        &self,
        workspace: &Workspace,
        outcome: WorkflowOutcome,
    ) -> Result<FinalizeReport, WorkspaceError> {
        assert_eq!(outcome, WorkflowOutcome::Success);
        let session = Session::load(&self.pool, workspace.session_id)
            .await
            .expect("session exists during finalize");
        let events = session.replay().await.expect("replay before finalize");
        let names = event_names(&events);

        assert_eq!(
            names,
            vec![
                "workflow_started",
                "workspace_prepared",
                "branch_created",
                "step_started",
                "step_completed",
                "merge_attempted"
            ],
            "merge_attempted must be persisted before merge IO"
        );
        assert_eq!(
            events[5].payload["source"],
            workspace.branch.as_deref().unwrap()
        );
        assert_eq!(events[5].payload["target"], "main");

        if self.conflict {
            return Err(WorkspaceError::MergeConflict {
                files: vec!["README.md".to_string(), "src/lib.rs".to_string()],
            });
        }

        Ok(FinalizeReport::new(Utc::now(), true, Vec::new()))
    }

    async fn prune(&self) -> Result<stepyard_sandbox_orchestrator::PruneReport, WorkspaceError> {
        unimplemented!("not used by finalize test")
    }
}

fn event_names(events: &[SessionEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| event.payload.get("event").and_then(|value| value.as_str()))
        .collect()
}

fn configured_engine(
    pool: sqlx::PgPool,
    session: Session,
    planned_path: PathBuf,
    conflict: bool,
) -> Engine {
    let manager = Arc::new(FinalizeAssertingWorkspaceManager {
        pool,
        planned_path: planned_path.clone(),
        conflict,
    });
    let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());
    let executor = Arc::new(OkExecutor);

    Engine::with_executor(
        HarnessConfig::default(),
        session,
        Workflow::new("workspace", vec![Step::cmd("one", "echo ok")]),
        lifecycle,
        executor,
    )
    .with_workspace_manager(
        manager,
        BranchStrategy::MergeToHead {
            target: "main".to_string(),
        },
        planned_path,
    )
}

#[tokio::test]
async fn engine_emits_merge_attempted_before_finalize_io() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "workspace-finalize".into())
            .await
            .expect("new session");
        let mut engine = configured_engine(
            pool.clone(),
            session,
            PathBuf::from("/tmp/stepyard-test-workspace"),
            false,
        );

        let first = engine.step().await.expect("step succeeds");
        assert!(matches!(first, StepOutcome::StepCompleted { .. }));
        let second = engine.step().await.expect("workflow finalizes");
        assert_eq!(second, StepOutcome::WorkflowCompleted);

        let events = engine.session().replay().await.expect("replay");
        assert_eq!(
            event_names(&events),
            vec![
                "workflow_started",
                "workspace_prepared",
                "branch_created",
                "step_started",
                "step_completed",
                "merge_attempted",
                "workflow_completed"
            ]
        );
    });
}

#[tokio::test]
async fn engine_emits_merge_conflict_before_returning_finalize_error() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "workspace-conflict".into())
            .await
            .expect("new session");
        let mut engine = configured_engine(
            pool.clone(),
            session,
            PathBuf::from("/tmp/stepyard-test-workspace"),
            true,
        );

        let first = engine.step().await.expect("step succeeds");
        assert!(matches!(first, StepOutcome::StepCompleted { .. }));
        let err = engine
            .step()
            .await
            .expect_err("conflict fails finalization");
        assert!(matches!(
            err,
            EngineError::Workspace(WorkspaceError::MergeConflict { ref files })
                if files == &vec!["README.md".to_string(), "src/lib.rs".to_string()]
        ));

        let events = engine.session().replay().await.expect("replay");
        assert_eq!(
            event_names(&events),
            vec![
                "workflow_started",
                "workspace_prepared",
                "branch_created",
                "step_started",
                "step_completed",
                "merge_attempted",
                "merge_conflict"
            ]
        );
        assert_eq!(events[6].payload["source"], events[5].payload["source"]);
        assert_eq!(events[6].payload["target"], "main");
        assert_eq!(
            events[6].payload["files"],
            serde_json::json!(["README.md", "src/lib.rs"])
        );
    });
}
