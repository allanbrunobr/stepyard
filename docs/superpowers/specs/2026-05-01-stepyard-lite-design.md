# Stepyard Lite — Design Specification

**Date:** 2026-05-01
**Status:** Draft for review
**Owner:** allanbrunoafya
**Predecessors:** Task #31 (v1→v2 migration, in flight), PR #43 (parallel v2 merged 2026-05-01)
**Successors (out of scope, future specs):** `stepyard-embed` library API, `branch_strategy` declarativo, completion-signal Agent variant

---

## 1. Context and Motivation

Stepyard today requires Postgres + Docker to run even a trivial `Cmd` step. After comparing against [sandcastle](https://github.com/ai-hero/sandcastle) — a TypeScript library that orchestrates AI coding agents via npm with no DB and pluggable sandbox providers — three friction points stand out for local dev and zero-friction adoption:

1. **Postgres requirement** — every replay-test, every `cargo test`, every demo needs a running database.
2. **Docker requirement** — every `Cmd` step routes through `stepyard-sandbox-orchestrator` Docker lifecycle, even for harmless local commands.
3. **No tail-friendly log surface** — the only way to inspect a running session is the React dashboard or `psql` queries.

This spec defines **Stepyard Lite**: a *build profile* of the existing engine that swaps the heavy parts for lightweight defaults, without forking the codebase or duplicating crates. The aspirational user story:

```
$ cargo install stepyard-engine --features sqlite
$ stepyard run hello.yaml
```

…and that just works, with no Postgres, no Docker, no migrations to apply by hand.

---

## 2. Goals (in scope)

1. **SQLite event store backend** as a compile-time alternative to Postgres, gated by mutually-exclusive Cargo features `postgres` (default) and `sqlite`.
2. **CLI sandbox-mode flag** (`--sandbox=docker|local` and `STEPYARD_SANDBOX` env) defaulting to `local` in the `sqlite` profile and `docker` in the `postgres` profile, wiring to the existing `LocalShellLifecycle`.
3. **File log mirror** at `.stepyard/logs/<session_id>.jsonl`, append-only, written alongside the event store, always on (opt-out via `--no-file-logs`).

Each goal lands as one independently-reviewable PR, gated behind Task #80 (which unblocks the v2-default flip and must merge first to avoid event-schema conflicts).

---

## 3. Non-goals

Explicitly **out of scope** for this spec — each may become its own future spec:

- **`stepyard-embed` library crate** — exposing `stepyard::run(...)` for in-process Rust embedding.
- **Multi-tenancy on SQLite** — Lite mode assumes a single-process owner of the SQLite file. Concurrent multi-session writes are undefined behavior.
- **Web dashboard SQLite support** — `packages/web/` continues to expect Postgres. Lite users use file logs or CLI introspection.
- **Data migration between backends** — no Postgres↔SQLite dump/load tool. Each backend is a fresh start.
- **Branch strategies, completion-signal Agents, idle timeouts, prompt sugar** — all deferred to separate specs.
- **Removing Postgres** — `postgres` remains the default and supported production profile. This spec adds an alternative; it does not replace.

---

## 4. Architecture

### 4.1 Build profile model

Single workspace, single binary, two Cargo features at the workspace root:

```toml
[features]
default = ["postgres"]
postgres = ["stepyard-session/postgres", "stepyard-engine/postgres"]
sqlite   = ["stepyard-session/sqlite",   "stepyard-engine/sqlite"]
```

Mutually exclusive. A `compile_error!()` in `stepyard-session/src/lib.rs` triggers when both features are enabled, and another when neither is enabled (same enforcement pattern already used in audit-emit-before-io's xtask).

### 4.2 Crate-level changes summary

| Crate | Change |
|---|---|
| `stepyard-core` | None (event schema is JSON, backend-agnostic). |
| `stepyard-session` | New `EventStore` trait. Two impls: `PgEventStore` (current code, `--features postgres`), `SqliteEventStore` (new, `--features sqlite`). Migration story split per backend. |
| `stepyard-harness` | `Engine::with_executor` and `Session::new` signatures updated to take `Arc<dyn EventStore>` instead of `&PgPool`. Logic untouched. |
| `stepyard-sandbox-orchestrator` | None — `LocalShellLifecycle` already exists. |
| `stepyard-engine` (binary) | New `--sandbox` CLI flag, profile-aware default. New `--no-file-logs` flag. Wiring of file-log writer. |
| `packages/web/` | None (Lite mode does not support the dashboard). |

### 4.3 PR sequencing

```
PR A1: stepyard-session ganha EventStore trait + SqliteEventStore impl
       │  (largest PR — schema, migrations, CI matrix lane, replay parity test)
       │
       ├──► PR A2: CLI --sandbox flag + LocalShellLifecycle wiring
       │   (medium — touches src/cli + src/main)
       │
       └──► PR A3: file log mirror writer
           (small — wraps Session with double-write)
```

A2 and A3 are independent of each other and can land in either order after A1.

### 4.4 Invariants preserved

- **Append-only event log.** Same write path semantics in both backends.
- **Replay determinism.** Same workflow + same input + same backend → identical event sequence (asserted by parity test in 7.1).
- **One-concern-per-PR.** Each of A1/A2/A3 reverts cleanly without affecting the others.
- **Audit-emit-before-io budget unchanged** (currently 3 baseline findings).
- **`scripts/audit-patterns.sh` blocking gates** continue to pass.

---

## 5. PR A1 — `EventStore` trait + SQLite implementation

The largest and most consequential PR. Detailed breakdown:

### 5.1 New trait

In `crates/stepyard-session/src/store.rs` (new module or extension of existing):

```rust
#[async_trait]
pub trait EventStore: Send + Sync + 'static {
    async fn append(
        &self,
        session_id: SessionId,
        event: serde_json::Value,
    ) -> Result<SessionEvent, SessionError>;

    async fn replay(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<SessionEvent>, SessionError>;

    async fn create_session(
        &self,
        id: SessionId,
        kind: String,
    ) -> Result<(SessionId, String, DateTime<Utc>), SessionError>;

    async fn load_session_meta(
        &self,
        id: SessionId,
    ) -> Result<SessionMeta, SessionError>;

    async fn update_status(
        &self,
        id: SessionId,
        status: SessionStatus,
        terminated_at: Option<DateTime<Utc>>,
    ) -> Result<(), SessionError>;

    /// Acquire a write-serialization lock for this session. In Postgres this
    /// is `pg_advisory_xact_lock` (held to transaction end). In SQLite this
    /// is achieved via `BEGIN IMMEDIATE`. Both serve to serialize concurrent
    /// `append` calls within a process. Cross-process serialization is a
    /// non-goal in Lite mode.
    async fn lock_session(&self, id: SessionId) -> Result<(), SessionError>;
}
```

`SessionMeta` is a small struct extracted from current `Session::load` (currently a tuple in `session.rs:120`).

### 5.2 Two implementations

**`PgEventStore`** — straight refactor of current code. `Session` struct's `pool: PgPool` field becomes `store: Arc<dyn EventStore>`. All `sqlx::query` / `sqlx::query_as` calls move into `PgEventStore` methods. Public surface of `Session` (the `append`, `replay`, `lock`, `update_status` methods) keeps the same shape but delegates to the trait.

**`SqliteEventStore`** — new. `pool: SqlitePool`. Schema mirror:

| Postgres | SQLite | Notes |
|---|---|---|
| `JSONB` | `JSON` (TEXT-backed) | sqlx serializes `serde_json::Value` identically. |
| `gen_random_uuid()` | host-side UUID via `Uuid::new_v4()` | Avoid SQLite uuid extensions for portability. |
| `CHECK (status IN (...))` | same | Both engines support it. |
| `pg_advisory_xact_lock(hashtextextended($1::text, 0))` | `BEGIN IMMEDIATE` transaction | SQLite doesn't have advisory locks; `BEGIN IMMEDIATE` acquires the reserved-write lock at start, which gives equivalent serialization for single-writer Lite mode. |
| `RETURNING` | `RETURNING` (sqlite ≥ 3.35) | Required version: pin `rusqlite` ≥ 3.35 via sqlx. Document in README. |
| `INTERVAL` arithmetic | substr/datetime fns | None used today — confirmed by survey. |

### 5.3 Migrations

Split `migrations/` into `migrations-pg/` and `migrations-sqlite/`. Each backend's `migrate()` function loads its own dir via `sqlx::migrate!(...)`. Schema files are textual mirrors with the differences in 5.2 applied.

`stepyard-session/src/lib.rs` exports a single `migrate(store: &dyn EventStore)` that delegates to the impl's own `migrate()`.

### 5.4 Public API change

Before:
```rust
let pool = PgPoolOptions::new().connect(&url).await?;
let session = Session::new(&pool, Uuid::new_v4(), "demo".into()).await?;
```

After:
```rust
let store: Arc<dyn EventStore> = build_store_from_env().await?;
let session = Session::new(store, Uuid::new_v4(), "demo".into()).await?;
```

Where `build_store_from_env()` is a feature-gated free function that:
- In `--features postgres`: opens a `PgPool` from `STEPYARD_HARNESS_DATABASE_URL`, returns `Arc<PgEventStore>`.
- In `--features sqlite`: opens a `SqlitePool` from `STEPYARD_HARNESS_DATABASE_URL` or default `~/.stepyard/sessions.db`, returns `Arc<SqliteEventStore>`.

All call sites updated:
- `crates/stepyard-session/tests/integration.rs`
- `crates/stepyard-harness/tests/*.rs` (every test that opens a pool today — ~12 files)
- `crates/stepyard-harness/src/engine.rs::Engine::with_executor` signature
- `src/main.rs` and `src/cli/*` glue

### 5.5 CI matrix

`.github/workflows/check.yml` gains a matrix dimension:
```yaml
strategy:
  matrix:
    backend: [postgres, sqlite]
```

Postgres lane: existing service-container job, runs `cargo test --features postgres --no-default-features`.
SQLite lane: no service container, runs `cargo test --features sqlite --no-default-features`. Faster; functions as a smoke lane.

Branch protection updated to require both lanes.

### 5.6 Replay parity test

New `crates/stepyard-harness/tests/replay_parity.rs` (gated to run only when both backends are buildable in dev — uses `cfg(any(feature = ..., feature = ...))` trick + a feature `parity-test`). Drives the same minimal workflow against both backends in sequence and asserts equivalent event sequences after normalization. Normalization steps: strip timestamps, recursively sort JSON object keys (Postgres `JSONB` reorders keys on storage; SQLite `JSON` text preserves insertion order — see 9.2), strip backend-specific row IDs. This is the primary correctness gate.

---

## 6. PR A2 — CLI `--sandbox` flag + LocalShell wiring

Smaller PR. Surface:

- Add `--sandbox <docker|local>` to `stepyard run`. Env: `STEPYARD_SANDBOX`. Precedence: CLI flag > env > profile default.
- Profile default: `local` when `--features sqlite`; `docker` when `--features postgres`.
- In `src/main.rs`, replace the unconditional `Arc::new(DockerLifecycle::new(...))` with a match on the resolved sandbox mode.
- `LocalShellLifecycle` is already exported from `stepyard-sandbox-orchestrator::local`; no new code there.
- New CLI integration test: `tests/cli_sandbox_flag.rs` exercises both modes with a trivial workflow.

No event-schema changes. No replay implications.

---

## 7. PR A3 — File log mirror

Smallest PR. Surface:

- New struct `FileLogMirror` in `stepyard-session` (or in the binary glue layer) that wraps an `Arc<dyn EventStore>` and double-writes every appended event to `.stepyard/logs/<session_id>.jsonl`.
- Format: one event per line, JSON serialization of the same `SessionEvent` shape stored in the DB. No transformation, no field renames.
- Always on; opt-out via `--no-file-logs` flag or `STEPYARD_NO_FILE_LOGS=1` env.
- **Failure mode:** if the file write fails (disk full, permission denied), the wrapper:
  1. Emits a one-shot `Event::FileLogWriteFailed` to the underlying store with the error class.
  2. Sets an internal "broken" flag and stops attempting further writes for that session.
  3. **Does NOT** fail the originating step. Event store remains source of truth.
  This is explicit per the silent-failure-hunter convention: visible degradation, not silent.
- New test `tests/file_log_mirror.rs` covering happy path + write-failure-degrades-gracefully + no-file-logs-flag-disables-writer.

---

## 8. Testing strategy

### 8.1 Existing tests
All current integration tests run in both backend lanes via the CI matrix. Tests that hard-code `STEPYARD_HARNESS_DATABASE_URL` to a Postgres URL get a helper `test_event_store()` that returns the right backend per cfg flag.

### 8.2 New tests
- `replay_parity.rs` — byte-equal event sequence across backends (5.6).
- `cli_sandbox_flag.rs` — CLI flag + env + profile-default precedence (PR A2).
- `file_log_mirror.rs` — happy path, write failure, opt-out (PR A3).

### 8.3 Gates
- `scripts/audit-patterns.sh` blocking gates remain green.
- `cargo run -p xtask -- audit-emit-before-io` baseline at 3 findings.
- `cargo clippy --all-targets -- -D warnings` per backend lane.

### 8.4 Non-tested
- Concurrent multi-process SQLite writes — out of scope (non-goal §3).
- Postgres ↔ SQLite data migration — out of scope.
- `packages/web/` against SQLite — out of scope (dashboard is Postgres-only).

---

## 9. Risks and open questions

### 9.1 Resolved during exploration
- ✅ `sqlx::query!` macro vs runtime `sqlx::query`: codebase uses runtime everywhere — feature switching is clean.
- ✅ `LocalShellLifecycle` already exists in `stepyard-sandbox-orchestrator::local` — no new sandbox code needed.
- ✅ `pg_advisory_xact_lock` mapping — `BEGIN IMMEDIATE` is the equivalent for single-writer SQLite.

### 9.2 Open
- **Default SQLite path.** Current proposal: `~/.stepyard/sessions.db`. Alternative: cwd-relative `.stepyard/sessions.db` to match `.stepyard/logs/`. Pick one in PR A1.
- **`SessionEvent` JSON encoding parity.** Postgres `JSONB` reorders keys; SQLite `JSON` text preserves insertion order. The replay parity test must normalize key order before comparing, or we'll get false negatives.
- **CI lane cost.** Doubling the workflow could push job time. Mitigation: SQLite lane skips Postgres-service-container setup, so net additional time is small. Verify in PR A1 CI run.
- **`cargo install --features sqlite` ergonomics.** `cargo install` does not propagate workspace features to binary crates cleanly in all versions. May require building from source with `--no-default-features --features sqlite` documented in README.

### 9.3 Future-spec dependencies
- `stepyard-embed` library crate (deferred) will reuse `EventStore` trait — the trait shape designed here is its public interface. Any narrowing of the trait now risks blocking the library spec.

---

## 10. Out-of-scope / explicit non-features

Restating §3 in negative form for reviewer clarity:

- This spec does **not** expose Lite as a separate binary or crate. Same `stepyard-engine`, two profiles.
- This spec does **not** add a runtime "switch backends mid-session" capability. Backend is fixed at compile time.
- This spec does **not** ship a Postgres → SQLite (or reverse) data migration tool. Each backend is greenfield.
- This spec does **not** support the React dashboard against SQLite. Use file logs or `sqlite3` CLI.
- This spec does **not** introduce concurrent multi-writer semantics on SQLite. Single-process Lite only.
- This spec does **not** modify any v2 step kind (Cmd/Call/Repeat/Map/Parallel/Template/Script/Agent/Chat) or scope semantics. Strictly a backend swap.

---

## 11. Acceptance criteria

The spec is complete when:

1. PR A1 merged: `cargo test --features sqlite --no-default-features` passes locally with no Postgres running; the same test suite passes under `--features postgres` against a running Postgres; replay parity test passes; CI matrix lane is green.
2. PR A2 merged: `STEPYARD_SANDBOX=local stepyard run hello.yaml` runs a `Cmd` step on the host without Docker; `--sandbox=docker` continues to work in the postgres profile.
3. PR A3 merged: after a Lite-mode `stepyard run`, `.stepyard/logs/<session_id>.jsonl` contains the full event sequence in JSONL format; deleting the file does not affect replay (event store remains source of truth).
4. End-to-end smoke: `cargo install stepyard-engine --features sqlite --no-default-features` (or local-build equivalent) → `stepyard run hello.yaml` → success, with no external dependencies running.

---

## 12. Implementation plan

To be produced by `superpowers:writing-plans` skill in the next stage of the brainstorming workflow.
