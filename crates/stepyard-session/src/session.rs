//! The [`Session`] handle — the public entry point for the append-only log.
//!
//! A `Session` is cheaply cloneable (`Clone + Send + Sync`) because internally
//! it holds an [`EventStore`](crate::EventStore) handle and a few UUIDs.
//! Cloning does not open a new connection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "postgres")]
use sqlx::PgPool;
#[cfg(feature = "postgres")]
use std::sync::Arc;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::pg_store::PgEventStore;
use crate::store::{SessionError, SessionEvent, SessionId};
use crate::store_trait::{DynEventStore, SessionMeta};

/// Lifecycle status of a session, matching the DB enum domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl SessionStatus {
    /// String label matching the DB check constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn from_db(s: &str) -> Result<Self, SessionError> {
        match s {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(SessionError::InvalidState(format!(
                "unknown session status `{other}`"
            ))),
        }
    }
}

/// Append-only session handle. Cheaply cloneable.
///
/// See crate-level docs for the invariants guaranteed by this type.
#[derive(Clone)]
pub struct Session {
    id: SessionId,
    workflow_id: Uuid,
    tenant_id: String,
    status: SessionStatus,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    store: DynEventStore,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("workflow_id", &self.workflow_id)
            .field("tenant_id", &self.tenant_id)
            .field("status", &self.status)
            .field("started_at", &self.started_at)
            .field("ended_at", &self.ended_at)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// Create a new session row in the database with status `running`.
    ///
    /// # Errors
    /// Returns [`SessionError::Database`] on SQL failure.
    #[cfg(feature = "postgres")]
    pub async fn new(
        pool: &PgPool,
        workflow_id: Uuid,
        tenant_id: String,
    ) -> Result<Self, SessionError> {
        Self::new_with_store(
            Arc::new(PgEventStore::new(pool.clone())),
            workflow_id,
            tenant_id,
        )
        .await
    }

    pub async fn new_with_store(
        store: DynEventStore,
        workflow_id: Uuid,
        tenant_id: String,
    ) -> Result<Self, SessionError> {
        let meta = store.create_session(workflow_id, tenant_id).await?;
        Ok(Self::from_meta(store, meta))
    }

    /// Load an existing session by its [`SessionId`].
    ///
    /// # Errors
    /// - [`SessionError::NotFound`] if no row matches.
    /// - [`SessionError::Database`] on SQL failure.
    #[cfg(feature = "postgres")]
    pub async fn load(pool: &PgPool, id: SessionId) -> Result<Self, SessionError> {
        Self::load_with_store(Arc::new(PgEventStore::new(pool.clone())), id).await
    }

    pub async fn load_with_store(
        store: DynEventStore,
        id: SessionId,
    ) -> Result<Self, SessionError> {
        let meta = store.load_session_meta(id).await?;
        Ok(Self::from_meta(store, meta))
    }

    fn from_meta(store: DynEventStore, meta: SessionMeta) -> Self {
        Self {
            id: meta.id,
            workflow_id: meta.workflow_id,
            tenant_id: meta.tenant_id,
            status: meta.status,
            started_at: meta.started_at,
            ended_at: meta.ended_at,
            store,
        }
    }

    /// Append an event payload to the session log.
    ///
    /// The resulting [`SessionEvent`] has `seq = max(existing) + 1`. Under
    /// concurrent calls on the same session, appends are serialized by a
    /// per-session advisory lock (Postgres `pg_advisory_xact_lock`).
    ///
    /// # Errors
    /// - [`SessionError::Database`] on SQL failure (including unique-constraint
    ///   violation if the advisory lock is bypassed).
    /// - [`SessionError::Payload`] if `payload` is not valid JSON (cannot fail
    ///   for [`serde_json::Value`] input).
    pub async fn append(&self, payload: serde_json::Value) -> Result<SessionEvent, SessionError> {
        self.store.append(self.id, payload).await
    }

    /// Replay all events for this session in `seq` order.
    ///
    /// Returns an empty vector for a freshly created session. Ordering is by
    /// `seq ASC`, never by `created_at` — this guarantees determinism even
    /// when clock skew or retries produce out-of-order timestamps.
    ///
    /// # Errors
    /// [`SessionError::Database`] on SQL failure.
    pub async fn replay(&self) -> Result<Vec<SessionEvent>, SessionError> {
        self.store.replay(self.id).await
    }

    /// The [`SessionId`] of this session.
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// The workflow UUID this session was dispatched for.
    pub fn workflow_id(&self) -> Uuid {
        self.workflow_id
    }

    /// The tenant identifier (e.g. `"edenred"`, `"afya"`).
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    /// Current lifecycle status.
    pub fn status(&self) -> SessionStatus {
        self.status
    }

    /// When the session was created.
    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }

    /// When the session finished (`None` while running).
    pub fn ended_at(&self) -> Option<DateTime<Utc>> {
        self.ended_at
    }

    /// Mark the session as `completed`, setting `ended_at = NOW()`.
    ///
    /// This updates the `sessions` row only. Events remain append-only
    /// (NFC2 unaffected). Safe to call once; subsequent calls are no-ops
    /// because the status check stops repeated transitions.
    ///
    /// # Errors
    /// [`SessionError::Database`] on SQL failure.
    pub async fn complete(&mut self) -> Result<(), SessionError> {
        self.finish(SessionStatus::Completed).await
    }

    /// Mark the session as `failed`, setting `ended_at = NOW()`.
    ///
    /// # Errors
    /// [`SessionError::Database`] on SQL failure.
    pub async fn fail(&mut self) -> Result<(), SessionError> {
        self.finish(SessionStatus::Failed).await
    }

    /// Mark the session as `cancelled`, setting `ended_at = NOW()`.
    ///
    /// # Errors
    /// [`SessionError::Database`] on SQL failure.
    pub async fn cancel(&mut self) -> Result<(), SessionError> {
        self.finish(SessionStatus::Cancelled).await
    }

    async fn finish(&mut self, status: SessionStatus) -> Result<(), SessionError> {
        // Only transition from `running`; re-calling with the same terminal
        // state is a no-op so the engine can safely call complete/fail
        // idempotently on cleanup paths.
        if let Some((db_status, ended_at)) = self.store.finish_session(self.id, status).await? {
            self.status = db_status;
            self.ended_at = ended_at;
        }
        // If row is None, session already terminal (or missing). Leave
        // local state untouched — callers can inspect `status()` to confirm.
        Ok(())
    }
}
