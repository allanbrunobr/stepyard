//! Append-only session log primitive for the Minion Engine v2 harness.
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
//! # Quick start
//!
//! ```no_run
//! use minion_session::{Session, SessionId};
//! use serde_json::json;
//! use sqlx::postgres::PgPoolOptions;
//! use uuid::Uuid;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let pool = PgPoolOptions::new()
//!     .max_connections(5)
//!     .connect("postgres://localhost/minion").await?;
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

mod session;
mod store;

pub use session::{Session, SessionStatus};
pub use store::{SessionError, SessionEvent, SessionId};

/// Runs the embedded SQL migrations against the given pool.
///
/// Call this once at application startup before any [`Session`] methods are
/// invoked. Idempotent — already-applied migrations are skipped by sqlx.
///
/// # Errors
/// Returns [`SessionError::Database`] if the migration runner fails.
pub async fn migrate(pool: &sqlx::PgPool) -> Result<(), SessionError> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| SessionError::Database(sqlx::Error::Migrate(Box::new(e))))?;
    Ok(())
}
