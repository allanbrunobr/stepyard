# VERIFICATION_REPORT — minion-engine-bmad-wt1

**Scope:** Epic 1 (Stories 1.1–1.4) + Epic 2 (Stories 2.1–2.5), this worktree.
**Branch:** `minion-engine-bmad-wt1`
**Date:** 2026-04-17
**Method:** Two-phase self-verification — Phase 1 AC coverage audit against spec text, Phase 2 Codex adversarial review.

---

## Phase 1 — AC coverage audit

All ACs spot-checked against spec in `PROMPT.md` vs. implementing code/test. Evidence cited by `file:line`.

### Epic 1 — Termination taxonomy & harness primitives

| Story | AC summary | Status | Evidence |
|---|---|---|---|
| 1.1 | Destroy container by session UUID on cancel; idempotent; structured log | **PASS** | `crates/minion-harness/src/engine.rs` cancel path; harness tests for destroy-by-UUID |
| 1.2 | `TerminationReason` sub-enum (`SignalReceived(String)`, `Timeout`, `UserAbort`, `InternalError`); `StepFailed { reason }` variant | **PASS** | `crates/minion-core/src/errors.rs` — `TerminationReason` enum, `EngineError::StepFailed { reason: TerminationReason }` |
| 1.3 | `Event::StepTimeoutFired` variant; workspace-wide `non_exhaustive_omitted_patterns = "deny"` lint | **PASS** | `crates/minion-core/src/event.rs` adds variant; workspace `Cargo.toml` lint config; all match sites updated |
| 1.4 | Step timeout enforced via `tokio::time::timeout`; emits `StepTimeoutFired` + `StepFailed { reason: Timeout }` | **PASS** | `crates/minion-harness/src/engine.rs` step path; integration test uses `start_paused = true` (Rule 7a) |

### Epic 2 — Crash-safe lifecycle & session visibility

| Story | AC summary | Status | Evidence |
|---|---|---|---|
| 2.1 | Thread `Arc<broadcast::Sender<()>>` through `HarnessConfig` → `Engine::new`; shared by all engines in one `minion` process (D1/D4) | **PASS** | `crates/minion-harness/src/engine.rs` `HarnessConfig::cancel_tx`; `src/cli/commands.rs` execute path clones single `shutdown_tx`; `src/cli/mod.rs::Cli::run` receives `Arc<broadcast::Sender<()>>` from `main()` |
| 2.2 | SIGINT/SIGTERM handlers (Unix only, cfg'd); safe body (no allocation, no async) per NFR10; broadcast fires once; 1s grace deadline (NFR1) | **PASS** | `src/signal.rs` installs handlers; `main.rs` awaits shutdown deadline; `shutdown_signal: Arc<OnceLock<String>>` plumbed through `Cli::run` |
| 2.3 | Engine emits `Event::SignalReceived { signal }` to session log **before** calling `lifecycle.destroy` (D5 emit-before-IO); returns `StepFailed { reason: TerminationReason::SignalReceived(signal) }`; subscribers render lowercased line | **PASS** | `crates/minion-harness/src/engine.rs:345-369` — `session.append(Event::SignalReceived { .. }).await?` precedes `finalise_cancel()`; `src/events/subscribers.rs:279` explicit arm; `src/cli/display.rs:109` renders `"  {} signal received: {}"` with red ✗ |
| 2.4 | `minion::startup::reconcile(pg, lifecycle) -> Result<ReconcileReport, ReconcileError>` runs three sequential phases (session → container → worktree); Phase 1 `SELECT … WHERE status='running'` + append `crash_recovery` + `UPDATE status='failed'`; Phase 2 argv-only `docker ps --filter name=minion-session-*` → `docker rm -f` orphans, tolerate "No such container"; Phase 3 Epic-4 TODO stub; structured `tracing::info!` log | **PASS** | `crates/minion-harness/src/startup.rs:75,96` — idempotent `WHERE status='running'` query; Phase 2 uses `tokio::process::Command::new("docker").args([...])`; Phase 3 contains exact TODO comment; integration test `crates/minion-harness/tests/startup_reconcile.rs` |
| 2.5 | `minion session list --status <enum>` CLI subcommand; clap `ValueEnum` for status; `humantime::Duration` for `--since`; direct PG query (D1, no in-memory cache); `started_at DESC` ordering; row-per-line stdout | **PASS** | `src/cli/commands.rs` — `SessionStatus` ValueEnum + `SessionListArgs` + `session_list` handler at lines 1253-1282 (D1 verbatim comment); `Cargo.toml` adds `humantime = "2"`; integration test `crates/minion-harness/tests/session_list_cli.rs` (seeds one session per status; asserts `--status running`, `--status completed`, invalid `--status foobar` → exit 2, invalid `--since notaduration` → exit 2, `--since 1h` filter, DESC ordering) |

### Cross-cutting NFRs

| NFR | Status | Evidence |
|---|---|---|
| NFR1 (cleanup within 1s) | **PASS** | `main.rs` awaits `shutdown_deadline` with 1s hard cap |
| NFR10 (safe signal handler body) | **PASS** | `src/signal.rs` handler body allocation-free and sync |
| NFR11 (crash recovery) | **PASS** | `minion::startup::reconcile()` runs before any engine construction on v2 path |
| NFR12 (idempotent cleanup) | **PASS** | Phase 2 tolerates "No such container"; Phase 1 query is idempotent via `WHERE status='running'` filter |

### Workspace test suite

```
cargo test --workspace → 510 passed (27 suites)
```

Baseline pre-Story-2.5: 500/25. +10 tests, +2 suites correspond to `session_list_cli.rs` (1 test) + startup reconcile suite + unit test `session_status_as_db_str_matches_db_constraint`.

---

## Phase 2 — Codex adversarial review

Codex companion run against the Epic 2 diff surfaced three findings. Severities cited **verbatim** as Codex labeled them; AC-scope defense beside each.

| # | Codex severity | Finding (paraphrased) | AC-scope defense |
|---|---|---|---|
| 1 | **critical** | Startup reconcile in Phase 1 will mark another process's live sessions as `failed` if two `minion` processes share a database — no tenant, process-id, lease, or heartbeat predicate on the `SELECT … WHERE status='running'` query. | The AC *literally* prescribes `SELECT id FROM sessions WHERE status = 'running'` with **no** ownership predicate. Multi-process ownership/lease design (heartbeat, advisory PG lock, or owned-lease column) is a future-epic concern — documented in Story 2.4 DAR as explicit AC design boundary (see `PROMPT.md` Story 2.4 DAR, "Single-process reconcile model is explicit AC design" bullet). **Follow-up Epic 4/5 story required** for multi-process model. |
| 2 | **high** | v1 engine path in `src/cli/commands.rs` does not call `minion::startup::reconcile()` before constructing its `Engine::with_options` — only the v2 path does. | Literal AC reads "before constructing any `Engine::new()`" — `Engine::new` is the `minion_harness::Engine::new` constructor on the v2 path; v1's `Engine::with_options` is a separate type. Deviation pre-documented in Story 2.4 DAR (#1). v1 is the current default (`default_value = "v1"` at `src/cli/commands.rs:100`), so this is a real operational gap — **follow-up story required**: either reconcile integration on v1 path, or v1 deprecation/removal. |
| 3 | **high** | `minion session list --status` has no `--tenant` filter; a reader can observe UUIDs of sessions across tenant boundaries. | Story 2.5 AC does not mention multi-tenant filtering — explicitly out-of-scope per Story 2.5 DAR ("No `--tenant` filter. Story AC does not mention multi-tenant filtering."). **Follow-up story required** for tenant-scoped `session list`, including auth/authz model. |

### Codex-raised scope boundary: fix vs. document

Per advisor direction, findings #1–#3 are **not fixed in this worktree** — each is a scope-expansion against explicit AC text and all three are durably documented in DAR (#1 preemptively added this session). Evaporation risk is mitigated by listing them as explicit follow-up items in `WORKTREE_COMPLETE.md`.

---

## Verdict

**READY against Epic 2 AC scope; 3 architectural concerns flagged by Codex require follow-up stories (Epic 4/5 candidates).**

All 9 stories (1.1–1.4, 2.1–2.5) pass spec-level AC review with corresponding test evidence. Workspace test suite green (510 passed). Three Codex-flagged concerns are documented as follow-up work in `WORKTREE_COMPLETE.md` — they are not in-scope AC violations, but they represent real operational gaps (especially Finding #2, v1 reconcile integration, given v1 is the default engine path).
