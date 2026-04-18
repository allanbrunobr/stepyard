# WORKTREE_COMPLETE — minion-engine-bmad-wt1

**Branch:** `minion-engine-bmad-wt1`
**Completed:** 2026-04-17
**Scope:** Epic 1 (F1–F4) + Epic 2 (Crash-Safe Process Lifecycle & Session Visibility)

---

## Shipped

Nine stories, nine commits, all sequential; workspace test suite green (`cargo test --workspace` → 510 passed, 27 suites).

### Epic 1 — Termination taxonomy & harness primitives

| Commit | Story | Summary |
|---|---|---|
| `c95b775` | 1.1 | Destroy container by session UUID on cancel (idempotent) |
| `4cf74d7` | 1.2 | `TerminationReason` sub-enum + `EngineError::StepFailed { reason }` |
| `89d7d63` | 1.3 | `Event::StepTimeoutFired` variant + workspace `non_exhaustive_omitted_patterns = "deny"` lint |
| `a7b27ef` | 1.4 | Enforce step timeout via `tokio::time::timeout`; emit `StepTimeoutFired` + `StepFailed { reason: Timeout }` |

### Epic 2 — Crash-safe process lifecycle & session visibility

| Commit | Story | Summary |
|---|---|---|
| `9d77a0f` | 2.1 | Thread `Arc<broadcast::Sender<()>>` through `HarnessConfig` → `Engine::new` (D1/D4) |
| `3c4b282` | 2.2 | Install SIGINT/SIGTERM handlers; graceful shutdown deadline (NFR1/NFR10) |
| `e6a84b6` | 2.3 | Engine emits `Event::SignalReceived` **before** container destroy (D5 emit-before-IO); `StepFailed { reason: SignalReceived(signal) }` |
| `ac249f4` | 2.4 | `minion::startup::reconcile()` — three-phase crash-recovery (session → container → Epic-4 worktree stub) |
| `c0388dc` | 2.5 | `minion session list --status <enum> [--since <duration>]` CLI subcommand; direct PG query (D1, no cache); DESC ordering |

---

## Follow-up items (explicit, Epic 4/5 candidates)

Three architectural concerns surfaced by Codex adversarial review. Each is a **scope-expansion against Epic 2 AC text**, not an in-scope AC gap — but each represents a real operational gap that must be tracked into a future epic. Do not let them evaporate.

### (a) Multi-process ownership/lease model for reconcile
- **Source:** Codex Finding #1 (critical).
- **Gap:** `minion::startup::reconcile()` Phase 1 `SELECT … WHERE status='running'` has no ownership predicate (tenant, pid, lease, heartbeat). Two `minion` processes sharing a database would mark each other's live sessions as `failed` on startup.
- **Current status:** Documented as explicit AC-design scope boundary in Story 2.4 DAR (`PROMPT.md` — "Single-process reconcile model is explicit AC design" bullet).
- **Action:** Design/implement multi-process ownership (heartbeat, advisory PG lock, or owned-lease column) as its own epic story. Must land **before** any multi-process deployment.

### (b) Reconcile integration on v1 engine path (or v1 deprecation)
- **Source:** Codex Finding #2 (high).
- **Gap:** `minion::startup::reconcile()` is called on the v2 execute path only. v1 (`Engine::with_options` in `src/cli/commands.rs`) does not call reconcile. **v1 is currently the default** (`--engine` arg `default_value = "v1"` at `src/cli/commands.rs:100`), so crash recovery is effectively off for default-path users.
- **Current status:** Deviation pre-documented in Story 2.4 DAR #1.
- **Action:** Either (i) wire `reconcile()` into the v1 execute path with the same call-before-engine-construction ordering, or (ii) deprecate v1 as part of a v2-default flip story. Track as blocker for Epic 2 operational completeness.

### (c) Tenant-scoped `session list`
- **Source:** Codex Finding #3 (high).
- **Gap:** `minion session list --status` returns all sessions across all tenants. No `--tenant` filter, no auth/authz check. A reader with PG credentials can observe session UUIDs across tenant boundaries.
- **Current status:** Out-of-scope per Story 2.5 DAR ("No `--tenant` filter. Story AC does not mention multi-tenant filtering.").
- **Action:** Add `--tenant` filter + define auth/authz model for operator CLIs. Track as prerequisite for multi-tenant deployment.

---

## Verification artifacts

- `VERIFICATION_REPORT.md` — AC coverage table (all 9 stories PASS) + Codex findings table with verbatim severities + verdict.
- `PROMPT.md` — Dev Agent Record for each story, DAR additions this session.
- `.done` — completion signal file.

**Verdict:** READY against Epic 2 AC scope; 3 architectural concerns flagged by Codex require follow-up stories (Epic 4/5 candidates).
