//! Story 1.1 — `Engine::finalise_cancel` must tear down the sandbox bound to
//! the session's own UUID, not a freshly generated random `SandboxId`.
//!
//! Before this fix the engine called `lifecycle.destroy(&SandboxId::default())`,
//! which generates a fresh `Uuid::new_v4()` every invocation. A real backend
//! has no way to map that random id back to the container for the session, so
//! the container leaked for the rest of the session's lifetime.
//!
//! Skipped gracefully if `STEPYARD_HARNESS_DATABASE_URL` is unset.

use std::sync::Arc;

use sqlx::postgres::PgPoolOptions;
use stepyard_harness::{Engine, HarnessConfig, Step, StepOutcome, Workflow};
use stepyard_sandbox_orchestrator::{MockCall, MockLifecycle, SandboxLifecycle};
use stepyard_session::{migrate, Session};
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

#[tokio::test]
async fn cancel_calls_destroy_with_session_uuid() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };

    let tenant = format!("cancel-cleanup-{}", Uuid::new_v4());
    let workflow = Workflow::new(
        "cancel-cleanup".to_string(),
        vec![Step::cmd("slow".to_string(), "sleep 60".to_string())],
    );

    let mock: Arc<MockLifecycle> = Arc::new(MockLifecycle::new());
    let session = Session::new(&pool, Uuid::new_v4(), tenant)
        .await
        .expect("new session");
    let session_uuid = *session.id().as_uuid();

    let lifecycle: Arc<dyn SandboxLifecycle> = mock.clone();
    let mut engine = Engine::new(HarnessConfig::default(), session, workflow, lifecycle);

    // Flip cancel before any step runs so `finalise_cancel` is the first
    // branch hit inside `step()`. No container is ever created — the only
    // lifecycle call we expect is the destroy that carries the session UUID.
    engine.cancel_token().cancel();

    let outcome = engine.step().await.expect("step returns");
    assert_eq!(outcome, StepOutcome::Cancelled);

    let calls = mock.calls().await;
    let destroys: Vec<&MockCall> = calls
        .iter()
        .filter(|c| matches!(c, MockCall::Destroy { .. }))
        .collect();
    assert_eq!(
        destroys.len(),
        1,
        "finalise_cancel must call destroy exactly once, got {calls:?}"
    );
    let MockCall::Destroy { id } = destroys[0] else {
        unreachable!("filtered for Destroy above")
    };
    assert_eq!(
        *id.as_uuid(),
        session_uuid,
        "destroy must receive the session's UUID, not a random one"
    );
}
