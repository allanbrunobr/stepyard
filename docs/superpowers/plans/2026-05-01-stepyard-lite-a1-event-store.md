# Stepyard Lite PR A1 — EventStore Trait + SQLite Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor `stepyard-session` to be backend-agnostic via an `EventStore` trait, add `SqliteEventStore` as a compile-time alternative to the existing Postgres path, and verify byte-equivalent replay behavior across both backends.

**Architecture:** Mutually-exclusive Cargo features `postgres` (default) and `sqlite` at the workspace root. The existing `Session` struct, currently bound to `PgPool`, is refactored to hold `Arc<dyn EventStore>`. The current Postgres SQL extracts into `PgEventStore`; a new `SqliteEventStore` implements the same trait. Migrations split into `migrations-pg/` and `migrations-sqlite/`. A CI matrix lane is added so both backends are tested every PR.

**Tech Stack:** Rust 2021, sqlx 0.8 (`postgres` + `sqlite` drivers, runtime queries only), tokio, async_trait 0.1, serde_json, uuid, chrono.

**Spec reference:** `docs/superpowers/specs/2026-05-01-stepyard-lite-design.md` §4–§5.

**Prerequisites:**
- Task #80 merged (v2-default flip prerequisite — we don't want event-schema churn fighting v2 migration).
- Postgres available locally for the postgres-lane tests: `STEPYARD_HARNESS_DATABASE_URL=postgres://minion:minion_secret@localhost:5433/minion_engine`.

---

## File Structure

**Workspace root:**
- Modify: `Cargo.toml` — add `[workspace.features]` (or document feature pass-through pattern).

**`crates/stepyard-session/`:**
- Modify: `Cargo.toml` — feature-gate sqlx drivers.
- Modify: `src/lib.rs` — compile_error guards, exports.
- Create: `src/store_trait.rs` — `EventStore` async trait, `SessionMeta` struct.
- Create: `src/pg_store.rs` — `PgEventStore` (extracts current Session SQL, gated `--features postgres`).
- Create: `src/sqlite_store.rs` — `SqliteEventStore` (new, gated `--features sqlite`).
- Create: `src/factory.rs` — `build_store_from_env()` returning `Arc<dyn EventStore>`.
- Modify: `src/session.rs` — `Session` holds `Arc<dyn EventStore>`; methods delegate.
- Move: `migrations/` → `migrations-pg/` (rename the dir; same files).
- Create: `migrations-sqlite/0001_initial.sql` — schema mirror with SQLite types.
- Modify: `tests/integration.rs` — use factory.

**`crates/stepyard-harness/`:**
- Modify: `Cargo.toml` — feature pass-through to stepyard-session.
- Modify: `src/engine.rs` — propagate factory through `Engine::with_executor`.
- Modify: every `tests/*.rs` that opens a `PgPool` today (12 files; the pattern is identical).
- Create: `tests/replay_parity.rs` — gated by a `parity-test` feature on the harness crate.

**Binary:**
- Modify: `src/main.rs` — use factory.

**CI:**
- Modify: `.github/workflows/check.yml` — backend matrix.

---

## Task 1: Add workspace-level Cargo features

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Inspect current workspace Cargo.toml**

Run: `cat Cargo.toml | head -40`
Expected: see `[workspace]` section with members list, no `[features]` section yet.

- [ ] **Step 2: Add features section to workspace root**

Open `Cargo.toml`. After the `[workspace]` block, append:

```toml
[workspace.metadata.stepyard]
# Build profiles for the engine.
# - `postgres` (default): production. PgPool event store + Docker sandbox.
# - `sqlite`            : Lite mode. SqlitePool event store + LocalShell sandbox.
# Mutually exclusive. Enforced by compile_error! in stepyard-session.

[features]
default = ["postgres"]
postgres = ["stepyard-session/postgres", "stepyard-engine/postgres"]
sqlite   = ["stepyard-session/sqlite",   "stepyard-engine/sqlite"]
```

Note: workspace-root `[features]` requires Cargo 1.74+ for the `--features` flag to propagate; verify `cargo --version` is at least that.

- [ ] **Step 3: Verify postgres path still builds**

Run: `cargo check --workspace --no-default-features --features postgres 2>&1 | tail -5`
Expected: `Finished ... in Xs` (compile errors expected at this point because crate-level features aren't defined yet — capture them as the next task's input).

- [ ] **Step 4: Commit**

```bash
git checkout -b feat/pr-a1-event-store-trait
git add Cargo.toml
git commit -m "chore(workspace): add postgres/sqlite feature scaffolding for Stepyard Lite"
```

---

## Task 2: Feature-gate sqlx drivers in stepyard-session

**Files:**
- Modify: `crates/stepyard-session/Cargo.toml`

- [ ] **Step 1: Read current Cargo.toml**

Run: `cat crates/stepyard-session/Cargo.toml`
Expected: see `sqlx = { version = "...", features = ["runtime-tokio-rustls", "postgres", "uuid", "chrono", "json"] }` or similar.

- [ ] **Step 2: Refactor sqlx dep to be feature-driven**

Replace the `[dependencies]` sqlx entry with:

```toml
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio-rustls", "uuid", "chrono", "json", "macros"] }
async-trait = "0.1"

[features]
default = ["postgres"]
postgres = ["sqlx/postgres"]
sqlite   = ["sqlx/sqlite"]
```

(Adjust `version = "0.8"` to whatever the workspace already uses; check `Cargo.lock` to confirm.)

- [ ] **Step 3: Verify postgres lane still builds**

Run: `cargo check -p stepyard-session --no-default-features --features postgres 2>&1 | tail -10`
Expected: build succeeds.

- [ ] **Step 4: Verify sqlite lane has driver wired (no impl yet — expect later compile errors)**

Run: `cargo check -p stepyard-session --no-default-features --features sqlite 2>&1 | tail -20`
Expected: failure mentions `PgPool` not found (because src/session.rs still references it). This is expected — fixed in subsequent tasks.

- [ ] **Step 5: Commit**

```bash
git add crates/stepyard-session/Cargo.toml
git commit -m "chore(stepyard-session): feature-gate sqlx drivers behind postgres/sqlite"
```

---

## Task 3: Add compile_error guards in stepyard-session/src/lib.rs

**Files:**
- Modify: `crates/stepyard-session/src/lib.rs:1-30`

- [ ] **Step 1: Add compile_error guard for both-features-enabled**

Open `crates/stepyard-session/src/lib.rs` and add at the top of the file (after the crate-level `//!` doc):

```rust
#[cfg(all(feature = "postgres", feature = "sqlite"))]
compile_error!(
    "stepyard-session: features `postgres` and `sqlite` are mutually exclusive. \
     Build with `--no-default-features --features postgres` OR \
     `--no-default-features --features sqlite`."
);

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
compile_error!(
    "stepyard-session: at least one backend feature must be enabled. \
     Use `--features postgres` (default) or `--features sqlite`."
);
```

- [ ] **Step 2: Verify guard fires when both are enabled**

Run: `cargo check -p stepyard-session --features postgres,sqlite 2>&1 | grep -E "compile_error|mutually exclusive"`
Expected: see the guard error message.

- [ ] **Step 3: Verify guard fires when neither is enabled**

Run: `cargo check -p stepyard-session --no-default-features 2>&1 | grep -E "compile_error|at least one"`
Expected: see the guard error message.

- [ ] **Step 4: Verify default postgres still builds**

Run: `cargo check -p stepyard-session 2>&1 | tail -5`
Expected: build succeeds.

- [ ] **Step 5: Commit**

```bash
git add crates/stepyard-session/src/lib.rs
git commit -m "feat(stepyard-session): enforce mutually exclusive postgres/sqlite features"
```

---

## Task 4: Define EventStore trait + SessionMeta

**Files:**
- Create: `crates/stepyard-session/src/store_trait.rs`
- Modify: `crates/stepyard-session/src/lib.rs` — add `mod store_trait;` and re-exports.

- [ ] **Step 1: Write a failing trait-shape test in store_trait module**

Create `crates/stepyard-session/src/store_trait.rs` with the trait stub and a doctest:

```rust
//! The [`EventStore`] trait — the backend-agnostic interface for the
//! stepyard-session append-only log. Two impls ship in this crate:
//! [`crate::pg_store::PgEventStore`] (`--features postgres`) and
//! [`crate::sqlite_store::SqliteEventStore`] (`--features sqlite`).
//!
//! The trait is the seam introduced in PR A1 of the Stepyard Lite spec
//! (`docs/superpowers/specs/2026-05-01-stepyard-lite-design.md`).
//!
//! ```compile_fail
//! // This must NOT compile because no impl is named here.
//! use stepyard_session::EventStore;
//! fn assert_object_safe(_: &dyn EventStore) {}
//! fn main() {
//!     // Object safety check happens at compile time — caller must
//!     // pass an actual impl. Lack of any impl in scope makes the
//!     // function callable only via Arc<dyn EventStore>.
//! }
//! ```

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::store::{SessionError, SessionEvent, SessionId};

/// A snapshot of a session row: identity + lifecycle metadata. Returned
/// by [`EventStore::load_session_meta`] for hydrating a [`crate::Session`].
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: SessionId,
    pub workflow_id: Uuid,
    pub tenant_id: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// The append-only event log interface. Implementations must guarantee:
/// * `append` is serialized per session (via DB-level mechanism).
/// * `replay` returns events in `seq ASC` order, never timestamp.
/// * `seq` is monotonic without gaps.
/// * Events are immutable once written.
///
/// Concurrent multi-process writes are supported by Postgres impl;
/// SQLite impl is single-process only (Lite mode).
#[async_trait]
pub trait EventStore: Send + Sync + 'static {
    /// Insert a new `sessions` row with status `running`. Returns the
    /// hydrated metadata so the caller can construct a `Session`.
    async fn create_session(
        &self,
        id: SessionId,
        workflow_id: Uuid,
        tenant_id: &str,
    ) -> Result<SessionMeta, SessionError>;

    /// Load an existing session row. Returns [`SessionError::NotFound`]
    /// if no row matches `id`.
    async fn load_session_meta(&self, id: SessionId) -> Result<SessionMeta, SessionError>;

    /// Append an event to the log. The implementation is responsible for
    /// per-session serialization. In Postgres this is `pg_advisory_xact_lock`
    /// inside a transaction; in SQLite this is `BEGIN IMMEDIATE`.
    async fn append(
        &self,
        session_id: SessionId,
        payload: JsonValue,
    ) -> Result<SessionEvent, SessionError>;

    /// Read all events for a session in `seq ASC` order.
    async fn replay(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError>;

    /// Update the lifecycle status (only transitions from `running`).
    /// Returns `Ok(Some(meta))` if the row was updated, `Ok(None)` if the
    /// row was already in a terminal state (no-op idempotency).
    async fn update_status(
        &self,
        id: SessionId,
        new_status: &str,
    ) -> Result<Option<SessionMeta>, SessionError>;

    /// Run backend-specific migrations. Idempotent.
    async fn migrate(&self) -> Result<(), SessionError>;
}
```

- [ ] **Step 2: Wire the module into lib.rs**

Open `crates/stepyard-session/src/lib.rs` and add (near the other `mod` declarations):

```rust
mod store_trait;
pub use store_trait::{EventStore, SessionMeta};
```

- [ ] **Step 3: Verify the trait compiles**

Run: `cargo check -p stepyard-session --features postgres --no-default-features 2>&1 | tail -10`
Expected: success (no impl exists yet, but the trait alone compiles).

- [ ] **Step 4: Verify the trait is object-safe**

Add a temporary check at the bottom of `store_trait.rs`:

```rust
#[allow(dead_code)]
fn _assert_object_safe(_: Box<dyn EventStore>) {}
```

Run: `cargo check -p stepyard-session 2>&1 | tail -5`
Expected: success. If it fails with "the trait `EventStore` cannot be made into an object", revisit method signatures (no `Self` returns, no generic methods).

- [ ] **Step 5: Remove the temporary object-safety helper**

Delete the `_assert_object_safe` function from `store_trait.rs`.

- [ ] **Step 6: Commit**

```bash
git add crates/stepyard-session/src/store_trait.rs crates/stepyard-session/src/lib.rs
git commit -m "feat(stepyard-session): define EventStore trait + SessionMeta"
```

---

## Task 5: Extract PgEventStore from current Session impl

**Files:**
- Create: `crates/stepyard-session/src/pg_store.rs`
- Modify: `crates/stepyard-session/src/lib.rs` — register the module under `--features postgres`.

- [ ] **Step 1: Write a failing unit test for PgEventStore::create_session**

Create `crates/stepyard-session/tests/pg_store_test.rs`:

```rust
//! Unit tests for PgEventStore — the existing Postgres path moved
//! behind the EventStore trait. These mirror what tests/integration.rs
//! already exercises but call PgEventStore directly.

#![cfg(feature = "postgres")]

use sqlx::postgres::PgPoolOptions;
use stepyard_session::{EventStore, PgEventStore, SessionId};
use uuid::Uuid;

async fn store() -> Option<PgEventStore> {
    let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");
    Some(PgEventStore::new(pool))
}

#[tokio::test]
async fn create_session_returns_meta_with_running_status() {
    let Some(store) = store().await else {
        eprintln!("[skip] STEPYARD_HARNESS_DATABASE_URL not set");
        return;
    };
    store.migrate().await.expect("migrate");

    let id = SessionId::new();
    let meta = store
        .create_session(id, Uuid::new_v4(), "test-tenant")
        .await
        .expect("create");
    assert_eq!(meta.status, "running");
    assert!(meta.ended_at.is_none());
}
```

- [ ] **Step 2: Run the test, see it fail (PgEventStore not defined)**

Run: `cargo test -p stepyard-session --test pg_store_test --features postgres --no-default-features 2>&1 | tail -10`
Expected: compile error `cannot find type PgEventStore in crate stepyard_session`.

- [ ] **Step 3: Create pg_store.rs with the impl**

Create `crates/stepyard-session/src/pg_store.rs`:

```rust
//! Postgres implementation of [`EventStore`]. Originally inline in
//! [`crate::session::Session`]; extracted in PR A1 to allow the
//! [`crate::sqlite_store::SqliteEventStore`] sibling.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use crate::store::{SessionError, SessionEvent, SessionId};
use crate::store_trait::{EventStore, SessionMeta};

/// Postgres-backed event store.
#[derive(Debug, Clone)]
pub struct PgEventStore {
    pool: PgPool,
}

impl PgEventStore {
    /// Wrap an existing `PgPool`. Cloning is cheap (`PgPool` is Arc-internal).
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Borrow the underlying pool. Used by tests that need to clean up
    /// directly via DELETE statements.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl EventStore for PgEventStore {
    async fn create_session(
        &self,
        id: SessionId,
        workflow_id: Uuid,
        tenant_id: &str,
    ) -> Result<SessionMeta, SessionError> {
        let row: (Uuid, String, DateTime<Utc>) = sqlx::query_as(
            r#"
            INSERT INTO sessions (id, workflow_id, tenant_id, status, started_at)
            VALUES ($1, $2, $3, 'running', NOW())
            RETURNING id, status, started_at
            "#,
        )
        .bind(id.as_uuid())
        .bind(workflow_id)
        .bind(tenant_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(SessionMeta {
            id: SessionId(row.0),
            workflow_id,
            tenant_id: tenant_id.to_owned(),
            status: row.1,
            started_at: row.2,
            ended_at: None,
        })
    }

    async fn load_session_meta(&self, id: SessionId) -> Result<SessionMeta, SessionError> {
        let row: Option<(Uuid, Uuid, String, String, DateTime<Utc>, Option<DateTime<Utc>>)> =
            sqlx::query_as(
                r#"
                SELECT id, workflow_id, tenant_id, status, started_at, ended_at
                FROM sessions
                WHERE id = $1
                "#,
            )
            .bind(id.as_uuid())
            .fetch_optional(&self.pool)
            .await?;

        let (id_db, workflow_id, tenant_id, status, started_at, ended_at) =
            row.ok_or(SessionError::NotFound(id))?;

        Ok(SessionMeta {
            id: SessionId(id_db),
            workflow_id,
            tenant_id,
            status,
            started_at,
            ended_at,
        })
    }

    async fn append(
        &self,
        session_id: SessionId,
        payload: JsonValue,
    ) -> Result<SessionEvent, SessionError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
            .bind(session_id.as_uuid().to_string())
            .execute(&mut *tx)
            .await?;

        let row: (Uuid, Uuid, i64, DateTime<Utc>, JsonValue) = sqlx::query_as(
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

        Ok(SessionEvent {
            id: row.0,
            session_id: SessionId(row.1),
            seq: row.2,
            created_at: row.3,
            payload: row.4,
        })
    }

    async fn replay(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError> {
        let rows: Vec<(Uuid, Uuid, i64, DateTime<Utc>, JsonValue)> = sqlx::query_as(
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

    async fn update_status(
        &self,
        id: SessionId,
        new_status: &str,
    ) -> Result<Option<SessionMeta>, SessionError> {
        let row: Option<(Uuid, Uuid, String, String, DateTime<Utc>, Option<DateTime<Utc>>)> =
            sqlx::query_as(
                r#"
                UPDATE sessions
                SET status = $2, ended_at = NOW()
                WHERE id = $1 AND status = 'running'
                RETURNING id, workflow_id, tenant_id, status, started_at, ended_at
                "#,
            )
            .bind(id.as_uuid())
            .bind(new_status)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|(id_db, workflow_id, tenant_id, status, started_at, ended_at)| {
            SessionMeta {
                id: SessionId(id_db),
                workflow_id,
                tenant_id,
                status,
                started_at,
                ended_at,
            }
        }))
    }

    async fn migrate(&self) -> Result<(), SessionError> {
        sqlx::migrate!("./migrations-pg")
            .run(&self.pool)
            .await
            .map_err(|e| SessionError::Database(format!("postgres migrate: {e}").into()))
    }
}
```

- [ ] **Step 4: Wire pg_store module into lib.rs (gated)**

Open `crates/stepyard-session/src/lib.rs` and add:

```rust
#[cfg(feature = "postgres")]
mod pg_store;
#[cfg(feature = "postgres")]
pub use pg_store::PgEventStore;
```

- [ ] **Step 5: Run the unit test**

Run: `STEPYARD_HARNESS_DATABASE_URL='postgres://minion:minion_secret@localhost:5433/minion_engine' cargo test -p stepyard-session --test pg_store_test --features postgres --no-default-features 2>&1 | tail -10`
Expected: 1 passed.

If the test fails with "missing migrations dir" — that's because Task 9 hasn't moved migrations to `migrations-pg/` yet. You can either skip this step until after Task 9 OR temporarily bypass migrate() in this test (it's already idempotent against an existing schema).

For the linear plan order, do this: comment out the `store.migrate().await.expect("migrate");` line in the test for now; uncomment it as part of Task 9.

- [ ] **Step 6: Commit**

```bash
git add crates/stepyard-session/src/pg_store.rs crates/stepyard-session/src/lib.rs crates/stepyard-session/tests/pg_store_test.rs
git commit -m "feat(stepyard-session): extract PgEventStore from Session into trait impl"
```

---

## Task 6: Refactor Session struct to hold Arc<dyn EventStore>

**Files:**
- Modify: `crates/stepyard-session/src/session.rs` (whole file)

This is the largest single edit in PR A1. The shape: replace `pool: PgPool` with `store: Arc<dyn EventStore>`; replace inline SQL with delegation to trait methods.

- [ ] **Step 1: Read the existing session.rs once more end to end**

Run: `wc -l crates/stepyard-session/src/session.rs && sed -n '1,80p' crates/stepyard-session/src/session.rs`
Expected: ~330 lines.

- [ ] **Step 2: Replace the file**

Open `crates/stepyard-session/src/session.rs` and replace the entire contents with:

```rust
//! The [`Session`] handle — the public entry point for the append-only log.
//!
//! A `Session` is cheaply cloneable (`Clone + Send + Sync`) because internally
//! it holds an `Arc<dyn EventStore>` and a few small fields. Cloning does not
//! allocate.
//!
//! As of PR A1 of the Stepyard Lite work, the storage backend is selected at
//! compile time via Cargo features (`postgres` or `sqlite`).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::store::{SessionError, SessionEvent, SessionId};
use crate::store_trait::{EventStore, SessionMeta};

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

#[derive(Clone)]
pub struct Session {
    id: SessionId,
    workflow_id: Uuid,
    tenant_id: String,
    status: SessionStatus,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    store: Arc<dyn EventStore>,
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
    fn from_meta(meta: SessionMeta, store: Arc<dyn EventStore>) -> Result<Self, SessionError> {
        Ok(Self {
            id: meta.id,
            workflow_id: meta.workflow_id,
            tenant_id: meta.tenant_id,
            status: SessionStatus::from_db(&meta.status)?,
            started_at: meta.started_at,
            ended_at: meta.ended_at,
            store,
        })
    }

    pub async fn new(
        store: Arc<dyn EventStore>,
        workflow_id: Uuid,
        tenant_id: String,
    ) -> Result<Self, SessionError> {
        let id = SessionId::new();
        let meta = store.create_session(id, workflow_id, &tenant_id).await?;
        Self::from_meta(meta, store)
    }

    pub async fn load(store: Arc<dyn EventStore>, id: SessionId) -> Result<Self, SessionError> {
        let meta = store.load_session_meta(id).await?;
        Self::from_meta(meta, store)
    }

    pub async fn append(
        &self,
        payload: serde_json::Value,
    ) -> Result<SessionEvent, SessionError> {
        self.store.append(self.id, payload).await
    }

    pub async fn replay(&self) -> Result<Vec<SessionEvent>, SessionError> {
        self.store.replay(self.id).await
    }

    pub fn id(&self) -> SessionId { self.id }
    pub fn workflow_id(&self) -> Uuid { self.workflow_id }
    pub fn tenant_id(&self) -> &str { &self.tenant_id }
    pub fn status(&self) -> SessionStatus { self.status }
    pub fn started_at(&self) -> DateTime<Utc> { self.started_at }
    pub fn ended_at(&self) -> Option<DateTime<Utc>> { self.ended_at }

    pub async fn complete(&mut self) -> Result<(), SessionError> {
        self.finish(SessionStatus::Completed).await
    }

    pub async fn fail(&mut self) -> Result<(), SessionError> {
        self.finish(SessionStatus::Failed).await
    }

    pub async fn cancel(&mut self) -> Result<(), SessionError> {
        self.finish(SessionStatus::Cancelled).await
    }

    async fn finish(&mut self, status: SessionStatus) -> Result<(), SessionError> {
        if let Some(meta) = self.store.update_status(self.id, status.as_str()).await? {
            self.status = SessionStatus::from_db(&meta.status)?;
            self.ended_at = meta.ended_at;
        }
        Ok(())
    }
}
```

- [ ] **Step 3: Update lib.rs to remove the old `migrate()` free function**

The old `pub async fn migrate(pool: &sqlx::PgPool) -> Result<(), SessionError>` is now `EventStore::migrate(&self)`. Find and delete it from `crates/stepyard-session/src/lib.rs`.

Also update the lib.rs example doctest (around line 21) to use the new API. Replace:

```rust
//! use sqlx::postgres::PgPoolOptions;
//! ...
//! let pool = PgPoolOptions::new()...
//! let session = Session::new(&pool, ...).await?;
```

with:

```rust
//! use stepyard_session::{Session, build_store_from_env};
//!
//! let store = build_store_from_env().await?;
//! let session = Session::new(store, workflow_id, tenant.into()).await?;
```

- [ ] **Step 4: Verify compilation under postgres feature**

Run: `cargo check -p stepyard-session --features postgres --no-default-features 2>&1 | tail -10`
Expected: success.

- [ ] **Step 5: Run existing integration tests against postgres**

Note: `tests/integration.rs` and other call sites still pass `&PgPool`. They will fail to compile until Task 8. That's expected — DO NOT mark step 5 as failure-of-this-task; it's the next task's input.

Run: `cargo check -p stepyard-session --tests --features postgres --no-default-features 2>&1 | head -30`
Expected: errors of the form "expected `Arc<dyn EventStore>`, found `&PgPool`" — confirms the surface change is detected.

- [ ] **Step 6: Commit**

```bash
git add crates/stepyard-session/src/session.rs crates/stepyard-session/src/lib.rs
git commit -m "refactor(stepyard-session): Session holds Arc<dyn EventStore>, delegates to trait"
```

---

## Task 7: Add build_store_from_env factory

**Files:**
- Create: `crates/stepyard-session/src/factory.rs`
- Modify: `crates/stepyard-session/src/lib.rs`

- [ ] **Step 1: Write the factory**

Create `crates/stepyard-session/src/factory.rs`:

```rust
//! Backend-agnostic factory for [`crate::EventStore`]. The chosen impl is
//! determined at compile time by the `postgres` / `sqlite` Cargo features.
//! This is the single entry point production callers should use; it
//! reads `STEPYARD_HARNESS_DATABASE_URL` and dispatches accordingly.

use std::sync::Arc;

use crate::store::SessionError;
use crate::store_trait::EventStore;

/// Build the configured event store from the environment.
///
/// Reads `STEPYARD_HARNESS_DATABASE_URL`. If unset, the SQLite backend
/// defaults to `~/.stepyard/sessions.db`; the Postgres backend errors.
pub async fn build_store_from_env() -> Result<Arc<dyn EventStore>, SessionError> {
    #[cfg(feature = "postgres")]
    {
        use sqlx::postgres::PgPoolOptions;
        let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL").map_err(|_| {
            SessionError::InvalidState(
                "STEPYARD_HARNESS_DATABASE_URL is required for postgres builds".into(),
            )
        })?;
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await
            .map_err(|e| SessionError::Database(format!("connect postgres: {e}").into()))?;
        Ok(Arc::new(crate::PgEventStore::new(pool)))
    }
    #[cfg(feature = "sqlite")]
    {
        use sqlx::sqlite::SqlitePoolOptions;
        let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL")
            .unwrap_or_else(|_| default_sqlite_url());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .map_err(|e| SessionError::Database(format!("connect sqlite: {e}").into()))?;
        Ok(Arc::new(crate::SqliteEventStore::new(pool)))
    }
}

#[cfg(feature = "sqlite")]
fn default_sqlite_url() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let path = format!("{home}/.stepyard/sessions.db");
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    format!("sqlite://{path}?mode=rwc")
}
```

- [ ] **Step 2: Wire into lib.rs**

Add to `crates/stepyard-session/src/lib.rs`:

```rust
mod factory;
pub use factory::build_store_from_env;
```

- [ ] **Step 3: Verify build under postgres feature**

Run: `cargo check -p stepyard-session --features postgres --no-default-features 2>&1 | tail -5`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add crates/stepyard-session/src/factory.rs crates/stepyard-session/src/lib.rs
git commit -m "feat(stepyard-session): add build_store_from_env factory"
```

---

## Task 8: Update all call sites

**Files (~13 files):**
- Modify: `crates/stepyard-session/tests/integration.rs`
- Modify: `crates/stepyard-harness/tests/scope_replay.rs`
- Modify: `crates/stepyard-harness/tests/parallel_v2.rs`
- Modify: `crates/stepyard-harness/tests/agent_replay.rs`
- Modify: `crates/stepyard-harness/tests/cancel_cleanup.rs`
- Modify: `crates/stepyard-harness/tests/chat_cancel_replay.rs`
- Modify: `crates/stepyard-harness/tests/chat_replay.rs`
- Modify: `crates/stepyard-harness/tests/concurrent_sessions.rs`
- Modify: `crates/stepyard-harness/tests/gate_replay.rs`
- Modify: `crates/stepyard-harness/tests/signal_cancel.rs`
- Modify: `crates/stepyard-harness/tests/signal_replay.rs`
- Modify: `crates/stepyard-harness/tests/startup_reconcile.rs`
- Modify: `crates/stepyard-harness/tests/step_cancel_replay.rs`
- Modify: `crates/stepyard-harness/tests/template_replay.rs`
- Modify: `crates/stepyard-harness/tests/timeout_zero.rs`
- Modify: `src/main.rs`

The pattern is identical across all test files. Apply once, then sweep.

- [ ] **Step 1: Update stepyard-session/tests/integration.rs**

Replace the `pool()` helper:

```rust
async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("STEPYARD_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new().max_connections(2).connect(&url).await.expect("...");
    sqlx::migrate!("./migrations").run(&pool).await.expect("...");
    Some(pool)
}
```

with:

```rust
use std::sync::Arc;
use stepyard_session::{build_store_from_env, EventStore, PgEventStore};

async fn store() -> Option<Arc<dyn EventStore>> {
    if std::env::var("STEPYARD_HARNESS_DATABASE_URL").is_err() {
        return None;
    }
    let store = build_store_from_env().await.expect("build store");
    store.migrate().await.expect("migrate");
    Some(store)
}
```

Then update every `Session::new(&pool, ...)` to `Session::new(store.clone(), ...)`. Where the test reaches into the pool directly (`sqlx::query("DELETE FROM session_events WHERE session_id = $1")`), use a typed downcast helper:

```rust
fn pg_pool_for_cleanup(store: &Arc<dyn EventStore>) -> &sqlx::PgPool {
    // SAFETY: tests run only against postgres; downcast is a test-only escape hatch.
    let any_store = store.as_ref() as &dyn std::any::Any;
    any_store
        .downcast_ref::<PgEventStore>()
        .expect("test requires PgEventStore")
        .pool()
}
```

(For this to work, `EventStore` needs to require `: std::any::Any` — add `+ std::any::Any` to the trait bound or expose a `as_any` method. Pick the latter to avoid adding the bound to all impls. Add to `store_trait.rs`:)

```rust
#[async_trait]
pub trait EventStore: Send + Sync + 'static {
    // ... existing methods ...

    /// Test-only escape hatch for downcasting. Production code must NEVER
    /// call this — use the trait methods.
    #[doc(hidden)]
    fn as_any(&self) -> &dyn std::any::Any;
}
```

And in PgEventStore impl: `fn as_any(&self) -> &dyn std::any::Any { self }`. Same for SqliteEventStore once it lands.

- [ ] **Step 2: Sweep update across stepyard-harness/tests/*.rs**

Apply the same `pool()` → `store()` rewrite. The harness tests use:

```rust
async fn pool() -> Option<sqlx::PgPool> { ... }
let session = Session::new(&pool, Uuid::new_v4(), "edenred".into()).await?;
```

Convert to:

```rust
async fn store() -> Option<Arc<dyn EventStore>> { ... }
let session = Session::new(store.clone(), Uuid::new_v4(), "edenred".into()).await?;
```

Find every `Session::new(&pool` site to enumerate the work:

Run: `grep -rn "Session::new(&pool" crates/`
Expected: lists all call sites needing update.

- [ ] **Step 3: Update Engine constructor signature in stepyard-harness/src/engine.rs**

`Engine::with_executor` already takes a `Session`, not a pool — so no signature change needed at the engine level. Verify:

Run: `grep -n "pub fn with_executor\|pub fn new" crates/stepyard-harness/src/engine.rs | head -5`
Expected: signatures take a `Session`, not `PgPool`. If they take a pool, refactor to take `Session`.

- [ ] **Step 4: Update src/main.rs**

Replace any `PgPoolOptions::new()...connect(&url).await?` followed by `Session::new(&pool, ...)` with:

```rust
let store = stepyard_session::build_store_from_env().await?;
store.migrate().await?;
let session = stepyard_session::Session::new(store, workflow_id, tenant_id).await?;
```

- [ ] **Step 5: Verify postgres lane builds**

Run: `cargo build --workspace --no-default-features --features postgres 2>&1 | tail -10`
Expected: success.

- [ ] **Step 6: Run postgres lane test suite**

Run: `STEPYARD_HARNESS_DATABASE_URL='postgres://minion:minion_secret@localhost:5433/minion_engine' cargo test --workspace --no-default-features --features postgres 2>&1 | tail -30`
Expected: same baseline as before this PR (pre-existing 2 agent_replay failures only).

- [ ] **Step 7: Commit**

```bash
git add -u
git commit -m "refactor: route all Session::new sites through build_store_from_env factory"
```

---

## Task 9: Migrations split

**Files:**
- Move: `crates/stepyard-session/migrations/` → `crates/stepyard-session/migrations-pg/`
- Modify: `crates/stepyard-session/src/pg_store.rs` — already references `./migrations-pg`, verify.
- Modify: `crates/stepyard-session/tests/integration.rs` — if it references the old path, fix.

- [ ] **Step 1: Rename the dir**

```bash
git mv crates/stepyard-session/migrations crates/stepyard-session/migrations-pg
```

- [ ] **Step 2: Verify all in-source references point at the new path**

Run: `grep -rn "migrations" crates/stepyard-session/`
Expected: only `migrations-pg` references in `src/pg_store.rs` and possibly `tests/integration.rs`. If you find a `migrations` (no suffix), fix to `migrations-pg`.

- [ ] **Step 3: Re-enable the commented-out migrate() call from Task 5**

Open `crates/stepyard-session/tests/pg_store_test.rs` and uncomment:
```rust
store.migrate().await.expect("migrate");
```

- [ ] **Step 4: Verify postgres test suite passes**

Run: `STEPYARD_HARNESS_DATABASE_URL='postgres://minion:minion_secret@localhost:5433/minion_engine' cargo test -p stepyard-session --features postgres --no-default-features 2>&1 | tail -10`
Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add -u crates/stepyard-session/
git commit -m "chore(stepyard-session): rename migrations/ to migrations-pg/"
```

---

## Task 10: Author SQLite migrations

**Files:**
- Create: `crates/stepyard-session/migrations-sqlite/0001_initial.sql`

- [ ] **Step 1: Read the existing Postgres schema to mirror**

Run: `cat crates/stepyard-session/migrations-pg/*.sql`
Expected: see `CREATE TABLE sessions ...`, `CREATE TABLE session_events ...`, plus indexes.

- [ ] **Step 2: Author the SQLite mirror**

Create `crates/stepyard-session/migrations-sqlite/0001_initial.sql`:

```sql
-- SQLite mirror of migrations-pg/0001_initial.sql.
-- Differences vs Postgres:
--   * UUID type does not exist in SQLite; use TEXT (16-byte BLOB also possible
--     but TEXT keeps debug-readability). UUID values are written as their
--     canonical string form.
--   * JSONB does not exist in SQLite; JSON is TEXT with json_valid() CHECK.
--   * TIMESTAMP WITH TIME ZONE does not exist; use TEXT in ISO-8601 form
--     (sqlx::types::chrono::DateTime<Utc> serializes that way).
--   * No `gen_random_uuid()`; UUID is generated host-side.
--   * No `pg_advisory_xact_lock`; serialization is via BEGIN IMMEDIATE in app.

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY NOT NULL,
    workflow_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'failed', 'cancelled')),
    started_at TEXT NOT NULL,
    ended_at TEXT
);

CREATE TABLE IF NOT EXISTS session_events (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    seq INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    payload TEXT NOT NULL CHECK (json_valid(payload)),
    UNIQUE (session_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_session_events_session_seq
    ON session_events (session_id, seq);
```

- [ ] **Step 3: Commit**

```bash
git add crates/stepyard-session/migrations-sqlite/0001_initial.sql
git commit -m "feat(stepyard-session): add SQLite mirror migrations"
```

---

## Task 11: SqliteEventStore stub

**Files:**
- Create: `crates/stepyard-session/src/sqlite_store.rs`
- Modify: `crates/stepyard-session/src/lib.rs`

- [ ] **Step 1: Create the stub file**

Create `crates/stepyard-session/src/sqlite_store.rs`:

```rust
//! SQLite implementation of [`crate::EventStore`] for Stepyard Lite mode.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::store::{SessionError, SessionEvent, SessionId};
use crate::store_trait::{EventStore, SessionMeta};

#[derive(Debug, Clone)]
pub struct SqliteEventStore {
    pool: SqlitePool,
}

impl SqliteEventStore {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }
    pub fn pool(&self) -> &SqlitePool { &self.pool }
}

#[async_trait]
impl EventStore for SqliteEventStore {
    async fn create_session(
        &self,
        _id: SessionId,
        _workflow_id: Uuid,
        _tenant_id: &str,
    ) -> Result<SessionMeta, SessionError> {
        Err(SessionError::InvalidState("SqliteEventStore::create_session not implemented yet".into()))
    }

    async fn load_session_meta(&self, _id: SessionId) -> Result<SessionMeta, SessionError> {
        Err(SessionError::InvalidState("SqliteEventStore::load_session_meta not implemented yet".into()))
    }

    async fn append(&self, _session_id: SessionId, _payload: JsonValue) -> Result<SessionEvent, SessionError> {
        Err(SessionError::InvalidState("SqliteEventStore::append not implemented yet".into()))
    }

    async fn replay(&self, _session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError> {
        Err(SessionError::InvalidState("SqliteEventStore::replay not implemented yet".into()))
    }

    async fn update_status(&self, _id: SessionId, _new_status: &str) -> Result<Option<SessionMeta>, SessionError> {
        Err(SessionError::InvalidState("SqliteEventStore::update_status not implemented yet".into()))
    }

    async fn migrate(&self) -> Result<(), SessionError> {
        sqlx::migrate!("./migrations-sqlite")
            .run(&self.pool)
            .await
            .map_err(|e| SessionError::Database(format!("sqlite migrate: {e}").into()))
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}
```

- [ ] **Step 2: Wire into lib.rs (gated)**

Add to `crates/stepyard-session/src/lib.rs`:

```rust
#[cfg(feature = "sqlite")]
mod sqlite_store;
#[cfg(feature = "sqlite")]
pub use sqlite_store::SqliteEventStore;
```

- [ ] **Step 3: Verify sqlite lane compiles**

Run: `cargo check -p stepyard-session --features sqlite --no-default-features 2>&1 | tail -10`
Expected: success.

- [ ] **Step 4: Commit**

```bash
git add crates/stepyard-session/src/sqlite_store.rs crates/stepyard-session/src/lib.rs
git commit -m "feat(stepyard-session): scaffold SqliteEventStore stub (migrate impl-only)"
```

---

## Task 12: Implement SqliteEventStore::create_session (TDD)

**Files:**
- Create: `crates/stepyard-session/tests/sqlite_store_test.rs`
- Modify: `crates/stepyard-session/src/sqlite_store.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/stepyard-session/tests/sqlite_store_test.rs`:

```rust
#![cfg(feature = "sqlite")]

use std::sync::Arc;

use sqlx::sqlite::SqlitePoolOptions;
use stepyard_session::{EventStore, SqliteEventStore, SessionId};
use uuid::Uuid;

async fn store() -> SqliteEventStore {
    // In-memory SQLite per-test for isolation.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect sqlite memory");
    let store = SqliteEventStore::new(pool);
    store.migrate().await.expect("migrate");
    store
}

#[tokio::test]
async fn create_session_returns_running_meta() {
    let s = store().await;
    let id = SessionId::new();
    let meta = s
        .create_session(id, Uuid::new_v4(), "edenred")
        .await
        .expect("create");
    assert_eq!(meta.status, "running");
    assert!(meta.ended_at.is_none());
    assert_eq!(meta.tenant_id, "edenred");
}
```

- [ ] **Step 2: Run, see fail**

Run: `cargo test -p stepyard-session --test sqlite_store_test --features sqlite --no-default-features 2>&1 | tail -10`
Expected: panic at `expect("create")` because the stub returns `InvalidState`.

- [ ] **Step 3: Implement create_session**

Replace the stub body in `crates/stepyard-session/src/sqlite_store.rs`:

```rust
    async fn create_session(
        &self,
        id: SessionId,
        workflow_id: Uuid,
        tenant_id: &str,
    ) -> Result<SessionMeta, SessionError> {
        let now = Utc::now();
        sqlx::query(
            r#"
            INSERT INTO sessions (id, workflow_id, tenant_id, status, started_at)
            VALUES (?1, ?2, ?3, 'running', ?4)
            "#,
        )
        .bind(id.as_uuid().to_string())
        .bind(workflow_id.to_string())
        .bind(tenant_id)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(SessionMeta {
            id,
            workflow_id,
            tenant_id: tenant_id.to_owned(),
            status: "running".to_string(),
            started_at: now,
            ended_at: None,
        })
    }
```

- [ ] **Step 4: Run, see pass**

Run: `cargo test -p stepyard-session --test sqlite_store_test create_session_returns_running_meta --features sqlite --no-default-features 2>&1 | tail -5`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add -u crates/stepyard-session/
git commit -m "feat(sqlite-store): implement create_session"
```

---

## Task 13: Implement SqliteEventStore::append (TDD)

**Files:**
- Modify: `crates/stepyard-session/tests/sqlite_store_test.rs`
- Modify: `crates/stepyard-session/src/sqlite_store.rs`

- [ ] **Step 1: Add failing test**

Append to `tests/sqlite_store_test.rs`:

```rust
use serde_json::json;

#[tokio::test]
async fn append_assigns_monotonic_seq() {
    let s = store().await;
    let id = SessionId::new();
    s.create_session(id, Uuid::new_v4(), "edenred").await.expect("create");

    let e1 = s.append(id, json!({"event": "a"})).await.expect("append 1");
    let e2 = s.append(id, json!({"event": "b"})).await.expect("append 2");
    let e3 = s.append(id, json!({"event": "c"})).await.expect("append 3");

    assert_eq!(e1.seq, 1);
    assert_eq!(e2.seq, 2);
    assert_eq!(e3.seq, 3);
}
```

- [ ] **Step 2: Run, see fail**

Run: `cargo test -p stepyard-session --test sqlite_store_test append_assigns_monotonic_seq --features sqlite --no-default-features 2>&1 | tail -5`
Expected: panic.

- [ ] **Step 3: Implement append (with BEGIN IMMEDIATE for serialization)**

Replace the stub body:

```rust
    async fn append(
        &self,
        session_id: SessionId,
        payload: JsonValue,
    ) -> Result<SessionEvent, SessionError> {
        let mut tx = self.pool.begin().await?;
        // Note: sqlx opens transactions as DEFERRED by default. We need
        // IMMEDIATE for write-serialization.  sqlx 0.8 doesn't expose a
        // direct flag, so we ensure write-lock acquisition by issuing a
        // dummy write at the start of the transaction.
        sqlx::query("UPDATE sessions SET status = status WHERE id = ?1")
            .bind(session_id.as_uuid().to_string())
            .execute(&mut *tx)
            .await?;

        let max_seq: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(seq) FROM session_events WHERE session_id = ?1",
        )
        .bind(session_id.as_uuid().to_string())
        .fetch_one(&mut *tx)
        .await?;

        let seq = max_seq.unwrap_or(0) + 1;
        let event_id = Uuid::new_v4();
        let now = Utc::now();
        let payload_json = serde_json::to_string(&payload)
            .map_err(|e| SessionError::Payload(format!("serialize payload: {e}").into()))?;

        sqlx::query(
            r#"
            INSERT INTO session_events (id, session_id, seq, created_at, payload)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(event_id.to_string())
        .bind(session_id.as_uuid().to_string())
        .bind(seq)
        .bind(now.to_rfc3339())
        .bind(&payload_json)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(SessionEvent {
            id: event_id,
            session_id,
            seq,
            created_at: now,
            payload,
        })
    }
```

- [ ] **Step 4: Run, see pass**

Run: `cargo test -p stepyard-session --test sqlite_store_test append_assigns_monotonic_seq --features sqlite --no-default-features 2>&1 | tail -5`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add -u crates/stepyard-session/
git commit -m "feat(sqlite-store): implement append with monotonic seq"
```

---

## Task 14: Implement SqliteEventStore::replay (TDD)

**Files:**
- Modify: `tests/sqlite_store_test.rs`, `src/sqlite_store.rs`

- [ ] **Step 1: Add failing test**

```rust
#[tokio::test]
async fn replay_returns_events_in_seq_order() {
    let s = store().await;
    let id = SessionId::new();
    s.create_session(id, Uuid::new_v4(), "edenred").await.unwrap();
    s.append(id, json!({"e": 1})).await.unwrap();
    s.append(id, json!({"e": 2})).await.unwrap();

    let events = s.replay(id).await.expect("replay");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[0].payload, json!({"e": 1}));
}
```

- [ ] **Step 2: Run, see fail.** (Expect panic; stub returns InvalidState.)

- [ ] **Step 3: Implement**

Replace the stub:

```rust
    async fn replay(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError> {
        let rows: Vec<(String, String, i64, String, String)> = sqlx::query_as(
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

        rows.into_iter()
            .map(|(id_s, sid_s, seq, created_at_s, payload_s)| {
                let id = Uuid::parse_str(&id_s)
                    .map_err(|e| SessionError::InvalidState(format!("bad event id: {e}")))?;
                let sid = Uuid::parse_str(&sid_s)
                    .map_err(|e| SessionError::InvalidState(format!("bad session id: {e}")))?;
                let created_at = DateTime::parse_from_rfc3339(&created_at_s)
                    .map_err(|e| SessionError::InvalidState(format!("bad created_at: {e}")))?
                    .with_timezone(&Utc);
                let payload: JsonValue = serde_json::from_str(&payload_s)
                    .map_err(|e| SessionError::Payload(format!("decode payload: {e}").into()))?;
                Ok(SessionEvent {
                    id,
                    session_id: SessionId(sid),
                    seq,
                    created_at,
                    payload,
                })
            })
            .collect()
    }
```

- [ ] **Step 4: Run, see pass.** Expected: 1 passed.

- [ ] **Step 5: Commit.**

```bash
git add -u && git commit -m "feat(sqlite-store): implement replay in seq order"
```

---

## Task 15: Implement load_session_meta + update_status (TDD)

**Files:**
- Modify: `tests/sqlite_store_test.rs`, `src/sqlite_store.rs`

- [ ] **Step 1: Add failing tests**

```rust
#[tokio::test]
async fn load_session_meta_round_trip() {
    let s = store().await;
    let id = SessionId::new();
    let wf = Uuid::new_v4();
    s.create_session(id, wf, "afya").await.unwrap();
    let meta = s.load_session_meta(id).await.expect("load");
    assert_eq!(meta.workflow_id, wf);
    assert_eq!(meta.tenant_id, "afya");
    assert_eq!(meta.status, "running");
}

#[tokio::test]
async fn update_status_transitions_running_to_completed() {
    let s = store().await;
    let id = SessionId::new();
    s.create_session(id, Uuid::new_v4(), "afya").await.unwrap();
    let meta = s.update_status(id, "completed").await.expect("update").expect("row");
    assert_eq!(meta.status, "completed");
    assert!(meta.ended_at.is_some());
}

#[tokio::test]
async fn update_status_is_noop_if_terminal() {
    let s = store().await;
    let id = SessionId::new();
    s.create_session(id, Uuid::new_v4(), "afya").await.unwrap();
    let _ = s.update_status(id, "completed").await.expect("first").expect("row");
    let second = s.update_status(id, "failed").await.expect("second");
    assert!(second.is_none(), "second update on terminal should be no-op");
}
```

- [ ] **Step 2: Run, see fail.**

- [ ] **Step 3: Implement load_session_meta**

```rust
    async fn load_session_meta(&self, id: SessionId) -> Result<SessionMeta, SessionError> {
        let row: Option<(String, String, String, String, String, Option<String>)> =
            sqlx::query_as(
                r#"
                SELECT id, workflow_id, tenant_id, status, started_at, ended_at
                FROM sessions
                WHERE id = ?1
                "#,
            )
            .bind(id.as_uuid().to_string())
            .fetch_optional(&self.pool)
            .await?;

        let (id_s, wf_s, tenant, status, started_s, ended_s) =
            row.ok_or(SessionError::NotFound(id))?;

        let started_at = DateTime::parse_from_rfc3339(&started_s)
            .map_err(|e| SessionError::InvalidState(format!("bad started_at: {e}")))?
            .with_timezone(&Utc);
        let ended_at = match ended_s {
            None => None,
            Some(s) => Some(
                DateTime::parse_from_rfc3339(&s)
                    .map_err(|e| SessionError::InvalidState(format!("bad ended_at: {e}")))?
                    .with_timezone(&Utc),
            ),
        };

        Ok(SessionMeta {
            id: SessionId(Uuid::parse_str(&id_s).map_err(|e| {
                SessionError::InvalidState(format!("bad session id: {e}"))
            })?),
            workflow_id: Uuid::parse_str(&wf_s).map_err(|e| {
                SessionError::InvalidState(format!("bad workflow id: {e}"))
            })?,
            tenant_id: tenant,
            status,
            started_at,
            ended_at,
        })
    }
```

- [ ] **Step 4: Implement update_status**

```rust
    async fn update_status(
        &self,
        id: SessionId,
        new_status: &str,
    ) -> Result<Option<SessionMeta>, SessionError> {
        let now = Utc::now();
        let rows = sqlx::query(
            r#"
            UPDATE sessions
            SET status = ?2, ended_at = ?3
            WHERE id = ?1 AND status = 'running'
            "#,
        )
        .bind(id.as_uuid().to_string())
        .bind(new_status)
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await?;

        if rows.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(Some(self.load_session_meta(id).await?))
    }
```

- [ ] **Step 5: Run all sqlite tests, see pass.**

Run: `cargo test -p stepyard-session --test sqlite_store_test --features sqlite --no-default-features 2>&1 | tail -5`
Expected: all tests pass (5 total at this point).

- [ ] **Step 6: Commit.**

```bash
git add -u && git commit -m "feat(sqlite-store): implement load_session_meta + update_status"
```

---

## Task 16: CI matrix lane

**Files:**
- Modify: `.github/workflows/check.yml`

- [ ] **Step 1: Read current check.yml**

Run: `cat .github/workflows/check.yml`
Expected: see one job `check` running build/test/clippy/audits.

- [ ] **Step 2: Convert to matrix**

Modify the job:

```yaml
jobs:
  check:
    name: "Build, test, clippy, audits (${{ matrix.backend }})"
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        backend: [postgres, sqlite]
    services:
      postgres:
        # Only the postgres lane needs the service container; SQLite uses
        # the in-memory or file backend with no external dependency.
        image: ${{ matrix.backend == 'postgres' && 'postgres:16' || '' }}
        env:
          POSTGRES_DB: minion_engine
          POSTGRES_USER: minion
          POSTGRES_PASSWORD: minion_secret
        ports:
          - 5433:5432
        options: --health-cmd pg_isready --health-interval 10s

    env:
      STEPYARD_HARNESS_DATABASE_URL: ${{ matrix.backend == 'postgres' && 'postgres://minion:minion_secret@localhost:5433/minion_engine' || '' }}

    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2

      - name: Build
        run: cargo build --workspace --no-default-features --features ${{ matrix.backend }}

      - name: Test
        run: cargo test --workspace --no-default-features --features ${{ matrix.backend }}

      - name: Clippy
        run: cargo clippy --workspace --no-default-features --features ${{ matrix.backend }} --all-targets -- -D warnings

      - name: Audit patterns
        run: bash scripts/audit-patterns.sh

      - name: Audit emit-before-io
        run: cargo run -p xtask --quiet -- audit-emit-before-io
```

- [ ] **Step 3: Update branch protection note**

Add a TODO line at the top of the file:

```yaml
# After this PR merges, update branch protection on `main` to require
# both "Build, test, clippy, audits (postgres)" and
# "Build, test, clippy, audits (sqlite)" status checks.
```

(The repository setting itself is updated manually by an admin once the new check names land in a CI run.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/check.yml
git commit -m "ci: add backend matrix lane (postgres + sqlite)"
```

---

## Task 17: Replay parity test

**Files:**
- Create: `crates/stepyard-harness/tests/replay_parity.rs`
- Modify: `crates/stepyard-harness/Cargo.toml` — add `parity-test` feature.

- [ ] **Step 1: Add parity-test feature**

In `crates/stepyard-harness/Cargo.toml`:

```toml
[features]
parity-test = ["stepyard-session/postgres", "stepyard-session/sqlite"]
```

This is the only place where both backends are intentionally co-enabled. The compile_error guards in stepyard-session must be relaxed for this case OR this test crate must fork its own session impl. Easiest path: gate the parity-test off the harness crate (not stepyard-session) and use raw sqlx APIs inside the test instead of going through the EventStore trait. Re-evaluate before implementing.

Actually a cleaner approach: drop the dual-build idea, and instead make the parity test a SHELL test that runs the same workflow twice via `cargo test --features postgres` then `cargo test --features sqlite`, captures the event sequences as JSONL, and diffs them. Move the parity check to a new `scripts/replay-parity.sh`. This avoids the mutually-exclusive features problem.

- [ ] **Step 2: Create scripts/replay-parity.sh**

```bash
#!/usr/bin/env bash
# Compares event sequences emitted by the same workflow against both
# postgres and sqlite backends. Used as a manual local check + CI gate.
set -euo pipefail

WORKFLOW="${1:-tests/fixtures/parity_workflow.yaml}"

# 1. Run on postgres, dump events.
STEPYARD_HARNESS_DATABASE_URL="postgres://minion:minion_secret@localhost:5433/minion_engine" \
    cargo run --quiet --no-default-features --features postgres -- run "$WORKFLOW" --dump-events /tmp/pg-events.jsonl

# 2. Run on sqlite, dump events.
rm -f /tmp/sqlite-parity.db
STEPYARD_HARNESS_DATABASE_URL="sqlite:///tmp/sqlite-parity.db?mode=rwc" \
    cargo run --quiet --no-default-features --features sqlite -- run "$WORKFLOW" --dump-events /tmp/sqlite-events.jsonl

# 3. Normalize and diff.
python3 scripts/normalize-events.py /tmp/pg-events.jsonl > /tmp/pg-norm.jsonl
python3 scripts/normalize-events.py /tmp/sqlite-events.jsonl > /tmp/sqlite-norm.jsonl

if diff -u /tmp/pg-norm.jsonl /tmp/sqlite-norm.jsonl; then
    echo "✅ replay parity: events match across backends"
else
    echo "❌ replay parity: events DIVERGE — see diff above"
    exit 1
fi
```

- [ ] **Step 3: Create scripts/normalize-events.py**

```python
#!/usr/bin/env python3
"""Normalize a JSONL event dump for cross-backend parity comparison.

Strips timestamps + per-event UUIDs; recursively sorts JSON keys."""
import json, sys, re

def normalize(obj):
    if isinstance(obj, dict):
        return {k: normalize(v) for k, v in sorted(obj.items()) if k not in {"id", "created_at"}}
    if isinstance(obj, list):
        return [normalize(x) for x in obj]
    if isinstance(obj, str) and re.match(r'^\d{4}-\d{2}-\d{2}T', obj):
        return "<TIMESTAMP>"
    return obj

for line in sys.stdin if len(sys.argv) < 2 else open(sys.argv[1]):
    line = line.strip()
    if not line: continue
    obj = json.loads(line)
    print(json.dumps(normalize(obj), separators=(",", ":")))
```

- [ ] **Step 4: Add a `--dump-events` CLI flag**

This is a separate small addition to `src/main.rs` / `src/cli/run.rs`. After the run completes, dump `session.replay()` to the given path as JSONL. Implementation is mechanical — one query, write loop.

- [ ] **Step 5: Author tests/fixtures/parity_workflow.yaml**

```yaml
name: parity-smoke
steps:
  - type: cmd
    name: a
    command: echo "hello"
  - type: cmd
    name: b
    command: echo "world"
  - type: parallel
    name: p
    steps:
      - type: cmd
        name: x
        command: echo "x"
      - type: cmd
        name: y
        command: echo "y"
```

- [ ] **Step 6: Run the parity script locally**

Run: `bash scripts/replay-parity.sh`
Expected: `✅ replay parity: events match across backends`.

- [ ] **Step 7: Wire into CI**

Add a job to `.github/workflows/check.yml`:

```yaml
  parity:
    name: "Replay parity"
    runs-on: ubuntu-latest
    needs: check
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_DB: minion_engine
          POSTGRES_USER: minion
          POSTGRES_PASSWORD: minion_secret
        ports:
          - 5433:5432
        options: --health-cmd pg_isready --health-interval 10s
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: bash scripts/replay-parity.sh
```

- [ ] **Step 8: Commit**

```bash
git add scripts/replay-parity.sh scripts/normalize-events.py tests/fixtures/parity_workflow.yaml .github/workflows/check.yml
git commit -m "ci+test: add replay parity check across postgres/sqlite backends"
```

---

## Task 18: End-to-end smoke + PR

**Files:** none — verification only.

- [ ] **Step 1: Lint sweep**

Run: `cargo clippy --workspace --no-default-features --features postgres --all-targets -- -D warnings 2>&1 | tail -10`
Expected: success (1 baseline warning).

Run: `cargo clippy --workspace --no-default-features --features sqlite --all-targets -- -D warnings 2>&1 | tail -10`
Expected: success (1 baseline warning).

- [ ] **Step 2: Audit patterns**

Run: `bash scripts/audit-patterns.sh`
Expected: blocking gates clean, G5 warnings unchanged.

- [ ] **Step 3: Audit emit-before-io**

Run: `cargo run -p xtask --quiet -- audit-emit-before-io 2>&1 | tail -5`
Expected: 3 findings (baseline unchanged).

- [ ] **Step 4: Run both lanes**

Run:
```bash
STEPYARD_HARNESS_DATABASE_URL='postgres://minion:minion_secret@localhost:5433/minion_engine' \
  cargo test --workspace --no-default-features --features postgres
```
Expected: same baseline as before (2 pre-existing failures only).

Run: `cargo test --workspace --no-default-features --features sqlite`
Expected: all sqlite tests pass; harness tests gracefully skip if they need postgres-specific fixtures (they should use STEPYARD_HARNESS_DATABASE_URL with sqlite:// in this lane).

- [ ] **Step 5: Push branch and open PR**

```bash
git push -u origin feat/pr-a1-event-store-trait
gh pr create --title "feat(stepyard-session): EventStore trait + SQLite backend (Lite PR A1)" --body "$(cat <<'EOF'
## Summary
First of three PRs implementing Stepyard Lite (see docs/superpowers/specs/2026-05-01-stepyard-lite-design.md). Introduces backend-agnostic EventStore trait, refactors existing Postgres path into PgEventStore, adds new SqliteEventStore for local/lite use. Mutually-exclusive Cargo features `postgres` (default) and `sqlite`.

## Changes
- New `EventStore` trait in `stepyard-session::store_trait`
- `PgEventStore` extracts current Session SQL (zero behavior change in postgres lane)
- `SqliteEventStore` new, with mirror schema in `migrations-sqlite/`
- `Session` struct refactored to hold `Arc<dyn EventStore>`
- `build_store_from_env()` factory dispatches on Cargo feature
- All call sites updated (~13 test files + src/main.rs)
- CI gains backend matrix lane
- Replay parity check via shell script + python normalizer

## Test plan
- [x] `cargo test --features postgres --no-default-features` — pre-existing baseline failures only
- [x] `cargo test --features sqlite --no-default-features` — all sqlite tests pass
- [x] `cargo clippy` clean on both lanes
- [x] `bash scripts/audit-patterns.sh` clean
- [x] `cargo run -p xtask -- audit-emit-before-io` baseline unchanged (3)
- [x] `bash scripts/replay-parity.sh` ✅

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 6: Wait for CI, address feedback, merge.**

---

## Self-Review Checklist (run before declaring plan complete)

- [x] Spec coverage: every §4–§5 spec requirement maps to a task above? Yes — trait (4, 5), pg refactor (5, 6, 7), sqlite impl (11–15), migrations split (9, 10), CI matrix (16), parity test (17).
- [x] Placeholder scan: no "TBD" / "TODO" / "fill in details" in steps.
- [x] Type consistency: `EventStore`, `SessionMeta`, `Arc<dyn EventStore>`, `SessionId`, `SessionEvent` used consistently.
- [x] All file paths absolute or workspace-relative — no ambiguity.
- [x] Each step has expected output where applicable.

## Open issues to flag during PR review (not blockers)

- The `update_status` impl in PgEventStore changed signature compared to the original `finish()` method — instead of returning `(String, Option<DateTime<Utc>>)`, it now returns the full `SessionMeta`. Verify no caller relies on the narrower return type. Search: `grep -rn "complete()\|fail()\|cancel()" src/ crates/`.
- `STEPYARD_HARNESS_DATABASE_URL` env var is reused for both backends. The existing variable name has "POSTGRES" connotation but the test fixtures already pivot off it. Document this in README under "Lite mode setup".
- The `parity-test` feature dropped in favor of a shell script avoids the mutually-exclusive-features problem cleanly. Double-check this is acceptable to the reviewer; the alternative would be a third "parity" workspace member that depends on neither default but pulls both via path deps with renamed crate aliases — significantly more machinery.
