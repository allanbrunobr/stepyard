use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use crate::session::SessionStatus;
use crate::store::{SessionError, SessionEvent, SessionId};
use crate::store_trait::{EventStore, SessionMeta};

#[derive(Debug, Clone)]
pub struct FileLogConfig {
    pub enabled: bool,
    pub directory: PathBuf,
}

impl Default for FileLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: PathBuf::from(".stepyard").join("logs"),
        }
    }
}

pub struct FileLogMirror {
    inner: Arc<dyn EventStore>,
    directory: PathBuf,
    broken_sessions: Mutex<HashSet<SessionId>>,
}

impl FileLogMirror {
    pub fn new(inner: Arc<dyn EventStore>, directory: PathBuf) -> Self {
        Self {
            inner,
            directory,
            broken_sessions: Mutex::new(HashSet::new()),
        }
    }

    fn path_for(&self, session_id: SessionId) -> PathBuf {
        self.directory.join(format!("{session_id}.jsonl"))
    }

    async fn mirror(&self, event: &SessionEvent) {
        if self
            .broken_sessions
            .lock()
            .await
            .contains(&event.session_id)
        {
            return;
        }

        let write_result = self.write_jsonl(event).await;
        if let Err(error_class) = write_result {
            self.broken_sessions.lock().await.insert(event.session_id);
            let payload = serde_json::to_value(stepyard_core::Event::FileLogWriteFailed {
                error_class,
                timestamp: Utc::now(),
            })
            .expect("FileLogWriteFailed serializes");
            let _ = self.inner.append(event.session_id, payload).await;
        }
    }

    async fn write_jsonl(&self, event: &SessionEvent) -> Result<(), String> {
        tokio::fs::create_dir_all(&self.directory)
            .await
            .map_err(classify_io_error)?;

        let path = self.path_for(event.session_id);
        let mut line = serde_json::to_vec(event).map_err(|_| "json_encode".to_string())?;
        line.push(b'\n');

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(classify_io_error)?;
        file.write_all(&line).await.map_err(classify_io_error)?;
        Ok(())
    }
}

fn classify_io_error(error: std::io::Error) -> String {
    if error.raw_os_error() == Some(28) {
        return "disk_full".to_string();
    }
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => "permission_denied".to_string(),
        _ => "io_other".to_string(),
    }
}

#[async_trait]
impl EventStore for FileLogMirror {
    async fn create_session(
        &self,
        workflow_id: uuid::Uuid,
        tenant_id: String,
    ) -> Result<SessionMeta, SessionError> {
        self.inner.create_session(workflow_id, tenant_id).await
    }

    async fn load_session_meta(&self, id: SessionId) -> Result<SessionMeta, SessionError> {
        self.inner.load_session_meta(id).await
    }

    async fn append(
        &self,
        session_id: SessionId,
        payload: serde_json::Value,
    ) -> Result<SessionEvent, SessionError> {
        let event = self.inner.append(session_id, payload).await?;
        self.mirror(&event).await;
        Ok(event)
    }

    async fn replay(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError> {
        self.inner.replay(session_id).await
    }

    async fn finish_session(
        &self,
        session_id: SessionId,
        status: SessionStatus,
    ) -> Result<Option<(SessionStatus, Option<DateTime<Utc>>)>, SessionError> {
        self.inner.finish_session(session_id, status).await
    }
}
