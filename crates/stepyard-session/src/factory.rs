use std::sync::Arc;

#[cfg(feature = "sqlite")]
use std::path::PathBuf;
#[cfg(feature = "postgres")]
use std::time::Duration;

#[cfg(feature = "postgres")]
use sqlx::postgres::PgPoolOptions;
#[cfg(feature = "sqlite")]
use sqlx::sqlite::SqlitePoolOptions;

use crate::file_log_mirror::{FileLogConfig, FileLogMirror};
#[cfg(feature = "postgres")]
use crate::pg_store::PgEventStore;
#[cfg(feature = "sqlite")]
use crate::sqlite_store::SqliteEventStore;
use crate::store::SessionError;
use crate::store_trait::DynEventStore;

#[derive(Debug, Clone, Default)]
pub struct StoreConfig {
    pub file_logs: FileLogConfig,
}

pub async fn build_store_from_env() -> Result<DynEventStore, SessionError> {
    build_store_with_file_logs(StoreConfig::default()).await
}

pub async fn build_store_with_file_logs(
    config: StoreConfig,
) -> Result<DynEventStore, SessionError> {
    let store = build_raw_store_from_env().await?;
    if config.file_logs.enabled {
        Ok(Arc::new(FileLogMirror::new(
            store,
            config.file_logs.directory,
        )))
    } else {
        Ok(store)
    }
}

#[cfg(feature = "postgres")]
async fn build_raw_store_from_env() -> Result<DynEventStore, SessionError> {
    let db_url = std::env::var("DATABASE_URL")
        .or_else(|_| std::env::var("STEPYARD_HARNESS_DATABASE_URL"))
        .map_err(|_| {
            SessionError::InvalidState(
                "postgres backend requires DATABASE_URL or STEPYARD_HARNESS_DATABASE_URL".into(),
            )
        })?;

    let pool = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&db_url)
        .await?;

    crate::migrate(&pool).await?;
    Ok(Arc::new(PgEventStore::new(pool)))
}

#[cfg(all(feature = "sqlite", not(feature = "postgres")))]
async fn build_raw_store_from_env() -> Result<DynEventStore, SessionError> {
    let path = std::env::var("STEPYARD_SQLITE_PATH")
        .or_else(|_| std::env::var("STEPYARD_HARNESS_DATABASE_URL"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".stepyard").join("sessions.db"));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            SessionError::InvalidState(format!(
                "failed to create sqlite session directory `{}`: {e}",
                parent.display()
            ))
        })?;
    }

    let url = format!("sqlite://{}?mode=rwc", path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;

    crate::migrate_sqlite(&pool).await?;
    Ok(Arc::new(SqliteEventStore::new(pool)))
}
