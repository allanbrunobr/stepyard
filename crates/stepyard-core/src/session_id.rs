//! [`SessionId`]: the identifier of a session.
//!
//! Lives in the contract crate (no IO) so crates that only need the id — the
//! sandbox orchestrator names containers `stepyard-session-<uuid>` — do not
//! depend on the session store with its database drivers (ARQ-10).
//! `stepyard_session::SessionId` re-exports it.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Newtype wrapper around [`Uuid`] identifying a session (`stepyard_session::Session`).
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
