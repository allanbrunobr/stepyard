//! Shared session-setup helpers used by both the v1 (`Engine::run`) and v2
//! (`stepyard_harness::Engine::resume`) execute paths.
//!
//! Split into three layered entry points:
//!
//! * [`connect_pg`] — read `DATABASE_URL`, open a pool, run session
//!   migrations. Fresh callers (Story 2.4 startup reconcile) use this to
//!   obtain a pool they can share with later stages.
//! * [`open_session_with_pool`] — given an already-connected pool, insert a
//!   new `sessions` row for this workflow dispatch and return the handle.
//! * [`open_session`] — back-compat compound wrapper that chains the two
//!   above for legacy callers that do not need to interleave work between
//!   pool setup and session creation.
//!
//! Centralising `DATABASE_URL` reading, pool construction, migrations, and
//! session creation keeps the Story 1.4 error-message pins from drifting
//! across callers.

use std::time::Duration;

use anyhow::Context;

/// Connect to PostgreSQL and run session migrations.
///
/// Returns a clear error (not `anyhow!`) when `DATABASE_URL` is missing or
/// the database is unreachable — this fulfils Story 1.4 AC:
/// "DATABASE_URL pointing to a PG that is down -> exit != 0 with 'engine
/// requires PostgreSQL backend'."
pub async fn connect_pg(json_mode: bool) -> anyhow::Result<sqlx::PgPool> {
    let db_url = std::env::var("DATABASE_URL").map_err(|_| {
        let msg = "engine requires PostgreSQL backend: DATABASE_URL env var is not set";
        if json_mode {
            let json = serde_json::json!({"error": msg, "type": "ConfigError"});
            println!("{}", serde_json::to_string_pretty(&json).unwrap_or_default());
        } else {
            eprintln!("{msg}");
            eprintln!("Hint: export DATABASE_URL=postgres://user:password@host:port/database");
        }
        anyhow::anyhow!("DATABASE_URL not set")
    })?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&db_url)
        .await
        .map_err(|e| {
            let msg = format!("engine requires PostgreSQL backend: cannot reach database: {e}");
            if json_mode {
                let json = serde_json::json!({"error": msg, "type": "DatabaseUnreachable"});
                println!("{}", serde_json::to_string_pretty(&json).unwrap_or_default());
            } else {
                eprintln!("{msg}");
            }
            anyhow::anyhow!("DATABASE_URL unreachable: {e}")
        })?;

    stepyard_session::migrate(&pool)
        .await
        .with_context(|| "engine requires PostgreSQL backend: migrations failed")?;

    Ok(pool)
}

/// Insert a new `sessions` row on an already-connected pool.
///
/// Splits out the session-creation step so callers can run work (such as
/// [`crate::startup::reconcile`]) between pool setup and session creation.
pub async fn open_session_with_pool(
    pool: &sqlx::PgPool,
    workflow_name: &str,
) -> anyhow::Result<stepyard_session::Session> {
    // Workflow identifier — stable UUID derived from the workflow name so that
    // the same workflow name always maps to the same workflow_id row. A real
    // workflows table (Story 2.x) will replace this with an opaque lookup.
    let workflow_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, workflow_name.as_bytes());
    let tenant_id = std::env::var("STEPYARD_TENANT").unwrap_or_else(|_| "default".to_string());

    stepyard_session::Session::new(pool, workflow_id, tenant_id)
        .await
        .with_context(|| "failed to create session row")
}

/// Back-compat compound helper: [`connect_pg`] + [`open_session_with_pool`].
pub async fn open_session(
    workflow_name: &str,
    json_mode: bool,
) -> anyhow::Result<stepyard_session::Session> {
    let pool = connect_pg(json_mode).await?;
    open_session_with_pool(&pool, workflow_name).await
}
