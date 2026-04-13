//! Shared session-setup helper used by both the v1 (`Engine::run`) and v2
//! (`minion_harness::Engine::resume`) execute paths.
//!
//! Centralises `DATABASE_URL` reading, pool construction, migrations, and
//! session creation so the two code paths never drift on the error messages
//! that Story 1.4 pinned down.

use std::time::Duration;

use anyhow::Context;

/// Connect to PostgreSQL, run Session migrations, and open a Session for this
/// workflow dispatch.
///
/// Returns a clear error (not `anyhow!`) when DATABASE_URL is missing or the
/// database is unreachable — this fulfills Story 1.4 AC:
/// "DATABASE_URL pointing to a PG that is down -> exit != 0 with 'engine
/// requires PostgreSQL backend'."
pub async fn open_session(
    workflow_name: &str,
    json_mode: bool,
) -> anyhow::Result<minion_session::Session> {
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

    minion_session::migrate(&pool)
        .await
        .with_context(|| "engine requires PostgreSQL backend: migrations failed")?;

    // Workflow identifier — stable UUID derived from the workflow name so that
    // the same workflow name always maps to the same workflow_id row. A real
    // workflows table (Story 2.x) will replace this with an opaque lookup.
    let workflow_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, workflow_name.as_bytes());
    let tenant_id = std::env::var("MINION_TENANT").unwrap_or_else(|_| "default".to_string());

    minion_session::Session::new(&pool, workflow_id, tenant_id)
        .await
        .with_context(|| "failed to create session row")
}
