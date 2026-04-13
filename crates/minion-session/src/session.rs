//! The [`Session`] handle — the public entry point for the append-only log.
//!
//! A `Session` is cheaply cloneable (`Clone + Send + Sync`) because internally
//! it holds a [`sqlx::PgPool`] and a few UUIDs. Cloning does not open a new
//! connection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::store::{SessionError, SessionEvent, SessionId};

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

    fn from_db(s: &str) -> Result<Self, SessionError> {
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
    pool: PgPool,
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
    pub async fn new(
        pool: &PgPool,
        workflow_id: Uuid,
        tenant_id: String,
    ) -> Result<Self, SessionError> {
        let id = SessionId::new();
        let row: (Uuid, String, chrono::DateTime<Utc>) = sqlx::query_as(
            r#"
            INSERT INTO sessions (id, workflow_id, tenant_id, status, started_at)
            VALUES ($1, $2, $3, 'running', NOW())
            RETURNING id, status, started_at
            "#,
        )
        .bind(id.as_uuid())
        .bind(workflow_id)
        .bind(&tenant_id)
        .fetch_one(pool)
        .await?;

        Ok(Self {
            id: SessionId(row.0),
            workflow_id,
            tenant_id,
            status: SessionStatus::from_db(&row.1)?,
            started_at: row.2,
            ended_at: None,
            pool: pool.clone(),
        })
    }

    /// Load an existing session by its [`SessionId`].
    ///
    /// # Errors
    /// - [`SessionError::NotFound`] if no row matches.
    /// - [`SessionError::Database`] on SQL failure.
    pub async fn load(pool: &PgPool, id: SessionId) -> Result<Self, SessionError> {
        let row: Option<(Uuid, Uuid, String, String, DateTime<Utc>, Option<DateTime<Utc>>)> =
            sqlx::query_as(
                r#"
                SELECT id, workflow_id, tenant_id, status, started_at, ended_at
                FROM sessions
                WHERE id = $1
                "#,
            )
            .bind(id.as_uuid())
            .fetch_optional(pool)
            .await?;

        let (id_db, workflow_id, tenant_id, status, started_at, ended_at) =
            row.ok_or(SessionError::NotFound(id))?;

        Ok(Self {
            id: SessionId(id_db),
            workflow_id,
            tenant_id,
            status: SessionStatus::from_db(&status)?,
            started_at,
            ended_at,
            pool: pool.clone(),
        })
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
    pub async fn append(
        &self,
        payload: serde_json::Value,
    ) -> Result<SessionEvent, SessionError> {
        let mut tx = self.pool.begin().await?;

        // Serialize appends per session so `seq` stays monotonic without gaps.
        // The lock key must fit in a BIGINT; `hashtextextended` returns i64.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
            .bind(self.id.as_uuid().to_string())
            .execute(&mut *tx)
            .await?;

        let row: (Uuid, Uuid, i64, DateTime<Utc>, serde_json::Value) = sqlx::query_as(
            r#"
            INSERT INTO session_events (id, session_id, seq, created_at, payload)
            VALUES (
                gen_random_uuid(),
                $1,
                COALESCE((SELECT MAX(seq) FROM session_events WHERE session_id = $1), 0) + 1,
                NOW(),
                $2
            )
            RETURNING id, session_id, seq, created_at, payload
            "#,
        )
        .bind(self.id.as_uuid())
        .bind(&payload)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(SessionEvent {
            id: row.0,
            session_id: SessionId(row.1),
            seq: row.2,
            created_at: row.3,
            payload: row.4,
        })
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
        let rows: Vec<(Uuid, Uuid, i64, DateTime<Utc>, serde_json::Value)> = sqlx::query_as(
            r#"
            SELECT id, session_id, seq, created_at, payload
            FROM session_events
            WHERE session_id = $1
            ORDER BY seq ASC
            "#,
        )
        .bind(self.id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(id, session_id, seq, created_at, payload)| SessionEvent {
                id,
                session_id: SessionId(session_id),
                seq,
                created_at,
                payload,
            })
            .collect())
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
        let row: Option<(String, Option<DateTime<Utc>>)> = sqlx::query_as(
            r#"
            UPDATE sessions
            SET status = $2, ended_at = NOW()
            WHERE id = $1 AND status = 'running'
            RETURNING status, ended_at
            "#,
        )
        .bind(self.id.as_uuid())
        .bind(status.as_str())
        .fetch_optional(&self.pool)
        .await?;

        if let Some((db_status, ended_at)) = row {
            self.status = SessionStatus::from_db(&db_status)?;
            self.ended_at = ended_at;
        }
        // If row is None, session already terminal (or missing). Leave
        // local state untouched — callers can inspect `status()` to confirm.
        Ok(())
    }
}
