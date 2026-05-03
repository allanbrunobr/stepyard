use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::session::SessionStatus;
use crate::store::{SessionError, SessionEvent, SessionId};
use crate::store_trait::{EventStore, SessionMeta};

#[derive(Debug, Clone)]
pub struct SqliteEventStore {
    pool: SqlitePool,
}

impl SqliteEventStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::Session;

    async fn store() -> SqliteEventStore {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        crate::migrate_sqlite(&pool).await.expect("migrate sqlite");
        SqliteEventStore::new(pool)
    }

    #[tokio::test]
    async fn session_round_trips_events_and_terminal_status() {
        let store = Arc::new(store().await);
        let mut session = Session::new_with_store(store.clone(), Uuid::new_v4(), "lite".into())
            .await
            .expect("create session");

        let first = session
            .append(json!({"event": "first"}))
            .await
            .expect("append first");
        let second = session
            .append(json!({"event": "second"}))
            .await
            .expect("append second");

        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);

        let reloaded = Session::load_with_store(store.clone(), session.id())
            .await
            .expect("reload session");
        let events = reloaded.replay().await.expect("replay events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].payload, json!({"event": "first"}));
        assert_eq!(events[1].payload, json!({"event": "second"}));

        session.complete().await.expect("complete session");
        assert_eq!(session.status(), SessionStatus::Completed);
        assert!(session.ended_at().is_some());
    }
}

type SqliteSessionRow = (
    String,
    String,
    String,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);
type SqliteEventRow = (String, String, i64, DateTime<Utc>, String);

fn parse_uuid(raw: String) -> Result<Uuid, SessionError> {
    Uuid::parse_str(&raw)
        .map_err(|e| SessionError::InvalidState(format!("invalid uuid `{raw}`: {e}")))
}

fn meta_from_row(row: SqliteSessionRow) -> Result<SessionMeta, SessionError> {
    let (id, workflow_id, tenant_id, status, started_at, ended_at) = row;
    Ok(SessionMeta {
        id: SessionId(parse_uuid(id)?),
        workflow_id: parse_uuid(workflow_id)?,
        tenant_id,
        status: SessionStatus::from_db(&status)?,
        started_at,
        ended_at,
    })
}

fn event_from_row(row: SqliteEventRow) -> Result<SessionEvent, SessionError> {
    Ok(SessionEvent {
        id: parse_uuid(row.0)?,
        session_id: SessionId(parse_uuid(row.1)?),
        seq: row.2,
        created_at: row.3,
        payload: serde_json::from_str(&row.4)?,
    })
}

#[async_trait]
impl EventStore for SqliteEventStore {
    async fn create_session(
        &self,
        workflow_id: Uuid,
        tenant_id: String,
    ) -> Result<SessionMeta, SessionError> {
        let id = SessionId::new();
        let row: SqliteSessionRow = sqlx::query_as(
            r#"
            INSERT INTO sessions (id, workflow_id, tenant_id, status, started_at)
            VALUES (?1, ?2, ?3, 'running', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            RETURNING id, workflow_id, tenant_id, status, started_at, ended_at
            "#,
        )
        .bind(id.as_uuid().to_string())
        .bind(workflow_id.to_string())
        .bind(&tenant_id)
        .fetch_one(&self.pool)
        .await?;

        meta_from_row(row)
    }

    async fn load_session_meta(&self, id: SessionId) -> Result<SessionMeta, SessionError> {
        let row: Option<SqliteSessionRow> = sqlx::query_as(
            r#"
            SELECT id, workflow_id, tenant_id, status, started_at, ended_at
            FROM sessions
            WHERE id = ?1
            "#,
        )
        .bind(id.as_uuid().to_string())
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
        let payload = serde_json::to_string(&payload)?;
        let event_id = Uuid::new_v4();

        let row: SqliteEventRow = sqlx::query_as(
            r#"
            INSERT INTO session_events (id, session_id, seq, created_at, payload)
            VALUES (
                ?1,
                ?2,
                COALESCE((SELECT MAX(seq) FROM session_events WHERE session_id = ?2), 0) + 1,
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                ?3
            )
            RETURNING id, session_id, seq, created_at, payload
            "#,
        )
        .bind(event_id.to_string())
        .bind(session_id.as_uuid().to_string())
        .bind(payload)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;
        event_from_row(row)
    }

    async fn replay(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError> {
        let rows: Vec<SqliteEventRow> = sqlx::query_as(
            r#"
            SELECT id, session_id, seq, created_at, payload
            FROM session_events
            WHERE session_id = ?1
            ORDER BY seq ASC
            "#,
        )
        .bind(session_id.as_uuid().to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(event_from_row).collect()
    }

    async fn finish_session(
        &self,
        session_id: SessionId,
        status: SessionStatus,
    ) -> Result<Option<(SessionStatus, Option<DateTime<Utc>>)>, SessionError> {
        let row: Option<(String, Option<DateTime<Utc>>)> = sqlx::query_as(
            r#"
            UPDATE sessions
            SET status = ?2, ended_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            WHERE id = ?1 AND status = 'running'
            RETURNING status, ended_at
            "#,
        )
        .bind(session_id.as_uuid().to_string())
        .bind(status.as_str())
        .fetch_optional(&self.pool)
        .await?;

        row.map(|(db_status, ended_at)| Ok((SessionStatus::from_db(&db_status)?, ended_at)))
            .transpose()
    }
}
