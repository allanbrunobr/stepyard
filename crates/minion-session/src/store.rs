//! Storage-facing types for the session log: [`SessionId`], [`SessionEvent`],
//! and the domain [`SessionError`].
//!
//! These types are intentionally schema-agnostic about the event payload —
//! payloads are stored as opaque JSON so that `minion-core`'s `Event` enum can
//! evolve independently of the session storage layer.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Newtype wrapper around [`Uuid`] identifying a [`Session`](crate::Session).
///
/// Implements [`Display`] and [`FromStr`] so it can be serialized to/from
/// human-readable forms (JSON strings, CLI args, URL paths).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// Generate a new random v4 [`SessionId`].
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Return the inner [`Uuid`].
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for SessionId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for SessionId {
    fn from(value: Uuid) -> Self {
        Self(value)
    }
}

impl From<SessionId> for Uuid {
    fn from(value: SessionId) -> Uuid {
        value.0
    }
}

/// One persisted entry in a session's append-only log.
///
/// `payload` is opaque JSON so that the event schema can evolve in
/// `minion-core` without forcing a schema migration of the session storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Row identifier (random v4 UUID).
    pub id: Uuid,
    /// The session this event belongs to.
    pub session_id: SessionId,
    /// Monotonic sequence number within `session_id`. Starts at 1, no gaps.
    pub seq: i64,
    /// Wall-clock time the event row was inserted (server NOW()).
    pub created_at: DateTime<Utc>,
    /// Opaque JSON payload (shape owned by `minion-core::Event`).
    pub payload: serde_json::Value,
}

/// Domain errors returned by session APIs.
///
/// Callers receive a typed error and never have to inspect `sqlx::Error`
/// variants directly. Use `SessionError::NotFound` to distinguish missing
/// sessions from general database failures.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The requested [`SessionId`] does not exist in the `sessions` table.
    #[error("session not found: {0}")]
    NotFound(SessionId),

    /// Underlying database failure (connection, sqlx, migration, etc).
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Invariant violation — should never happen in normal operation.
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// Failed to serialize or deserialize a JSON payload.
    #[error("payload encoding error: {0}")]
    Payload(#[from] serde_json::Error),
}
