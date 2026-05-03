//! Append-only session log primitive for the Stepyard Engine v2 harness.
//!
//! A [`Session`] is the central primitive of the engine's v2 architecture
//! (phase A of the refactor). Every piece of context the harness reconstructs
//! between `step`s comes from replaying the session event log.
//!
//! # Invariants
//!
//! 1. **Append-only.** Once a [`SessionEvent`] is written, it is never edited
//!    or deleted. Truncating means starting a new [`SessionId`].
//! 2. **Monotonic `seq`.** Events within a session have strictly increasing
//!    `seq` values starting at 1, with no gaps.
//! 3. **Replay is deterministic.** `Session::load(id).replay()` always returns
//!    events in `seq` order, independent of `created_at` clock skew.
//!
//! # Postgres quick start
//!
//! ```ignore
//! use stepyard_session::{Session, SessionId};
//! use serde_json::json;
//! use sqlx::postgres::PgPoolOptions;
//! use uuid::Uuid;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = PgPoolOptions::new()
//!     .max_connections(5)
//!     .connect("postgres://localhost/stepyard").await?;
//!
//! // Create a new session for a dispatched workflow
//! let session = Session::new(&pool, Uuid::new_v4(), "edenred".to_string()).await?;
//!
//! // Append events (monotonic seq, append-only)
//! session.append(json!({"type": "step_started", "step": "review"})).await?;
//! session.append(json!({"type": "step_completed", "step": "review"})).await?;
//!
//! // Replay in deterministic order
//! let events = session.replay().await?;
//! assert_eq!(events.len(), 2);
//! assert_eq!(events[0].seq, 1);
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture reference
//!
//! See `minion-engine/ARCHITECTURE.md` sections "minion-session" and
//! "Data Model" (Invariante 2, Invariante 11). Schema is in
//! `crates/minion-session/migrations/`.

#[cfg(all(feature = "postgres", feature = "sqlite"))]
compile_error!("stepyard-session: features `postgres` and `sqlite` are mutually exclusive");

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
compile_error!("stepyard-session: enable exactly one backend feature: `postgres` or `sqlite`");

mod factory;
pub mod file_log_mirror;
#[cfg(feature = "postgres")]
mod pg_store;
mod session;
#[cfg(feature = "sqlite")]
mod sqlite_store;
mod store;
mod store_trait;

pub use factory::{build_store_from_env, build_store_with_file_logs, StoreConfig};
pub use file_log_mirror::{FileLogConfig, FileLogMirror};
#[cfg(feature = "postgres")]
pub use pg_store::PgEventStore;
pub use session::{Session, SessionStatus};
#[cfg(feature = "sqlite")]
pub use sqlite_store::SqliteEventStore;
pub use store::{SessionError, SessionEvent, SessionId};
pub use store_trait::{DynEventStore, EventStore, SessionMeta};

/// Runs the embedded SQL migrations against the given pool.
///
/// Call this once at application startup before any [`Session`] methods are
/// invoked. Idempotent — already-applied migrations are skipped by sqlx.
///
/// # Errors
/// Returns [`SessionError::Database`] if the migration runner fails.
#[cfg(feature = "postgres")]
pub async fn migrate(pool: &sqlx::PgPool) -> Result<(), SessionError> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| SessionError::Database(sqlx::Error::Migrate(Box::new(e))))?;
    Ok(())
}

#[cfg(feature = "sqlite")]
pub async fn migrate_sqlite(pool: &sqlx::SqlitePool) -> Result<(), SessionError> {
    sqlx::migrate!("./migrations-sqlite")
        .run(pool)
        .await
        .map_err(|e| SessionError::Database(sqlx::Error::Migrate(Box::new(e))))?;
    Ok(())
}
