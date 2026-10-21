# Stepyard Lite PR A3 — File Log Mirror Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Status:** Implemented on `main` via PR #61 (`640757c`) and hardened by PR #64 (`678ef31`) and PR #66 (`04280fe`). The task checkboxes below are preserved as the original execution plan, not as current open work.

**Goal:** Mirror every appended `SessionEvent` to a session-scoped JSONL file at `.stepyard/logs/<session_id>.jsonl`, append-only, always on with opt-out (`--no-file-logs` / `STEPYARD_NO_FILE_LOGS=1`). Failure to write the file degrades non-silently (one `Event::FileLogWriteFailed` is emitted to the underlying event store, the per-session mirror is then disabled), but never fails the originating step.

**Architecture:** A `FileLogMirror` decorator that wraps `Arc<dyn EventStore>` and impls `EventStore` itself. In its `append`, it (1) delegates to the inner store, (2) on success serializes the resulting `SessionEvent` as a single JSON line and appends to `.stepyard/logs/<session_id>.jsonl`. A per-session "broken" flag (set after the first file IO failure) short-circuits further writes for that session. The factory from PR A1 (`build_store_from_env`) is extended to optionally wrap its return value with this decorator.

**Tech Stack:** Rust 2021, async-trait 0.1, tokio (`fs::OpenOptions::append(true)`), serde_json, dashmap (or `Mutex<HashMap>`), thiserror.

**Spec reference:** `docs/superpowers/specs/2026-05-01-stepyard-lite-design.md` §7.

**Prerequisites:**
- PR A1 merged: `EventStore` trait with `append`, `replay`, `lock_session`, etc.
- PR A2 may or may not have landed; A3 is independent of A2.

---

## File Structure

**`crates/stepyard-core/`:**
- Modify: `src/event.rs` — add `Event::FileLogWriteFailed` variant.

**`crates/stepyard-session/`:**
- Create: `src/file_log_mirror.rs` — `FileLogMirror` decorator + per-session state + tests.
- Modify: `src/lib.rs` — public re-export of `FileLogMirror` and the `FileLogConfig` config struct.
- Modify: `src/factory.rs` — extend `build_store_from_env` (or add a sibling `build_store_with_logs`) to optionally wrap with `FileLogMirror`.
- Modify: `Cargo.toml` — add `dashmap` (or use std `Mutex<HashMap>`; pick in Task 3).

**Binary CLI:**
- Modify: `src/cli/commands.rs` — `--no-file-logs` flag on `ExecuteArgs`; honored when constructing the store.
- Modify: `src/cli/mod.rs` — help banner mention.

**Tests:**
- Create: `crates/stepyard-session/tests/file_log_mirror.rs` — integration tests covering happy path, opt-out, and write-failure-degrades-gracefully.

**Docs:**
- Modify: `README.md` — short paragraph about `.stepyard/logs/`.

---

## Task 1: Branch + baseline gates

**Files:** none

- [ ] **Step 1: Branch off latest main**

```bash
git fetch origin main
git checkout -b feat/pr-a3-file-log-mirror origin/main
```
Expected: `Switched to a new branch 'feat/pr-a3-file-log-mirror'`.

- [ ] **Step 2: Confirm A1 has landed**

Run: `git log --oneline -10 | grep -iE "EventStore|stepyard.lite|sqlite"`
Expected: A1 commit visible. If not, STOP — A3 depends on the `EventStore` trait shape introduced in A1.

- [ ] **Step 3: Capture clippy baseline**

```bash
cargo clippy --workspace --all-targets --no-default-features --features sqlite -- -D warnings 2>&1 | tail -5
cargo clippy --workspace --all-targets --no-default-features --features postgres -- -D warnings 2>&1 | tail -5
```
Expected: both clean.

---

## Task 2: Add `Event::FileLogWriteFailed` variant (TDD)

**Files:**
- Modify: `crates/stepyard-core/src/event.rs`

The variant carries an error class string (not the raw `io::Error` — that's not `Serialize`) and a session-scoped marker.

- [ ] **Step 1: Write a failing serde round-trip test**

In `crates/stepyard-core/src/event.rs`, locate the existing `#[cfg(test)] mod tests` block (search: `grep -n '#\[cfg(test)\]' crates/stepyard-core/src/event.rs`). If it doesn't exist, add it at the bottom of the file. Append the following test:

```rust
    #[test]
    fn file_log_write_failed_round_trips() {
        use chrono::TimeZone;
        let original = Event::FileLogWriteFailed {
            error_class: "permission_denied".to_string(),
            timestamp: chrono::Utc.with_ymd_and_hms(2026, 5, 1, 12, 0, 0).unwrap(),
        };
        let encoded = serde_json::to_string(&original).unwrap();
        // Sanity: discriminator tag + snake_case naming.
        assert!(
            encoded.contains(r#""event":"file_log_write_failed""#),
            "got: {encoded}"
        );
        let decoded: Event = serde_json::from_str(&encoded).unwrap();
        match decoded {
            Event::FileLogWriteFailed { error_class, .. } => {
                assert_eq!(error_class, "permission_denied");
            }
            other => panic!("expected FileLogWriteFailed, got {other:?}"),
        }
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p stepyard-core file_log_write_failed_round_trips 2>&1 | tail -8`
Expected: compile error — `no variant or associated item named FileLogWriteFailed`.

- [ ] **Step 3: Add the variant**

In the same file, append a new variant inside the `Event` enum (immediately before the closing `}` of the enum, after `ChatMessageAppended`):

```rust
    /// The file-log mirror writer failed to append to
    /// `.stepyard/logs/<session_id>.jsonl`. Emitted exactly once per session
    /// to the underlying event store, after which the mirror is disabled for
    /// that session for the lifetime of the process. The originating step is
    /// **not** failed; the event store remains the source of truth and the
    /// degradation is visible (not silent) per the silent-failure-hunter
    /// convention.
    ///
    /// `error_class` is a stable, low-cardinality label
    /// (`permission_denied`, `disk_full`, `io_other`) — never the raw
    /// `io::Error` `Display`, which can leak host paths.
    FileLogWriteFailed {
        error_class: String,
        timestamp: DateTime<Utc>,
    },
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p stepyard-core file_log_write_failed_round_trips 2>&1 | tail -5`
Expected: 1 passed.

Run: `cargo test -p stepyard-core 2>&1 | tail -5`
Expected: full suite green.

- [ ] **Step 5: Commit**

```bash
git add crates/stepyard-core/src/event.rs
git commit -m "feat(core): add Event::FileLogWriteFailed for visible file-log degradation"
```

---

## Task 3: Define `FileLogMirror` skeleton + `FileLogConfig`

**Files:**
- Create: `crates/stepyard-session/src/file_log_mirror.rs`
- Modify: `crates/stepyard-session/src/lib.rs`
- Modify: `crates/stepyard-session/Cargo.toml` (only if `tokio::fs` not yet a feature on the existing `tokio` dep)

- [ ] **Step 1: Read existing `tokio` dep on stepyard-session**

Run: `grep -A1 '^tokio' crates/stepyard-session/Cargo.toml`
Expected: see `tokio = { version = "1", features = [...] }`. Verify `fs` is in the feature list. If missing, add `"fs"` to the feature array.

- [ ] **Step 2: Create the file with public surface only (no impl yet)**

Create `crates/stepyard-session/src/file_log_mirror.rs`:

```rust
//! `FileLogMirror` — decorator over `EventStore` that double-writes every
//! appended `SessionEvent` to `.stepyard/logs/<session_id>.jsonl`.
//!
//! See spec `docs/superpowers/specs/2026-05-01-stepyard-lite-design.md` §7
//! for the design rationale, including the non-silent failure mode.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use stepyard_core::Event;
use tokio::sync::Mutex;

use crate::store::{SessionError, SessionEvent, SessionId};
use crate::store_trait::{EventStore, SessionMeta};
use crate::session::SessionStatus;

/// Configuration for the file log mirror.
///
/// `directory` is the base dir; per-session files are `directory/<id>.jsonl`.
/// `enabled` is the global on/off — when `false`, [`build_store_with_logs`]
/// returns the inner store unwrapped (no overhead).
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

/// Per-session state for the mirror. After the first IO failure the session
/// is marked `broken: true` and subsequent appends short-circuit to the inner
/// store only.
#[derive(Debug)]
struct MirrorState {
    broken: bool,
}

/// `EventStore` decorator that double-writes appends to a JSONL file.
pub struct FileLogMirror {
    inner: Arc<dyn EventStore>,
    directory: PathBuf,
    sessions: Mutex<HashMap<SessionId, MirrorState>>,
}

impl FileLogMirror {
    pub fn new(inner: Arc<dyn EventStore>, directory: PathBuf) -> Self {
        Self {
            inner,
            directory,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn jsonl_path(&self, session_id: &SessionId) -> PathBuf {
        self.directory.join(format!("{}.jsonl", session_id.as_uuid()))
    }
}
```

- [ ] **Step 3: Wire the module into `lib.rs`**

Open `crates/stepyard-session/src/lib.rs`. After the existing module declarations, add:

```rust
pub mod file_log_mirror;
pub use file_log_mirror::{FileLogConfig, FileLogMirror};
```

- [ ] **Step 4: Confirm it compiles**

Run: `cargo check -p stepyard-session --no-default-features --features sqlite 2>&1 | tail -10`
Expected: `Finished` (allowed: dead-code warnings since we have no impl yet).

Run: `cargo check -p stepyard-session --no-default-features --features postgres 2>&1 | tail -10`
Expected: same.

- [ ] **Step 5: Commit**

```bash
git add crates/stepyard-session/src/file_log_mirror.rs crates/stepyard-session/src/lib.rs crates/stepyard-session/Cargo.toml
git commit -m "feat(session): add FileLogMirror skeleton + FileLogConfig"
```

---

## Task 4: Implement `EventStore for FileLogMirror::append` happy path (TDD)

**Files:**
- Modify: `crates/stepyard-session/src/file_log_mirror.rs`
- Create: `crates/stepyard-session/tests/file_log_mirror.rs`

The happy path: `append` succeeds at the inner store, writes one JSONL line to `<dir>/<session_id>.jsonl`, returns the `SessionEvent` unchanged.

- [ ] **Step 1: Write the failing integration test (happy path only)**

Create `crates/stepyard-session/tests/file_log_mirror.rs`:

```rust
//! Integration tests for `FileLogMirror`.
//!
//! Uses an in-memory SQLite store as the inner backend so the test does not
//! require Postgres. The choice of `sqlite` here also means these tests run
//! in the SQLite CI lane.

#![cfg(feature = "sqlite")]

use std::sync::Arc;

use serde_json::json;
use stepyard_session::{
    factory::build_store_for_test_sqlite_in_memory,
    FileLogMirror, FileLogConfig,
};
use stepyard_session::store_trait::EventStore;
use stepyard_session::store::SessionId;
use tempfile::TempDir;

async fn fresh_inner() -> Arc<dyn EventStore> {
    build_store_for_test_sqlite_in_memory()
        .await
        .expect("failed to build in-memory sqlite store")
}

#[tokio::test]
async fn append_writes_event_to_jsonl_file() {
    let inner = fresh_inner().await;
    let dir = TempDir::new().unwrap();
    let mirror = FileLogMirror::new(inner.clone(), dir.path().to_path_buf());

    let session_id = SessionId::new();
    inner
        .create_session(session_id, "test_workflow".to_string())
        .await
        .unwrap();

    let payload = json!({"event": "workflow_started", "timestamp": "2026-05-01T12:00:00Z"});
    let stored = mirror.append(session_id, payload.clone()).await.unwrap();
    assert_eq!(stored.session_id, session_id);

    let path = dir.path().join(format!("{}.jsonl", session_id.as_uuid()));
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("expected file at {}: {e}", path.display()));

    let lines: Vec<_> = body.lines().collect();
    assert_eq!(lines.len(), 1, "expected exactly one line, got {body:?}");

    let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(parsed.get("seq").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(
        parsed.get("payload").unwrap(),
        &payload,
        "payload should round-trip verbatim"
    );
}
```

Note: `build_store_for_test_sqlite_in_memory` is a test helper introduced in PR A1 (Task 8 of that plan). If A1's helper has a different name, substitute it; the function is whatever opens an in-memory `SqliteEventStore` for tests.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p stepyard-session --no-default-features --features sqlite --test file_log_mirror append_writes_event_to_jsonl_file 2>&1 | tail -15`
Expected: compile error — `FileLogMirror::append` not yet implemented.

- [ ] **Step 3: Implement `EventStore for FileLogMirror`**

Append to `crates/stepyard-session/src/file_log_mirror.rs`:

```rust
impl FileLogMirror {
    async fn write_jsonl_line(
        &self,
        session_id: &SessionId,
        event: &SessionEvent,
    ) -> Result<(), std::io::Error> {
        use tokio::io::AsyncWriteExt;
        tokio::fs::create_dir_all(&self.directory).await?;
        let path = self.jsonl_path(session_id);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let mut line = serde_json::to_vec(event).map_err(std::io::Error::other)?;
        line.push(b'\n');
        file.write_all(&line).await?;
        file.flush().await?;
        Ok(())
    }

    fn classify_io_error(err: &std::io::Error) -> &'static str {
        use std::io::ErrorKind::*;
        match err.kind() {
            PermissionDenied => "permission_denied",
            // Stable name for "no space left on device". The kind exists on
            // recent Rust; if the toolchain is older than 1.83 we fall back
            // to the generic label.
            kind if format!("{kind:?}") == "StorageFull" => "disk_full",
            _ => "io_other",
        }
    }
}

#[async_trait]
impl EventStore for FileLogMirror {
    async fn append(
        &self,
        session_id: SessionId,
        event: serde_json::Value,
    ) -> Result<SessionEvent, SessionError> {
        let stored = self.inner.append(session_id, event).await?;

        // Acquire a per-session lock for the file write to keep the JSONL
        // append-order matching the DB seq order.
        let mut sessions = self.sessions.lock().await;
        let state = sessions
            .entry(session_id)
            .or_insert(MirrorState { broken: false });

        if state.broken {
            return Ok(stored);
        }

        if let Err(err) = self.write_jsonl_line(&session_id, &stored).await {
            let class = Self::classify_io_error(&err).to_string();
            tracing::warn!(
                session_id = %session_id.as_uuid(),
                error = %err,
                class = %class,
                "file log mirror write failed; disabling for this session",
            );
            state.broken = true;
            // Drop the lock before re-entering the inner store so an emit-
            // before-io audit doesn't see a long-held lock during the IO.
            drop(sessions);

            let degradation = serde_json::to_value(Event::FileLogWriteFailed {
                error_class: class,
                timestamp: Utc::now(),
            })
            .map_err(SessionError::Payload)?;
            // Best effort. If the inner store also fails here, fall through
            // — the original step is preserved either way.
            let _ = self.inner.append(session_id, degradation).await;
        }
        Ok(stored)
    }

    async fn replay(&self, session_id: SessionId) -> Result<Vec<SessionEvent>, SessionError> {
        // Replay from the source of truth (event store), never from the file.
        self.inner.replay(session_id).await
    }

    async fn create_session(
        &self,
        id: SessionId,
        kind: String,
    ) -> Result<(SessionId, String, DateTime<Utc>), SessionError> {
        self.inner.create_session(id, kind).await
    }

    async fn load_session_meta(&self, id: SessionId) -> Result<SessionMeta, SessionError> {
        self.inner.load_session_meta(id).await
    }

    async fn update_status(
        &self,
        id: SessionId,
        status: SessionStatus,
        terminated_at: Option<DateTime<Utc>>,
    ) -> Result<(), SessionError> {
        self.inner.update_status(id, status, terminated_at).await
    }

    async fn lock_session(&self, id: SessionId) -> Result<(), SessionError> {
        self.inner.lock_session(id).await
    }
}
```

- [ ] **Step 4: Run the test to confirm it passes**

Run: `cargo test -p stepyard-session --no-default-features --features sqlite --test file_log_mirror append_writes_event_to_jsonl_file 2>&1 | tail -10`
Expected: 1 passed.

- [ ] **Step 5: Run full session crate tests both profiles**

```bash
cargo test -p stepyard-session --no-default-features --features sqlite 2>&1 | tail -8
cargo test -p stepyard-session --no-default-features --features postgres 2>&1 | tail -8
```
Expected: green on sqlite (the new test runs); green on postgres (the new test is `#![cfg(feature = "sqlite")]`-gated out).

- [ ] **Step 6: Commit**

```bash
git add crates/stepyard-session/src/file_log_mirror.rs crates/stepyard-session/tests/file_log_mirror.rs
git commit -m "feat(session): implement FileLogMirror::append happy path with JSONL write"
```

---

## Task 5: Test the write-failure → broken-session degradation (TDD)

**Files:**
- Modify: `crates/stepyard-session/tests/file_log_mirror.rs`

The mirror must (a) emit `Event::FileLogWriteFailed` once, (b) stop writing for that session, (c) NOT fail the originating append, (d) keep working for other sessions.

We trigger a write failure by pointing `directory` at a path that *exists as a regular file* — `create_dir_all` then errors with `NotADirectory`/`AlreadyExists`/`PermissionDenied` depending on platform.

- [ ] **Step 1: Write the failing degradation test**

Append to `crates/stepyard-session/tests/file_log_mirror.rs`:

```rust
#[tokio::test]
async fn write_failure_disables_mirror_for_that_session_only() {
    let inner = fresh_inner().await;
    let tmp = TempDir::new().unwrap();
    // Point directory at a regular file so create_dir_all fails.
    let blocker = tmp.path().join("logs");
    std::fs::write(&blocker, b"i am a file, not a directory").unwrap();

    let mirror = FileLogMirror::new(inner.clone(), blocker.clone());

    let s1 = SessionId::new();
    inner.create_session(s1, "wf1".to_string()).await.unwrap();

    // First append: succeeds at the inner store, file write fails, degradation
    // event is recorded, originating event still returned.
    let payload = json!({"event": "workflow_started", "timestamp": "2026-05-01T12:00:00Z"});
    let stored1 = mirror.append(s1, payload.clone()).await.unwrap();
    assert_eq!(stored1.seq, 1, "originating event still gets seq=1");

    // Second append on same session: still succeeds at inner; mirror is broken
    // → no further degradation events should be emitted.
    let stored2 = mirror.append(s1, json!({"event": "step_started"})).await.unwrap();
    assert_eq!(stored2.seq, 3, "seq advanced past the degradation event at seq=2");

    // Replay should show: original payload (seq=1), FileLogWriteFailed (seq=2),
    // step_started (seq=3). Exactly one degradation event.
    let events = inner.replay(s1).await.unwrap();
    let degradations: Vec<_> = events
        .iter()
        .filter(|e| {
            e.payload
                .get("event")
                .and_then(|v| v.as_str())
                == Some("file_log_write_failed")
        })
        .collect();
    assert_eq!(
        degradations.len(),
        1,
        "exactly one degradation event expected, got {}",
        degradations.len()
    );

    // A *different* session should still get its mirror broken on first failure
    // independently — proves the broken flag is per-session.
    let s2 = SessionId::new();
    inner.create_session(s2, "wf2".to_string()).await.unwrap();
    let _ = mirror.append(s2, json!({"event": "workflow_started"})).await.unwrap();
    let s2_events = inner.replay(s2).await.unwrap();
    let s2_degradations: Vec<_> = s2_events
        .iter()
        .filter(|e| {
            e.payload.get("event").and_then(|v| v.as_str()) == Some("file_log_write_failed")
        })
        .collect();
    assert_eq!(s2_degradations.len(), 1, "session 2 also got its own degradation");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p stepyard-session --no-default-features --features sqlite --test file_log_mirror write_failure_disables_mirror_for_that_session_only 2>&1 | tail -20`
Expected: 1 passed.

If it fails on `assert_eq!(stored2.seq, 3, ...)`: the degradation event got seq=2 (good), the second user event got seq=3 (good). If a different value, inspect `inner.replay(s1)` ordering and adjust expectations. Common gotcha: macOS may not return `NotADirectory` for `create_dir_all` on a file path; if the test passes on Linux but not macOS, switch the trigger to a read-only directory:

```rust
let blocker = tmp.path().join("readonly_logs");
std::fs::create_dir(&blocker).unwrap();
let mut perms = std::fs::metadata(&blocker).unwrap().permissions();
perms.set_readonly(true);
std::fs::set_permissions(&blocker, perms).unwrap();
```

- [ ] **Step 3: Commit**

```bash
git add crates/stepyard-session/tests/file_log_mirror.rs
git commit -m "test(session): assert FileLogMirror non-silent degradation on write failure"
```

---

## Task 6: Test the opt-out + factory wiring (TDD)

**Files:**
- Modify: `crates/stepyard-session/src/factory.rs`
- Modify: `crates/stepyard-session/tests/file_log_mirror.rs`

`FileLogConfig::enabled = false` should produce a store with no mirror at all (zero overhead, no `.stepyard/logs/` directory created).

- [ ] **Step 1: Write the failing factory wrapping test**

Append to `crates/stepyard-session/tests/file_log_mirror.rs`:

```rust
#[tokio::test]
async fn opt_out_returns_inner_store_unwrapped() {
    use stepyard_session::factory::wrap_with_file_logs;

    let inner = fresh_inner().await;
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("should-not-be-created");
    let cfg = FileLogConfig {
        enabled: false,
        directory: dir.clone(),
    };
    let store = wrap_with_file_logs(inner, cfg);

    let s = SessionId::new();
    store.create_session(s, "wf".to_string()).await.unwrap();
    store.append(s, json!({"event": "x"})).await.unwrap();

    assert!(!dir.exists(), "log directory should not be created when opt-out is on");
}

#[tokio::test]
async fn opt_in_wraps_with_mirror_and_creates_dir() {
    use stepyard_session::factory::wrap_with_file_logs;

    let inner = fresh_inner().await;
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("logs");
    let cfg = FileLogConfig {
        enabled: true,
        directory: dir.clone(),
    };
    let store = wrap_with_file_logs(inner, cfg);

    let s = SessionId::new();
    store.create_session(s, "wf".to_string()).await.unwrap();
    store.append(s, json!({"event": "x"})).await.unwrap();

    let path = dir.join(format!("{}.jsonl", s.as_uuid()));
    assert!(path.exists(), "expected JSONL file at {}", path.display());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p stepyard-session --no-default-features --features sqlite --test file_log_mirror opt_ 2>&1 | tail -10`
Expected: compile error — `wrap_with_file_logs` not found in `factory` module.

- [ ] **Step 3: Add `wrap_with_file_logs` to the factory**

Open `crates/stepyard-session/src/factory.rs`. Append:

```rust
use std::sync::Arc;

use crate::file_log_mirror::{FileLogConfig, FileLogMirror};
use crate::store_trait::EventStore;

/// Wrap an event store with a file log mirror per `cfg`.
///
/// When `cfg.enabled == false`, returns the inner store unchanged. No
/// directory is created, no decorator is allocated. Cheap opt-out.
pub fn wrap_with_file_logs(
    inner: Arc<dyn EventStore>,
    cfg: FileLogConfig,
) -> Arc<dyn EventStore> {
    if !cfg.enabled {
        return inner;
    }
    Arc::new(FileLogMirror::new(inner, cfg.directory))
}
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p stepyard-session --no-default-features --features sqlite --test file_log_mirror 2>&1 | tail -10`
Expected: 4 tests pass (Task 4 + Task 5 + 2 new in this task).

- [ ] **Step 5: Commit**

```bash
git add crates/stepyard-session/src/factory.rs crates/stepyard-session/tests/file_log_mirror.rs
git commit -m "feat(session): factory.wrap_with_file_logs honors enabled flag"
```

---

## Task 7: Add `--no-file-logs` CLI flag and `STEPYARD_NO_FILE_LOGS` env

**Files:**
- Modify: `src/cli/commands.rs`

- [ ] **Step 1: Read the field block**

Run: `grep -n 'pub no_sandbox\|pub sandbox_runtime\|pub no_file_logs' src/cli/commands.rs`
Expected: see `pub no_sandbox` (existing) and possibly `pub sandbox_runtime` if A2 has merged.

- [ ] **Step 2: Add the field to `ExecuteArgs`**

In `src/cli/commands.rs`, in the `ExecuteArgs` struct, after the existing sandbox flags, add:

```rust
    /// Disable the per-session JSONL file log at `.stepyard/logs/<id>.jsonl`.
    /// The DB event store is still the source of truth; this only suppresses
    /// the mirror writer. Equivalent: `STEPYARD_NO_FILE_LOGS=1`.
    #[arg(long = "no-file-logs")]
    pub no_file_logs: bool,
```

- [ ] **Step 3: Resolve the flag where the store is built**

Find the call site that builds the store. After PR A1 it is `build_store_from_env(...).await?` in `execute_v2` (search: `grep -n 'build_store_from_env' src/`). After:

```rust
let store = build_store_from_env().await?;
```

Replace with:

```rust
let store = build_store_from_env().await?;
let file_logs_enabled = !args.no_file_logs
    && std::env::var("STEPYARD_NO_FILE_LOGS")
        .map(|v| v != "1")
        .unwrap_or(true);
let log_dir = std::path::PathBuf::from(".stepyard").join("logs");
let store = stepyard_session::factory::wrap_with_file_logs(
    store,
    stepyard_session::FileLogConfig {
        enabled: file_logs_enabled,
        directory: log_dir,
    },
);
```

(If A1 used a different factory function name, mirror that name. The crucial point is the wrap happens *after* the inner store is built.)

- [ ] **Step 4: Confirm both profiles still build**

```bash
cargo check -p stepyard --no-default-features --features postgres 2>&1 | tail -8
cargo check -p stepyard --no-default-features --features sqlite 2>&1 | tail -8
```
Expected: `Finished` on both.

- [ ] **Step 5: Run all integration tests**

```bash
cargo test --workspace --no-default-features --features sqlite 2>&1 | tail -10
cargo test --workspace --no-default-features --features postgres 2>&1 | tail -10
```
Expected: green on both.

- [ ] **Step 6: Commit**

```bash
git add src/cli/commands.rs
git commit -m "feat(cli): --no-file-logs flag + STEPYARD_NO_FILE_LOGS env opt-out"
```

---

## Task 8: End-to-end CLI test for the file log mirror

**Files:**
- Create: `tests/cli_file_logs.rs`
- Re-use: `tests/fixtures/hello_lite.yaml` (created in PR A2; if A2 has not merged, create a minimal local fixture inline)

- [ ] **Step 1: Author the test**

Create `tests/cli_file_logs.rs`:

```rust
//! End-to-end check: `stepyard run hello.yaml` writes
//! `.stepyard/logs/<session_id>.jsonl` with the full event sequence.

#![cfg(feature = "sqlite")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn build_binary() {
    let out = Command::new(env!("CARGO"))
        .args([
            "build",
            "--no-default-features",
            "--features",
            "sqlite",
            "--bin",
            "stepyard",
        ])
        .output()
        .expect("cargo build failed");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn fixture(name: &str) -> PathBuf {
    project_root().join("tests").join("fixtures").join(name)
}

fn ensure_fixture() -> PathBuf {
    let path = fixture("hello_lite.yaml");
    if !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "name: hello_lite\nsteps:\n  - name: greet\n    type: cmd\n    command: echo\n    args: [\"hi\"]\n",
        )
        .unwrap();
    }
    path
}

#[test]
fn file_log_jsonl_is_written_alongside_event_store() {
    build_binary();
    let bin = project_root().join("target").join("debug").join("stepyard");
    let workflow = ensure_fixture();

    let cwd = tempfile::tempdir().expect("tempdir");
    let db_path = cwd.path().join("sessions.db");

    let output = Command::new(&bin)
        .current_dir(cwd.path())
        .arg("execute")
        .arg(&workflow)
        .arg("--engine")
        .arg("v2")
        .env("STEPYARD_HARNESS_DATABASE_URL", format!("sqlite://{}", db_path.display()))
        .env("STEPYARD_SANDBOX", "local")
        .output()
        .expect("run failed");

    assert!(
        output.status.success(),
        "stepyard exited non-zero.\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let log_dir = cwd.path().join(".stepyard").join("logs");
    assert!(log_dir.is_dir(), "expected {}", log_dir.display());
    let entries: Vec<_> = std::fs::read_dir(&log_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "expected one .jsonl file in log dir");

    let entry = entries.into_iter().next().unwrap().unwrap();
    let path = entry.path();
    let body = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert!(
        lines.len() >= 3,
        "expected at least workflow_started, step_started, workflow_completed (got {} lines)",
        lines.len()
    );
    for line in &lines {
        let _: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("bad json: {line}: {e}"));
    }
}

#[test]
fn no_file_logs_flag_suppresses_mirror() {
    build_binary();
    let bin = project_root().join("target").join("debug").join("stepyard");
    let workflow = ensure_fixture();

    let cwd = tempfile::tempdir().expect("tempdir");
    let db_path = cwd.path().join("sessions.db");

    let output = Command::new(&bin)
        .current_dir(cwd.path())
        .arg("execute")
        .arg(&workflow)
        .arg("--engine")
        .arg("v2")
        .arg("--no-file-logs")
        .env("STEPYARD_HARNESS_DATABASE_URL", format!("sqlite://{}", db_path.display()))
        .env("STEPYARD_SANDBOX", "local")
        .output()
        .expect("run failed");

    assert!(
        output.status.success(),
        "stepyard exited non-zero.\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let log_dir = cwd.path().join(".stepyard").join("logs");
    assert!(
        !log_dir.exists() || std::fs::read_dir(&log_dir).unwrap().count() == 0,
        "log dir should be empty or absent when --no-file-logs is set"
    );
}

#[test]
fn env_var_opt_out_works() {
    build_binary();
    let bin = project_root().join("target").join("debug").join("stepyard");
    let workflow = ensure_fixture();

    let cwd = tempfile::tempdir().expect("tempdir");
    let db_path = cwd.path().join("sessions.db");

    let output = Command::new(&bin)
        .current_dir(cwd.path())
        .arg("execute")
        .arg(&workflow)
        .arg("--engine")
        .arg("v2")
        .env("STEPYARD_NO_FILE_LOGS", "1")
        .env("STEPYARD_HARNESS_DATABASE_URL", format!("sqlite://{}", db_path.display()))
        .env("STEPYARD_SANDBOX", "local")
        .output()
        .expect("run failed");

    assert!(output.status.success());
    let log_dir = cwd.path().join(".stepyard").join("logs");
    assert!(
        !log_dir.exists() || std::fs::read_dir(&log_dir).unwrap().count() == 0,
        "STEPYARD_NO_FILE_LOGS=1 should suppress the mirror"
    );
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test --test cli_file_logs --no-default-features --features sqlite 2>&1 | tail -25`
Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/cli_file_logs.rs
git commit -m "test(cli): end-to-end file log mirror + opt-out flag/env"
```

---

## Task 9: README + help text update

**Files:**
- Modify: `README.md`
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: README section**

In `README.md`, add (near the section added by PR A2 if it merged, otherwise wherever CLI flags are documented):

````markdown
### File log mirror (`.stepyard/logs/`)

Every session writes a JSONL mirror of its event log to
`.stepyard/logs/<session_id>.jsonl` (always on by default). One JSON object
per line, schema identical to the rows in the `session_events` table.

Disable via flag or env:

```bash
stepyard execute hello.yaml --no-file-logs
STEPYARD_NO_FILE_LOGS=1 stepyard execute hello.yaml
```

If the mirror cannot write (disk full, permission denied), it emits one
`Event::FileLogWriteFailed` to the event store, disables itself for that
session, and the workflow continues normally — the event store remains the
source of truth.
````

- [ ] **Step 2: Help banner**

Open `src/cli/mod.rs`. Find the help banner block and add a line after the existing sandbox bullets:

```
• File logs           — written to .stepyard/logs/<session_id>.jsonl (disable with --no-file-logs)
```

- [ ] **Step 3: Commit**

```bash
git add README.md src/cli/mod.rs
git commit -m "docs(file-logs): document .stepyard/logs/ mirror and opt-out"
```

---

## Task 10: Final gates + open PR

**Files:** none

- [ ] **Step 1: Format + clippy on both profiles**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --no-default-features --features postgres -- -D warnings 2>&1 | tail -5
cargo clippy --workspace --all-targets --no-default-features --features sqlite -- -D warnings 2>&1 | tail -5
```
Expected: clean on both.

- [ ] **Step 2: Audit-emit-before-io baseline**

```bash
cargo run -p xtask -- audit-emit-before-io 2>&1 | tail -5
```
Expected: `3 finding(s)` (unchanged baseline).

If the count rose: the most likely violation is the inner `self.inner.append(degradation)` call in `FileLogMirror::append` running after the file IO. The current arrangement is intentional — the file IO is the operation we just finished and the degradation event is the audit message *for that IO* — but document it in the PR body if the audit flags it.

- [ ] **Step 3: Run audit-patterns**

Run: `bash scripts/audit-patterns.sh 2>&1 | tail -5`
Expected: blocking gates pass.

- [ ] **Step 4: Smoke run**

```bash
rm -rf /tmp/stepyard-a3-smoke && mkdir -p /tmp/stepyard-a3-smoke
cd /tmp/stepyard-a3-smoke && STEPYARD_HARNESS_DATABASE_URL=sqlite:///tmp/stepyard-a3-smoke/sessions.db \
  STEPYARD_SANDBOX=local \
  cargo run --manifest-path "$OLDPWD/Cargo.toml" --no-default-features --features sqlite -- \
    execute "$OLDPWD/tests/fixtures/hello_lite.yaml" --engine v2
ls -la /tmp/stepyard-a3-smoke/.stepyard/logs/
cd "$OLDPWD"
```
Expected: at least one `<uuid>.jsonl` file with multiple lines.

- [ ] **Step 5: Push and open PR**

```bash
git push -u origin feat/pr-a3-file-log-mirror
gh pr create \
  --title "feat(session): file log mirror + non-silent degradation (Stepyard Lite PR A3)" \
  --body "$(cat <<'EOF'
## Summary

- Adds `FileLogMirror` decorator that wraps any `EventStore` and double-writes appended `SessionEvent`s to `.stepyard/logs/<session_id>.jsonl`.
- Adds `Event::FileLogWriteFailed` variant for visible degradation (one emit per session, then the mirror disables itself for that session).
- Always on by default. Opt-out via `--no-file-logs` flag or `STEPYARD_NO_FILE_LOGS=1` env.
- The originating step is **never** failed by a file IO error; the event store remains the source of truth.

Spec ref: `docs/superpowers/specs/2026-05-01-stepyard-lite-design.md` §7.

## Test plan

- [ ] `cargo test -p stepyard-session --no-default-features --features sqlite` covers happy path, write-failure degradation, and opt-out
- [ ] `cargo test --test cli_file_logs --no-default-features --features sqlite` covers end-to-end CLI behavior (3 tests)
- [ ] `cargo test --workspace --no-default-features --features postgres` green (no regressions; new tests are sqlite-cfg'd)
- [ ] `cargo clippy -- -D warnings` clean on both profiles
- [ ] `cargo run -p xtask -- audit-emit-before-io` baseline still 3
- [ ] Manual smoke: `rm -rf .stepyard/logs && stepyard execute tests/fixtures/hello_lite.yaml --engine v2` produces `.stepyard/logs/<uuid>.jsonl` with the full event sequence
- [ ] Manual smoke: `--no-file-logs` produces no log dir

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 6: Watch CI and merge**

Poll: `gh pr view --json statusCheckRollup,mergeable,mergeStateStatus | jq`
On all green: `gh pr merge --squash --auto`.

---

## Self-Review

**Spec coverage (§7):**
- [x] `FileLogMirror` wraps `Arc<dyn EventStore>` and double-writes to `.stepyard/logs/<session_id>.jsonl` (Tasks 3–4).
- [x] One JSON object per line, JSON serialization of the `SessionEvent` shape (Task 4 step 3 `serde_json::to_vec(event)`).
- [x] Always on, opt-out via `--no-file-logs` / `STEPYARD_NO_FILE_LOGS=1` (Task 7).
- [x] Failure emits one `Event::FileLogWriteFailed` to the inner store, sets per-session broken flag, never fails the step (Task 4 step 3 `append` impl, Task 5 test).
- [x] Visible degradation per silent-failure-hunter convention (variant added in Task 2).
- [x] New test `tests/file_log_mirror.rs` covers happy path + write-failure + opt-out (Tasks 4, 5, 6).

**Placeholder scan:** None — every step has the actual code or shell command.

**Type consistency:**
- `SessionId`, `SessionEvent`, `EventStore`, `SessionMeta` all from PR A1 — names match the A1 plan's Task 4/6 definitions.
- `FileLogConfig { enabled, directory }` field shape matches between Task 3 (definition), Task 6 (test), Task 7 (CLI wiring).
- `wrap_with_file_logs(inner, cfg) -> Arc<dyn EventStore>` signature matches between Task 6 step 3 (definition) and Task 7 step 3 (call site).
- `Event::FileLogWriteFailed { error_class, timestamp }` field shape matches between Task 2 (variant) and Task 4 step 3 (constructor).

**Open issues for PR review:**
1. The `classify_io_error` helper depends on `ErrorKind::StorageFull` (stable in Rust 1.83+). If the workspace MSRV is older, the runtime `format!("{kind:?}") == "StorageFull"` trick will simply never match and disk-full will fall through to `io_other`. Acceptable lossy classification; revisit when MSRV bumps.
2. Spec §7 says "in `stepyard-session` *or* in the binary glue layer". This plan puts it in `stepyard-session` so the future `stepyard-embed` library spec gets the same behavior for free. If reviewers prefer keeping `stepyard-session` minimal, move to a new `stepyard-cli-glue` mini-crate in a follow-up.
3. The `replay()` impl reads from the inner store, not the file. This is by design (the event store is source of truth) but it means a user who deletes the DB and keeps the JSONL cannot replay. The README in Task 9 should call this out explicitly — the current draft does, but reviewers may want a stronger banner.
4. The Task 4 impl reuses the `sessions` lock to guard both `MirrorState` lookup AND the file write. If file IO is slow, this serializes appends across all sessions for the brief window of the lock. If this becomes a bottleneck, switch to per-session `Arc<Mutex<MirrorState>>` retrieved out of an outer `RwLock<HashMap<SessionId, _>>`. Out of scope here.
