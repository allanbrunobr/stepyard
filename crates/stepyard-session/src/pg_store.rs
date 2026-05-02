use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::session::SessionStatus;
use crate::store::{SessionError, SessionEvent, SessionId};
use crate::store_trait::{EventStore, SessionMeta};

#[derive(Debug, Clone)]
pub struct PgEventStore {
    pool: PgPool,
}

impl PgEventStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

type SessionRow = (
    Uuid,
    Uuid,
    String,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);
type EventRow = (Uuid, Uuid, i64, DateTime<Utc>, serde_json::Value);

fn meta_from_row(row: SessionRow) -> Result<SessionMeta, SessionError> {
    let (id, workflow_id, tenant_id, status, started_at, ended_at) = row;
    Ok(SessionMeta {
        id: SessionId(id),
        workflow_id,
        tenant_id,
        status: SessionStatus::from_db(&status)?,
        started_at,
        ended_at,
    })
}

fn event_from_row(row: EventRow) -> SessionEvent {
    SessionEvent {
        id: row.0,
        session_id: SessionId(row.1),
        seq: row.2,
        created_at: row.3,
        payload: row.4,
    }
}

#[async_trait]
impl EventStore for PgEventStore {
    async fn create_session(
        &self,
        workflow_id: Uuid,
        tenant_id: String,
    ) -> Result<SessionMeta, SessionError> {
        let id = SessionId::new();
        let row: SessionRow = sqlx::query_as(
            r#"
            INSERT INTO sessions (id, workflow_id, tenant_id, status, started_at)
            VALUES ($1, $2, $3, 'running', NOW())
            RETURNING id, workflow_id, tenant_id, status, started_at, ended_at
            "#,
        )
        .bind(id.as_uuid())
        .bind(workflow_id)
        .bind(&tenant_id)
        .fetch_one(&self.pool)
        .await?;

        meta_from_row(row)
    }

    async fn load_session_meta(&self, id: SessionId) -> Result<SessionMeta, SessionError> {
        let row: Option<SessionRow> = sqlx::query_as(
            r#"
            SELECT id, workflow_id, tenant_id, status, started_at, ended_at
            FROM sessions
            WHERE id = $1
            "#,
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;

        meta_from_row(row.ok_or(SessionError::NotFound(id))?)
    }

    async fn append(
        &self,
        session_id: SessionId,
        payload: serde_json::Value,
    ) -> Result<SessionEvent, SessionError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
            .bind(session_id.as_uuid().to_string())
            .execute(&mut *tx)
            .await?;

        let row: EventRow = sqlx::query_as(
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
        .bind(session_id.as_uuid())
        .bind(&payload)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(event_from_row(row))
    }

    async fn replay(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError> {
        let rows: Vec<EventRow> = sqlx::query_as(
            r#"
            SELECT id, session_id, seq, created_at, payload
            FROM session_events
            WHERE session_id = $1
            ORDER BY seq ASC
            "#,
        )
        .bind(session_id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(event_from_row).collect())
    }

    async fn finish_session(
        &self,
        session_id: SessionId,
        status: SessionStatus,
    ) -> Result<Option<(SessionStatus, Option<DateTime<Utc>>)>, SessionError> {
        let row: Option<(String, Option<DateTime<Utc>>)> = sqlx::query_as(
            r#"
            UPDATE sessions
            SET status = $2, ended_at = NOW()
            WHERE id = $1 AND status = 'running'
            RETURNING status, ended_at
            "#,
        )
        .bind(session_id.as_uuid())
        .bind(status.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(db_status, ended_at)| Ok((SessionStatus::from_db(&db_status)?, ended_at)))
            .transpose()
    }
}
