use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::session::SessionStatus;
use crate::store::{SessionError, SessionEvent, SessionId};

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: SessionId,
    pub workflow_id: Uuid,
    pub tenant_id: String,
    pub status: SessionStatus,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait EventStore: Send + Sync + 'static {
    async fn create_session(
        &self,
        workflow_id: Uuid,
        tenant_id: String,
    ) -> Result<SessionMeta, SessionError>;

    async fn load_session_meta(&self, id: SessionId) -> Result<SessionMeta, SessionError>;

    async fn append(
        &self,
        session_id: SessionId,
        payload: serde_json::Value,
    ) -> Result<SessionEvent, SessionError>;

    async fn replay(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError>;

    async fn finish_session(
        &self,
        session_id: SessionId,
        status: SessionStatus,
    ) -> Result<Option<(SessionStatus, Option<DateTime<Utc>>)>, SessionError>;
}

pub type DynEventStore = Arc<dyn EventStore>;
