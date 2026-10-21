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
        file.flush().await.map_err(classify_io_error)?;
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

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use std::path::PathBuf;

    use serde_json::{json, Value};
    use sqlx::sqlite::SqlitePoolOptions;
    use uuid::Uuid;

    use super::*;
    use crate::{Session, SqliteEventStore};

    async fn sqlite_store() -> Arc<dyn EventStore> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        crate::migrate_sqlite(&pool).await.expect("migrate sqlite");
        Arc::new(SqliteEventStore::new(pool))
    }

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "stepyard-file-log-mirror-{label}-{}",
            Uuid::new_v4()
        ))
    }

    fn read_jsonl(path: PathBuf) -> Vec<SessionEvent> {
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read jsonl `{}`: {e}", path.display()));
        body.lines()
            .map(|line| {
                serde_json::from_str::<SessionEvent>(line)
                    .unwrap_or_else(|e| panic!("decode jsonl line `{line}`: {e}"))
            })
            .collect()
    }

    #[tokio::test]
    async fn append_mirrors_session_event_as_jsonl() {
        let inner = sqlite_store().await;
        let dir = temp_path("happy");
        let store: Arc<dyn EventStore> =
            Arc::new(FileLogMirror::new(Arc::clone(&inner), dir.clone()));
        let session = Session::new_with_store(store, Uuid::new_v4(), "lite".into())
            .await
            .expect("create session");

        let appended = session
            .append(json!({"event": "first"}))
            .await
            .expect("append");

        let mirrored = read_jsonl(dir.join(format!("{}.jsonl", session.id())));
        assert_eq!(mirrored.len(), 1);
        assert_eq!(mirrored[0].session_id, session.id());
        assert_eq!(mirrored[0].seq, appended.seq);
        assert_eq!(mirrored[0].payload, json!({"event": "first"}));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn write_failure_emits_one_degradation_event_and_disables_that_session() {
        let inner = sqlite_store().await;
        let blocker = temp_path("blocked");
        std::fs::write(&blocker, "not a directory").expect("write blocker file");

        let store: Arc<dyn EventStore> =
            Arc::new(FileLogMirror::new(Arc::clone(&inner), blocker.clone()));
        let session = Session::new_with_store(store, Uuid::new_v4(), "lite".into())
            .await
            .expect("create session");

        session
            .append(json!({"event": "first"}))
            .await
            .expect("first append still succeeds");
        session
            .append(json!({"event": "second"}))
            .await
            .expect("second append still succeeds");

        let replay = inner.replay(session.id()).await.expect("replay");
        let payloads: Vec<&Value> = replay.iter().map(|event| &event.payload).collect();

        assert_eq!(payloads[0], &json!({"event": "first"}));
        assert_eq!(
            payloads[1].get("event").and_then(Value::as_str),
            Some("file_log_write_failed")
        );
        assert_eq!(
            payloads[1].get("error_class").and_then(Value::as_str),
            Some("io_other")
        );
        assert_eq!(payloads[2], &json!({"event": "second"}));
        assert_eq!(
            payloads
                .iter()
                .filter(|payload| {
                    payload.get("event").and_then(Value::as_str) == Some("file_log_write_failed")
                })
                .count(),
            1,
            "mirror should emit the degradation event once per broken session"
        );

        let _ = std::fs::remove_file(blocker);
    }
}
