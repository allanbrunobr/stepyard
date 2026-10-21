---
stepsCompleted: [1, 2, 3, 4, 5, 6, 7, 8]
inputDocuments:
  - _bmad-output/sandcastle-features/prd.md
  - minion-engine/ARCHITECTURE.md
  - ARCHITECTURE-MINION-ENGINE.md
  - _bmad-output/engine-v2/epics.md
workflowType: 'architecture'
project_name: 'Stepyard — Sandcastle-Inspired Features'
user_name: 'Bruno'
date: '2026-04-16'
lastStep: 8
status: 'complete'
completedAt: '2026-04-16T00:00:00Z'
---

# Architecture Decision Document - Stepyard — Sandcastle-Inspired Features

_This document builds collaboratively through step-by-step discovery. Sections are appended as we work through each architectural decision together._

---

## Project Context Analysis

### Requirements Overview

**Functional Requirements:** 27 FRs across 6 categories, scoped as MVP (FR1-FR12), Growth (FR13-FR24), and Expansion (FR25-FR27). Architecturally they decompose into:

- **Step Execution Safety (FR1-FR4)** — timeout enforcement and cancel correctness as properties of `stepyard-harness::Engine`. Requires wrapping executor in `tokio::time::timeout()`, fixing `finalise_cancel()` to use actual session ID, and extending `Event` enum with termination-reason variants.

- **Process Lifecycle (FR5-FR8)** — OS signal handling at the CLI binary layer, coordinated with a new in-process active-session registry. Must integrate with existing `CancelToken` (`Arc<AtomicBool>`) mechanism without deadlocking the tokio runtime during Docker subprocess cleanup.

- **Sandbox Environment (FR9-FR12)** — extension of `SandboxLifecycle::exec()` signature to accept `HashMap<String, String>` environment dict, threaded through `docker exec --env` flags. Env var resolution uses cascading strategy (step-level > workflow-level > `.stepyard/defaults.yaml` > host `${VAR}`).

- **Git Workspace Management (FR13-FR18)** — introduces new `WorkspaceManager` trait for `git worktree` lifecycle. Enables N concurrent agents on same repo via isolated working directories. Supports three branch strategies (Head, MergeToHead, NamedBranch) with conflict-preservation semantics.

- **Workflow Configuration (FR19-FR21)** — YAML preprocessor layer that resolves `{{KEY}}` placeholders (from CLI args) and validates placeholder completeness pre-execution. Integrates with existing YAML parser via serde deserialization hook.

- **Session Observability (FR22-FR24)** — new `Event` variants (`IdleTimeoutFired`, `SignalReceived`, `BranchCreated`, `MergeAttempted`, `MergeConflict`) added as backward-compatible extensions via `#[non_exhaustive]` + `#[serde(other)]`. New CLI `session list --status` command for operator queries.

- **Provider Extensibility (FR25-FR27)** — leverages existing `SandboxLifecycle` trait to add Podman, cloud providers. Requires trait refinement for config object and optional TTY forwarding (Expansion phase only).

**Non-Functional Requirements:**

- **Performance:** Signal handler cleanup <1s; step timeout precision <1s of configured threshold; `tokio::time::timeout()` overhead <50ms; env var resolution <10ms; worktree creation <5s; worktree pruning <30s for 50 stale worktrees.
- **Security:** Env vars are opt-in only (no host env passthrough); values never logged to session events (names only); sandbox boundary preserved via `docker exec` interface; signal handler avoids PostgreSQL connection (best-effort event recording).
- **Reliability:** Crash recovery via `Session::replay()` with zero manual intervention; idempotent `destroy_by_session()` tolerates already-destroyed containers; deterministic termination events for every kill path; worktrees with uncommitted changes preserved, never deleted.
- **Integration:** Docker CE 20.10+ (no bollard); PostgreSQL via existing `session_events` table (no schema migration for MVP); git 2.30+ via CLI (no libgit2); existing YAML workflows unchanged via `#[serde(default)]`.
- **Maintainability:** `cargo clippy -- -D warnings` clean; >=70% unit coverage per modified crate; `thiserror` in library crates (no `anyhow`); no breaking changes to public trait signatures in MVP.

### Scale & Complexity

Project complexity is **high**. The engine exists in a mid-refactor state (Engine v2, Epics 1-2 mostly complete, 3-5 pending). This PRD adds features that touch **every existing crate** plus introduces one new trait (`WorkspaceManager`).

- **Primary technical domain:** Rust backend CLI + library for AI agent orchestration with Docker container lifecycle management.
- **Complexity level:** High — process isolation, signal safety, multi-provider abstraction, concurrent session execution with git worktree isolation, crash recovery via event replay.
- **Estimated architectural components:** Modifications to 4 existing crates, 1 new trait (`WorkspaceManager`), 1 new CLI subsystem (signal handler + active session registry), 5+ new `Event` enum variants, 3+ YAML schema additions.

### Technical Constraints & Dependencies

**Hard constraints (non-negotiable):**

- **No new crates.** All features land in existing `stepyard-core`, `stepyard-session`, `stepyard-sandbox-orchestrator`, `stepyard-harness`. ADR-011 reserves the only planned future crate (`stepyard-mcp-proxy`) for a separate concern.
- **No new binaries.** Single `stepyard` binary; existing plan for `stepyard-mcp-proxy` as second binary is unrelated.
- **Docker CLI subprocess only.** No bollard or embedded Docker client. All operations via `tokio::process::Command` invoking `docker run`, `docker exec`, `docker rm -f`. String-parsed errors.
- **Session-log-as-truth.** The engine holds zero in-memory state between steps. Any new feature (timeout config, branch state, env dict) must be expressible as events in the session log; otherwise resume-after-crash cannot reconstruct state.
- **Rust async safety.** All types crossing task boundaries must be `Send + Sync`. No `Mutex<T>` held across `.await` points (prior bug: `Mutex<Option<Instant>>` made `Engine` futures `!Send`). Use `&mut self` exclusivity or `AtomicBool`.
- **Backward compatibility.** MVP preserves all existing public trait signatures. Env dict added via new method (not signature change). YAML fields added via `#[serde(default)]`.

**Runtime dependencies:**

- Rust edition 2021; `tokio` (async runtime), `sqlx` (PostgreSQL), `uuid`, `serde`, `thiserror`.
- New dependencies introduced by this PRD: `tokio::signal` (stdlib, no crate add), `tokio::time` (stdlib).
- No new external crates for env dict, branch strategies, or prompt templating (stdlib + git CLI).

**Integration surfaces:**

- PostgreSQL `session_events` table (existing); JSONB payload, new variants via `#[non_exhaustive]` + `#[serde(other)]`-safe.
- Workflow YAML schema (existing); new fields: `timeout:`, `env:`, `branch_strategy:`.
- CLI (existing); new flags/commands: `--timeout`, `--branch-strategy` (Growth), `session list` (Growth).

### Cross-Cutting Concerns Identified

1. **Session event emission discipline.** Every new feature must emit corresponding events to the session log. This is not optional — without it, resume-after-crash reconstructs incorrect state. Architecture must decide event granularity (e.g., emit on every env var resolution? only on failures? at worktree-level or repo-level?).

2. **Async `Send + Sync` correctness.** Signal handler and active session registry must be accessible from tokio tasks without locking across `.await` points. The existing `CancelToken` (`Arc<AtomicBool>`) is the proven pattern; architecture must extend this to cover container ID tracking and worktree paths.

3. **Backward compatibility envelope.** Every API change must be additive. `SandboxLifecycle::exec(id, cmd)` becomes `exec(id, cmd, env)` via a new method with default implementation, not by changing the existing signature. Architecture must define the default-impl pattern.

4. **Docker-CLI-only constraint.** Rules out typed error handling from Docker; must parse stderr strings. Architecture must define the error taxonomy (container-not-found vs daemon-unreachable vs image-pull-failed) with string-matching classifier.

5. **Test coverage obligation.** ≥70% unit coverage per modified crate. Integration tests must cover signal handling without flakiness (timing-sensitive). Architecture must specify mock strategy for lifecycle operations (extend `MockLifecycle` with env recording, container tracking).

---

## Starter Template Evaluation

### Primary Technology Domain

**Rust backend CLI + library** for AI agent orchestration. Brownfield — existing codebase is the canvas. No starter template selection applies.

### Starter Options Considered

Not applicable. The existing Stepyard workspace is mid-refactor (Engine v2, Epics 1-2 complete) with locked technology choices inherited from ADR-011, ADR-012 and the existing `minion-engine/ARCHITECTURE.md`. The PRD explicitly forbids new crates or binaries. All features land in existing crates.

### Inherited Foundation (Existing Scaffold)

**Language & Runtime:**

- Rust edition 2021
- `tokio` 1.x async runtime
- Single `stepyard` binary (plus reserved `stepyard-mcp-proxy` unrelated to this PRD)

**Dependencies (locked by existing workspace):**

- `sqlx` — PostgreSQL access (compile-time checked queries)
- `serde`, `serde_json`, `serde_yaml` — serialization
- `thiserror` — library error types (no `anyhow` in `lib.rs`)
- `anyhow` — allowed only in `main.rs`
- `chrono` — timestamps
- `uuid` — session IDs (v4), workflow version IDs (v5)
- `clap` (derive) — CLI parsing
- `tracing` + `tracing-subscriber` — structured logging

**New dependencies introduced by this PRD:**

- `tokio::signal` — stdlib, no crate add
- `tokio::time` — stdlib, no crate add
- No external crates for env dict, branch strategies, or templating (stdlib + git CLI)

**Workspace Structure (locked):**

- `crates/stepyard-core` — IO-free contracts (`Event`, `StepRecord`, `WorkflowDef`, `Subscriber` trait, `EngineError`). **No tokio, sqlx, reqwest.**
- `crates/stepyard-session` — `Session`, `SessionEvent`, `SessionId`, PostgreSQL append-only log. Advisory xact lock for concurrent appends.
- `crates/stepyard-harness` — `Engine` (step/resume/cancel), `HarnessConfig`, `StepExecutor` trait, `CancelToken`.
- `crates/stepyard-sandbox-orchestrator` — `SandboxLifecycle` trait, `DockerLifecycle`, `MockLifecycle`, `SandboxId`, `SandboxError`.

**Code Organization Patterns (inherited):**

- One crate per concern; crates communicate via `stepyard-core` contracts.
- Public types use `#[non_exhaustive]` for forward compatibility.
- Public enums use `#[serde(other)]` for schema evolution.
- Traits with `async fn` use `async_trait` (tokio runtime required).
- `MockLifecycle` pattern for tests — extend, don't reinvent.

**Testing Infrastructure (inherited):**

- Unit tests via `#[tokio::test]` inside each crate's `src/` modules.
- Integration tests via `assert_cmd` in workspace `tests/` directory.
- PostgreSQL fixture via `sqlx::test` macro (ephemeral DB per test).
- `MockLifecycle` for sandbox tests (call recording, no Docker required).
- Required coverage: ≥70% unit per modified crate, ≥50% integration overall.

**Development Experience (inherited):**

- `cargo build --release --features slack` for VPS deployment.
- `cargo clippy -- -D warnings` clean across workspace (non-negotiable).
- Deploy via `cargo install --path .` → `/usr/local/bin/stepyard`.
- Debug via `tracing` structured logs with `RUST_LOG=stepyard=debug`.

**Note:** Because this is brownfield, the "project initialization story" is N/A. The first implementation story is the MVP bug fix (Cancel cleanup fix, ~3 lines in `stepyard-harness/src/engine.rs` line 413-416).

---

## Core Architectural Decisions

_Ten decisions refined through Party Mode review with Winston (Architect), Amelia (Developer), Murat (Test Architect), and Dr. Quinn (Problem Solver). Revisions from the session are marked where applicable._

### D1 — Active Session Cancel Broadcast (revised)

**Decision:** Replace the initially-proposed global `once_cell::sync::Lazy<DashMap<SessionId, ActiveSession>>` registry with a per-process `Arc<tokio::sync::broadcast::Sender<()>>` owned by `main()` and passed into each `Engine::new()` call. Each engine subscribes during construction and aborts its current step if the broadcast fires.

**Why revised:** Dr. Quinn flagged that a mutable in-memory registry becomes a second source of truth alongside the session log, violating the "session-log-as-truth" invariant. Orphan container cleanup (the original justification for the registry) is a startup-time concern, not a runtime-registry concern (see Crash Recovery below).

**Rust shape:**

```rust
// stepyard-harness/src/engine.rs
pub struct Engine {
    session: Session,
    lifecycle: Arc<dyn SandboxLifecycle>,
    cancel: CancelToken,                 // per-engine Arc<AtomicBool>
    shutdown_rx: broadcast::Receiver<()>,// subscribes to process-wide signal
    // …
}
```

**Send + Sync safety:** `broadcast::Receiver` is `Send`; no `Mutex` crosses `.await`. The cancel flag remains `Arc<AtomicBool>` (the proven pattern).

**Rejected alternatives:** `DashMap` registry (dual source of truth); `Arc<Mutex<Vec<CancelToken>>>` (mutex across await risk in future extensions).

---

### D2 — Signal Handler as Broadcast Producer (revised)

**Decision:** The `stepyard` binary's `main()` installs a `tokio::signal::unix` handler for `SIGINT` and `SIGTERM` that:

1. Fires `broadcast::Sender<()>::send(())` once (best-effort; ignores `SendError` when no receivers exist).
2. Waits up to `HarnessConfig::shutdown_grace_s` (default 10s) for in-flight engines to complete their `finalise_cancel()` path.
3. Exits with code `130` (`128 + SIGINT`) or `143` (`128 + SIGTERM`) after grace expires.

**Why revised:** Original design had the signal handler directly iterate a `DashMap` and call `destroy()` on each entry — this duplicated the engine's own cancel-cleanup path and risked double-destroy races. Broadcast pattern delegates cleanup to each engine's existing `finalise_cancel()` (which will also be fixed per MVP bug).

**Signal-safety:** The handler itself does only atomic-signal-safe work (`broadcast::send`, which is lock-free for the single-producer case). PostgreSQL writes happen from the engine task, not the handler — best-effort only; crash-recovery replay reconciles if the signal-path write loses.

---

### D3 — SandboxLifecycle::exec() Env Extension via Default-Impl Method

**Decision:** Keep the existing `exec(id, cmd)` trait method signature. Add a sibling default method:

```rust
#[async_trait]
pub trait SandboxLifecycle: Send + Sync {
    async fn exec(&self, id: &SandboxId, cmd: &[String]) -> Result<ExecOutput, SandboxError>;

    async fn exec_with_env(
        &self,
        id: &SandboxId,
        cmd: &[String],
        env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        // Default: ignore env (preserves impls that predate this method)
        self.exec(id, cmd).await
    }
}
```

`DockerLifecycle` and `MockLifecycle` override `exec_with_env`. Callers inside `stepyard-harness` always invoke `exec_with_env` (resolving to empty-map when no env configured).

**Why:** Additive; no breaking change to downstream impls. MockLifecycle's existing call-recording is extended with an `env` field at the `MockLifecycleCall` struct.

---

### D4 — WorkspaceManager Lives in `stepyard-sandbox-orchestrator`

**Decision:** Introduce `pub trait WorkspaceManager` in `crates/stepyard-sandbox-orchestrator/src/workspace.rs`, not in `stepyard-core` and not in a new crate.

**Why:** Workspace concerns (git worktrees) sit at the same architectural layer as sandbox concerns (Docker containers): both are IO-bound external-process coordination used only by the harness. `stepyard-core` stays IO-free (its contract). Adding a new crate violates the PRD's "no new crates" constraint and buys nothing: the trait has one impl (`GitWorktreeManager`) plus its mock.

**Shape:**

```rust
#[async_trait]
pub trait WorkspaceManager: Send + Sync {
    async fn prepare(&self, spec: &WorkspaceSpec) -> Result<WorkspaceHandle, WorkspaceError>;
    async fn finalize(&self, handle: WorkspaceHandle, outcome: StepOutcome) -> Result<(), WorkspaceError>;
    async fn prune_stale(&self, retention: Duration) -> Result<PruneReport, WorkspaceError>;
}
```

---

### D5 — Event Variants Added Inline with Exhaustive-Match Guard (revised)

**Decision:** Add new variants directly to the existing `stepyard_core::Event` enum (no sub-enum, no wrapper):

- `StepTimeoutFired { step_index: u32, configured_ms: u64 }`
- `IdleTimeoutFired { step_index: u32, idle_threshold_ms: u64 }`
- `SignalReceived { signal: String }`
- `BranchCreated { branch: String, base: String }`
- `MergeAttempted { source: String, target: String }`
- `MergeConflict { source: String, target: String, files: Vec<String> }`
- `WorkspacePrepared { path: String, strategy: String }`
- `WorkspacePruned { path: String, reason: String }`

**Why revised:** Party Mode (Murat) added `#[deny(non_exhaustive_omitted_patterns)]` at exhaustive match sites (replay, subscribers, CLI formatters) to prevent silent-drop bugs when new variants land without updating every consumer. The `#[non_exhaustive]` + `#[serde(other)]` pattern preserves forward-compat for external JSONB readers; the `deny` lint forces internal correctness. _(Stability caveat: the lint is nightly-only on stable Rust — see §Pattern Enforcement for the compensating pre-merge grep audit.)_

**Serde:** All new variants use `#[serde(rename_all = "snake_case")]` consistent with existing variants. No schema migration required (payload column is `jsonb`).

---

### D6 — Env Var Resolution in `stepyard-harness::Engine::prepare_step` — argv-only (revised)

**Decision:** Env resolution lives in `Engine::prepare_step`, which builds the final `HashMap<String, String>` from four cascading sources and passes it to `exec_with_env`. Resolution order (highest wins): step-level → workflow-level → `.stepyard/defaults.yaml` → host env for `${VAR}` references.

**Security constraint (added by party mode):** Values are passed through `docker exec --env KEY=VAL` as argv arguments, **never shell-interpolated**. The step's command is also passed as `&[String]` argv, not joined into a shell string. This is documented at both `Engine::prepare_step` and `DockerLifecycle::exec_with_env` to prevent future refactors from introducing command injection.

**Why this location:** Engine owns the merge semantics (it has visibility to step + workflow). Lifecycle stays a dumb executor — it receives a resolved env map and a resolved argv, with no knowledge of where values came from.

**Testing obligation:** Negative-case tests that a value containing `$(rm -rf /)` or `` `cat /etc/passwd` `` appears verbatim in the container and does not execute.

---

### D7 — Template Substitution: Pre-Parse Pass, argv-only (revised)

**Decision:** `{{KEY}}` placeholder substitution happens in a dedicated preprocessor (`stepyard_core::template::substitute`) that runs **before** `serde_yaml::from_str`. Substitution sources: CLI `--var KEY=VAL` flags, then `.stepyard/defaults.yaml`.

**Security constraint (added by party mode):** The substituted result is a YAML document; substituted values become string literals in the YAML (quoted as needed). When those values eventually reach a step's `command:` field, they are split into argv by the workflow parser, never joined into a shell string.

**Why:** Pre-parse substitution is simpler than a `serde::Deserialize` hook (no custom Deserializer; works for any YAML shape). Missing placeholders produce `EngineError::PlaceholderUnresolved { key, found_at }` before any step runs.

**Testing obligation:** Negative-case tests: a CLI value of `foo; rm -rf /` becomes a literal string arg, not two shell commands.

---

### D8 — Worktree Pruning at Startup via `stepyard::startup::reconcile()` (revised)

**Decision:** Pruning is **not** called from `Engine::new()` (per-session) and **not** run on a timer. It runs once at `main()` startup inside a new `stepyard::startup::reconcile()` function.

**Why revised:** Dr. Quinn flagged that per-session pruning (a) creates unpredictable latency (every engine init scans filesystem) and (b) couples pruning to session creation (non-deterministic test behavior). Startup pruning is deterministic — exactly one scan per binary invocation.

**Two-phase protocol** (unchanged from original):

1. `git worktree prune` (removes git metadata for deleted worktrees).
2. Filesystem walk under `.stepyard/workspaces/`: for each orphan dir without a matching worktree entry, emit `WorkspacePruned`. **Skip** dirs with uncommitted changes (detected via `git status --porcelain`).

Retention: worktrees older than `HarnessConfig::workspace_retention_hours` (default 24h).

---

### D9 — Error Taxonomy via `TerminationReason` Sub-Enum (revised)

**Decision:** Replace the initially-proposed flat `EngineError` variants (`StepTimeout`, `IdleTimeout`, `Cancelled`, etc.) with a single structured variant:

```rust
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("step {step_index} failed: {reason}")]
    StepFailed {
        step_index: u32,
        reason: TerminationReason,
    },
    #[error("placeholder {key} unresolved (referenced at {found_at})")]
    PlaceholderUnresolved { key: String, found_at: String },
    #[error("sandbox error: {0}")]
    Sandbox(#[from] SandboxError),
    // … existing variants
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum TerminationReason {
    #[error("step timeout after {configured_ms}ms")]
    StepTimeout { configured_ms: u64 },
    #[error("idle timeout after {idle_ms}ms with no output")]
    IdleTimeout { idle_ms: u64 },
    #[error("cancelled by operator")]
    Cancelled,
    #[error("signal {0} received")]
    SignalReceived(String),
    #[error("{0}")]
    Other(String),
}
```

**Why revised:** Amelia pointed out that callers (CLI status formatters, replay, subscribers) want to match on "how did this step end" as a unit. A flat enum forces every consumer to list N variants; the sub-enum lets consumers match `StepFailed { reason, .. }` and dispatch on reason in one place. Also maps 1:1 to the new `Event::*TimeoutFired` variants (D5).

**Docker CLI error classification:** `DockerLifecycle` parses stderr strings into `SandboxError::{ContainerNotFound, DaemonUnreachable, ImagePullFailed, Other}` via a small string-matching classifier (documented at `crates/stepyard-sandbox-orchestrator/src/docker_errors.rs`).

---

### D10 — Branch Strategy Config: `branch_strategy:` in workflow YAML

**Decision:** Branch strategy is specified per-workflow (not per-step, not per-CLI-invocation) under a top-level YAML field `branch_strategy:`. Values: `head | merge_to_head | named_branch` (Rust snake_case to match existing YAML conventions).

**Why:** Branch semantics apply to an entire workflow execution (all steps share the workspace). Per-step would create undefined behavior for multi-step workflows. CLI flag `--branch-strategy` (Growth phase) overrides the YAML field for one-off runs.

**Validation:** `named_branch` requires a sibling `branch_name:` field (or `{{BRANCH_NAME}}` placeholder). Missing → `EngineError::PlaceholderUnresolved` at parse time.

---

### Crash Recovery — Startup Reconciliation (new, added by party mode)

**Decision:** `stepyard::startup::reconcile()` runs before any `Engine::new()` call in `main()`. It executes three independent steps in sequence:

1. **Session reconciliation:** `SELECT id FROM sessions WHERE status = 'running'` → for each, replay events to determine last-known state; if the session's container is gone (step 2 below), emit a `SignalReceived { signal: "crash_recovery" }` event and transition to `status='failed'`.
2. **Container reconciliation:** `docker ps --filter "name=stepyard-session-*" --format "{{.Names}}"` → list active containers; cross-reference against open sessions from step 1; destroy any container not matching a running session (orphan from prior crash).
3. **Worktree pruning:** per D8 two-phase protocol.

**Why new:** Dr. Quinn flagged that D1/D2's original DashMap registry was implicitly solving two problems — runtime cancel coordination AND crash-recovery orphan cleanup. The revision separates these concerns: cancel → broadcast (D1), orphan cleanup → startup reconcile (here). This is a cleaner architectural boundary and makes both behaviors deterministic and testable.

**Idempotency:** All three steps tolerate already-clean state. `DockerLifecycle::destroy_by_session()` is idempotent (existing behavior). `WorkspaceManager::prune_stale()` is idempotent by construction.

---

## Decision Menu

You have reviewed the 10 revised core architectural decisions (+ Crash Recovery section). Choose:

- **[A]** Propose an amendment or challenge a specific decision
- **[P]** Enter Party Mode again for further multi-agent review
- **[C]** Accept as-is and continue to Step 5 (Implementation Patterns)

---

## Implementation Patterns & Consistency Rules

_Nine patterns refined through two Party Mode rounds: Round 1 with Winston (Architect), Amelia (Developer), Murat (Test Architect), Dr. Quinn (Problem Solver); Round 2 with Winston, Barry (Quick Flow Dev), Paige (Tech Writer), Dr. Quinn. Revisions from each round are marked where applicable._

### TL;DR — The Three Rules That Apply to Every Story

If you read nothing else in this section, remember these:

1. **Argv-only subprocess calls.** Never join user-controlled strings into a shell string; use `Command::new(…).args([…])`. Users who need shell features write `command: ["sh", "-c", "…"]` explicitly — that shell's security is theirs.
2. **Synchronous emit-before-IO.** Every state-changing action calls `session.append(evt).await?` on the same `.await` chain as the subsequent external IO. Never `tokio::spawn` the emit. (Exempt: read-only queries and cross-session reconciliation — see Event Emission section.)
3. **No `Mutex` across `.await`.** All types crossing task boundaries must be `Send + Sync`. Use `Arc<AtomicBool>` or `tokio::sync::Mutex` when coordination is unavoidable.

Everything else in this section is scoped to specific stories, crates, or API surfaces. If the rule doesn't mention your current work, it likely doesn't apply.

### Pattern Categories Defined

**Critical Conflict Points Identified:** 9 areas where AI agents implementing this PRD could diverge and produce incompatible code. Foundational patterns (workspace layout, `thiserror`/`anyhow` split, `#[async_trait]`, `#[non_exhaustive]`, `MockLifecycle` base pattern) are already locked by the existing engine; the rules below govern the *new* code landing from this PRD.

### Naming Patterns

**Rust Identifiers (inherited, reaffirmed):**

- Modules/functions/fields: `snake_case`
- Types/enums/traits: `PascalCase`
- Constants: `SCREAMING_SNAKE_CASE`
- Lifetimes: short lowercase (`'a`, `'ctx`)

**Event Variants (new, D5):**

- PascalCase noun-phrase with action suffix: `StepTimeoutFired`, `MergeAttempted`, `BranchCreated`, `WorkspacePrepared`
- Tense rule: past-tense verb for completed actions (`Fired`, `Created`, `Prepared`); past-participle for attempted-but-unresolved (`Attempted`, `Received`)
- ✅ `SignalReceived { signal: String }` ✅ `IdleTimeoutFired { step_index, idle_threshold_ms }`
- ❌ `TimeoutEvent` (noun-only, drops the action); ❌ `OnSignal` (JS-style event-handler naming)

**Error Variants (new, D9):**

- `StepFailed { step_index, reason: TerminationReason }` is the ONLY termination error; agents must NOT add sibling variants like `StepTimeout` or `StepCancelled` at the top level
- `TerminationReason` variants are plain PascalCase: `StepTimeout`, `IdleTimeout`, `Cancelled`, `SignalReceived(String)`, `Other(String)`
- **Parse-error variant (new, party mode Round 3):** `EngineError::InvalidWorkflowField { path: String, got: String, expected: &'static str }` is the unified workflow-parse-error variant. All parse-time validation failures (duration strings, env keys, env values, enum strings) map into this variant with a best-effort `path` (e.g., `steps[2].env.FOO`, `steps[0].timeout`) and a stable `expected` description. Agents MUST NOT add sibling variants like `InvalidEnvKey`, `InvalidEnvValue`, `InvalidTimeout` — taxonomic sprawl is worse than one variant with rich fields.

**`Other(String)` discipline (both `TerminationReason::Other` and `SandboxError::Other`, new, party mode Round 3):**

- **Stored form:** raw UTF-8-lossy of external output, truncated at 8 KiB at a UTF-8 char boundary. Preserved verbatim as an internal diagnostic. PostgreSQL storage is NOT a security boundary.
- **Displayed form:** every path from a stored `Other(String)` to a human/API surface MUST route through `sanitize_human` at the display boundary (see Display Boundary Sanitization below). The raw stored form MUST NEVER reach a terminal, log line, rendered event-payload field, or CLI error printout without sanitization.
- **Construction:** only permitted inside `*_errors.rs` classifier modules (`docker_errors.rs`, `git_errors.rs`, `workspace_errors.rs`). Call sites elsewhere MUST map external output through the classifier first; a classifier returning `Other(raw)` is the single escape hatch for unclassified output.
- **Reviewer checklist:** new `Other(...)` construction outside a classifier module is a review-stop. New display of `Other(...)` without `sanitize_human` is a review-stop.
- Future: known-secret redaction (API keys, tokens, passwords by regex) tracked as a separate ADR.

**Database Naming (inherited):**

- Table names: `snake_case` plural (`sessions`, `session_events`)
- Column names: `snake_case` (`session_id`, `created_at`)
- No new tables in MVP; schema evolution via JSONB payload columns

**YAML Field Naming (new conventions):**

- All fields: `snake_case` matching Rust field names
- New fields: `timeout:` (duration string, see Timeout Field Format), `idle_timeout:` (duration string, see Timeout Field Format), `env:` (`HashMap<String, String>`, see Env Var Key/Value Validation), `branch_strategy:` (enum string), `branch_name:` (string)
- Enum values in YAML: `snake_case` strings (`head`, `merge_to_head`, `named_branch`) — NOT PascalCase, NOT kebab-case

### Structure Patterns

**Crate Boundaries (locked):**

- New code lands in existing crates only (PRD constraint: no new crates).
- `WorkspaceManager` trait → `stepyard-sandbox-orchestrator` (NOT `stepyard-core`)
- **Startup reconcile → `src/startup.rs` (workspace-root binary, not a crate)** — _revised by party mode; path corrected in Step 6._ Rationale: `reconcile()` orchestrates across PostgreSQL (session query), Docker (container ps), and git (worktree prune); the binary is the natural assembly point where concrete types (`DockerLifecycle`, `GitWorktreeManager`, PG pool) are constructed. Putting it in `stepyard-harness` would force threading abstract factories through the library layer. _Note: the binary's source lives at `src/` in the workspace root (`[[bin]] name = "stepyard" path = "src/main.rs"` in `Cargo.toml`); there is no `crates/stepyard/` directory._
- Signal handler setup → `stepyard` binary's `main.rs` only

**Module Placement Within Crates:**

- New public types: one file per major trait (`workspace.rs`, `template.rs`)
- Internal helpers: sibling `{module}/internal.rs` or `{module}/_impl.rs`
- Errors co-located with the trait/type they belong to (`WorkspaceError` in `workspace.rs`)

**Test Placement:**

- Unit tests: `#[tokio::test]` in `#[cfg(test)] mod tests` inside each `src/` module
- Integration tests: `crates/*/tests/*.rs` using `assert_cmd` + `tempdir`
- PostgreSQL tests: `#[sqlx::test]` macro (ephemeral DB per test, inherited)
- **Property-based tests (new, party mode, scoped in Round 2):** `proptest` lives in the same crate as the code under test, in a `#[cfg(test)] mod proptest_tests` module. **Required for these specific API surfaces only** — template substitution (`{{KEY}}` preprocessor), env var resolution (`${VAR}` expansion), YAML preprocessor, and CLI argument parsing (`--var KEY=VAL`, `--timeout`, `--branch-strategy`). NOT required for cleanup logic, pure internal helpers, or any function whose inputs are constrained by the caller rather than by the user.
- Security negative-case tests (D6/D7): one file `tests/injection_negative.rs` per crate that handles substitution, with BOTH positive-control cases (argv-passed value is literal) AND negative-control cases (explicit `sh -c` invocation DOES execute, proving the escape hatch is user-owned).

### Format Patterns

**Serde Serialization:**

- Every public enum/struct that crosses a persistence or JSON boundary: `#[serde(rename_all = "snake_case")]` at the type level — NOT per-field `#[serde(rename = "...")]`
- Events: always use `#[derive(Debug, Clone, Serialize, Deserialize)]` + `#[non_exhaustive]` + `#[serde(tag = "kind")]` (existing pattern)
- New variant of `Event`: must include `#[serde(rename_all = "snake_case")]` on struct-variant fields

**Error Messages:**

- `#[error("…")]` strings: lowercase, no trailing punctuation, include context values via `{field}` not `{self.field}`
- ✅ `#[error("step {step_index} failed: {reason}")]`
- ❌ `#[error("Step {step_index} Failed.")]`

**YAML Defaults:**

- New optional fields: `#[serde(default)]` at the field level (preserves backward-compat for existing workflows)
- Default values: `impl Default` for types with multiple fields; inline `#[serde(default = "…")]` only for single-field overrides

**YAML-Safe Template Substitution (new, party mode):**

- Template substitutor (`{{KEY}}` preprocessor) MUST YAML-escape every substituted value via `serde_yaml::to_string(&value)`, then embed the escaped form into the document. Never raw-string-interpolate user values into YAML text.
- **Why:** A user value containing YAML metacharacters (`---`, `&anchor`, `*alias`, `: `) would otherwise inject YAML structure, bypassing all downstream argv protections.
- ✅ `let escaped = serde_yaml::to_string(&raw_value)?; result.push_str(&escaped);`
- ❌ `result.push_str(&raw_value);`
- **Known issue (Round 2):** `serde_yaml` (dtolnay) was archived in late 2024. The workspace currently uses it (inherited from Engine v2), which makes this a slow-burning tech-debt item. Planned migration to `serde_yml` (maintained fork) or `serde_yaml_ng` is scoped to the Growth phase and will be tracked as a separate ADR. For MVP, `serde_yaml` remains in use; the substitution pattern is independent of the underlying crate choice.

**Timeout Field Format (new, party mode Round 3):**

- YAML `timeout:` and `idle_timeout:` fields are **duration strings**, not numeric milliseconds. Grammar: `duration := segment+; segment := integer unit; unit := "ms" | "s" | "m" | "h"`.
- Parser lives in `stepyard-core/src/duration.rs` with **narrow public API**: `pub use duration::{parse_duration, deserialize_optional, DurationParseError}` — nothing else is re-exported from the module.
- **Strict rules:** units MUST appear in decreasing magnitude order; no whitespace anywhere; no duplicate units; no bare numbers; no fractional values; no negative values; empty string rejected.
- Accepted: `30s`, `500ms`, `10m`, `2h`, `1h30m`, `2h15m30s`, `1m500ms`.
- Rejected (each must produce `EngineError::InvalidWorkflowField`): `30`, `30 s`, `30 seconds`, `1.5h`, `-5s`, `1m30m`, `30s1m`, `""`.
- **`0s` semantics:** a valid-parse zero duration; follows the full timeout path (emit `StepTimeoutFired` → `StepFailed { reason: TerminationReason::StepTimeout { configured_ms: 0 } }`) before any lifecycle call. NOT a sentinel for "no timeout" — absence of the YAML key means no timeout.
- Parse errors wrap into `InvalidWorkflowField { path, got, expected: "duration string like \"30s\" or \"1h30m\"" }`.
- **Rationale for custom grammar (not `humantime_serde`):** `humantime` accepts too much (`"30 seconds"`, `"1 hour 30 minutes"`, fractionals); the workflow schema needs a small, audit-friendly, stable surface that will not silently accept new dialects on crate upgrade.

**Env Var Key/Value Validation (new, party mode Round 3):**

- YAML `env:` field is typed `HashMap<String, String>` at the serde level; non-string keys are rejected by serde before custom validation runs.
- **Key grammar:** `^[A-Za-z_][A-Za-z0-9_]*$` (POSIX-compatible environment variable identifiers).
- **Value grammar:** arbitrary UTF-8 except NUL (`\0`). Leading/trailing whitespace is preserved verbatim.
- Validation failures wrap into `InvalidWorkflowField` with best-effort path (e.g., `steps[2].env.BAD-KEY`) — a custom visitor to yield exact paths is deferred; the default serde path ("env") plus the offending key suffix is acceptable for MVP.
- `expected` strings (stable, for reviewer and grep audit):
  - Key failure: `"env key matching ^[A-Za-z_][A-Za-z0-9_]*$"`
  - Value failure: `"env value (UTF-8 without NUL)"`

**Display Boundary Sanitization (new, party mode Round 3):**

- **API:** `stepyard::display::sanitize_human(input: &str) -> String`. Character-level contract (operates on `char`s after any upstream `String::from_utf8_lossy`):
  - Preserve `\n` (U+000A), `\r` (U+000D), `\t` (U+0009) verbatim.
  - Any other `char::is_control()` char is escaped as `\u{XXXX}` (lowercase hex, zero-padded to min 4 digits).
  - All other printable chars preserved verbatim — including multibyte UTF-8, emoji, and bidi-neutral scripts.
- **Length ceiling:** truncate to 8192 **bytes** at a UTF-8 char boundary; append literal `… [truncated <N> bytes]` where `<N>` is the byte count of the dropped tail.
- **Call site discipline:** `sanitize_human` is invoked at the **display boundary** (CLI error printer, structured-log rendering site, API response serializer) — NOT inside `thiserror::Error` `#[error(...)]` format strings at construction. Keeping sanitization at the output site means the stored error value is the raw diagnostic; only the rendered form is safe.
- **Required tests (stepyard binary test suite):**
  - U+202E RIGHT-TO-LEFT OVERRIDE (bidi attack) — must appear as `\u{202e}` after sanitize.
  - `\x1b[2J\x1b[H<fake-prompt>` (ANSI CSI clear-screen + cursor-home) — `\x1b` must appear as `\u{001b}`.
  - Long input (>8192 bytes) — must terminate with the truncation marker, and the byte count must match the dropped tail exactly.

### Communication Patterns

**Event Emission — synchronous-append-before-IO (revised, party mode):**

- Every new feature that affects session state MUST emit an event via `session.append(evt).await?` on the SAME `.await` chain as the subsequent IO, in the SAME `async fn`. **Never `tokio::spawn` the emit.**
- Emission ordering: **decision → `append(event).await?` → IO action**. Example: decide to cancel → `session.append(SignalReceived {…}).await?` → `lifecycle.destroy(…).await`.
- **Why revised (Round 1):** Original rule ("emit before IO") was intent-correct but mechanism-ambiguous — a future author could write `tokio::spawn(async move { session.append(evt).await })` followed by the IO call, which races. The revision makes the mechanism explicit: append must be in the same future chain as the IO.
- Durability comes from PostgreSQL's commit atomicity (advisory xact lock), not from tokio task scheduling. If PG is unreachable, `append` fails and the IO does not run — which is the correct behavior for session-log-as-truth.
- **Exemption (Round 2):** The rule applies to operations that **mutate a live session's state**. Read-only queries and **cross-session reconciliation (e.g., startup `reconcile()` — see Crash Recovery section)** are exempt. Reconcile runs BEFORE any engine exists, so there is no session to append into; its IO operations are bootstrap-phase and documented separately.

**Cancel Broadcast (D1/D2):**

- `broadcast::Sender<()>` is constructed once in `main()`; passed into every `Engine::new()` via `HarnessConfig::shutdown_rx`
- NEVER use `once_cell::sync::Lazy<…>` or `static` for runtime coordination state — violates session-log-as-truth
- Each engine holds a `broadcast::Receiver<()>` as a field; subscribes during `new()`; selects on `recv()` inside `run_step()`

**Tracing Fields:**

- Structured fields, not format strings: `tracing::info!(session_id = %id, step = step_index, "step started")` NOT `tracing::info!("step {} started for session {}", step_index, id)`
- Field names: `snake_case` matching event field names when possible (correlates logs to events)
- Log levels: `debug` for verbose internal state, `info` for session milestones, `warn` for recoverable issues, `error` for fatal
- **Env-value confidentiality (new, party mode Round 3):** env **values** (the `v` in `env[k] = v`) MUST NOT appear in any `tracing::*!` field, `Event` payload field, or `thiserror::Error` `#[error(...)]` format string. Only env **keys** are permitted to be logged or emitted, as `env_keys: Vec<String>`. A doc comment MUST sit at the docker exec argv construction site: `// env_values cross here: argv only, never logged or event-emitted`. Rationale: env values routinely carry API keys, tokens, and database URLs; the log/event channel is shared with operators and external aggregators (Loki, Datadog) where exposure is out of stepyard's control.

### Process Patterns

**Async Safety (inherited, reaffirmed):**

- All types crossing task boundaries: `Send + Sync`
- NEVER hold `std::sync::Mutex` or `parking_lot::Mutex` across `.await`
- Use `Arc<AtomicBool>` for flags, `tokio::sync::Mutex` if mutex-across-await is unavoidable (it usually is avoidable)
- `async fn` in traits: `#[async_trait]` annotation required (existing pattern; tokio runtime is a given)
- **Public `async fn` futures MUST be `Send` (not `Send + 'static`, new, party mode Round 3):** every public `async fn` in a library crate has a compile-time `assert_send` test verifying the returned future is `Send`. The `'static` bound is deliberately NOT required — it would force callers to clone or own all captured references, which is wrong for APIs that borrow from `&self` or local scopes. The `Send` bound alone is sufficient for crossing task boundaries. See Pattern Enforcement G6 for the compile-time check.

**Error Handling:**

- Library crates (`stepyard-core`, `stepyard-session`, `stepyard-harness`, `stepyard-sandbox-orchestrator`): `thiserror` only; NO `anyhow`
- Binary (`crates/stepyard/src/main.rs`): `anyhow::Result<()>` at the top; propagate library errors via `?`
- Error conversion: `#[from]` on `thiserror` variants; NEVER manual `.map_err()` unless adding context

**Argv-Not-Shell Security Rule (D6/D7, clarified by party mode):**

- Every user-provided string that reaches a subprocess MUST be passed as an argv element, never joined into a shell string at the stepyard layer.
- ✅ `Command::new("docker").args(["exec", "--env", &format!("{k}={v}"), …])`
- ❌ `Command::new("sh").arg("-c").arg(format!("docker exec --env {k}={v} …"))`
- **Explicit shell escape hatch (new, party mode):** Stepyard does NOT provide an implicit shell. Users needing pipes, redirects, or shell expansion write the invocation explicitly in their workflow YAML:
  ```yaml
  command: ["sh", "-c", "ls | grep foo"]
  ```
  The security of what's inside that `sh -c` string is the **user's responsibility**, not stepyard's. This boundary is documented in the workflow schema docs and reinforced by the negative-control test case (see Test Placement).
- Template substitution (`{{KEY}}`) produces YAML-escaped string literals (see YAML-Safe Template Substitution); the YAML parser then splits `command:` into argv elements. No shell layer is introduced by stepyard at any point.
- Every crate with substitution responsibilities has a `tests/injection_negative.rs` with BOTH cases:
  1. User value `$(rm -rf /)` passed through `command: [$VAR]` appears as a literal argv element and does NOT execute (stepyard's guarantee).
  2. User value `$(malicious)` passed through `command: ["sh", "-c", "$VAR"]` DOES execute, proving the escape hatch is user-owned.

**Mock Extension Pattern (enforced by party mode):**

- When extending an existing trait (e.g., `SandboxLifecycle::exec_with_env`): extend the existing `MockLifecycle`, do not create `MockLifecycleV2`.
- Add new fields to the existing `MockLifecycleCall` struct with `#[serde(default)]` for backward compat in test fixtures.
- Preserve call-recording semantics: every trait method records one entry; tests assert on the sequence.
- **Mutation-testing safeguard (new, party mode):** When extending a trait with a default-impl method, the mock's override MUST record the new parameter in `MockLifecycleCall`, AND at least one test MUST assert on that parameter (e.g., `assert_eq!(calls[0].env, expected_env)`). Without this, a default impl that silently drops the parameter passes every existing test.

**Idempotency:**

- Cleanup operations (`destroy_by_session`, `prune_stale`, `finalise_cancel`) MUST tolerate already-clean state
- Check-then-act pattern: query state, act only if needed, tolerate "not found" errors as success
- Log level for idempotent tolerance: `debug` when the state was already clean (expected); `warn` only when the tolerance masks a potentially unexpected condition

### Enforcement Guidelines

**All AI Agents MUST:**

- Run `cargo clippy --workspace --all-targets -- -D warnings` before declaring a story complete
- Write at least one unit test per new public function/method; integration test per user-visible CLI behavior
- Emit a session event via synchronous `append(…).await?` for every state-changing action before performing external IO on the same `.await` chain
- Document the argv-not-shell rule in doc-comments on any new function accepting user-controlled strings
- For any new trait method with default impl: ensure the mock override records the new parameter, AND write at least one test asserting on it (mutation-resistance)

**Pattern Enforcement:**

- **Workspace-wide lints (Round 3 — corrected after empirical `cargo check` capture):** `[workspace.lints.rust]` in root `Cargo.toml` pins `non_exhaustive_omitted_patterns = "deny"`. Intent: force all consumer crates (`stepyard-core`, `stepyard-harness`, `stepyard-session`, `stepyard-sandbox-orchestrator`, `stepyard` binary) to handle every variant of `#[non_exhaustive]` enums explicitly. **Reality:** the lint is unstable on stable Rust (tracking `rust-lang/rust#89554`). On the pinned MSRV toolchain, `cargo check` emits `warning: unknown lint: non_exhaustive_omitted_patterns` and the `deny` level does **not** escalate — `-D warnings` treats `unknown_lints` as a separate category and does not fail the build. The pin only actively fires on nightly.
  - **MSRV (corrected):** Workspace root `Cargo.toml` pins `rust-version = "1.82"` (not 1.75 as a prior revision claimed — the binary uses `Option::is_none_or`, stable since 1.82). The `[workspace.lints]` table itself parses fine from 1.74+, but the lint inside is a no-op on stable regardless of version.
  - **Compensating enforcement (primary, blocking — planned, not yet wired):** pre-merge CI grep audit that every `match` on `Event`, `EngineError`, or `TerminationReason` in consumer crates either lists all variants or has a wildcard arm (`_ =>` or `other =>`). Target home: `.github/workflows/check.yml` (file does not yet exist at the time this revision was written — the only present workflow is `release.yml`). This check should land alongside the Sandcastle PRD implementation; blocking status activates once the workflow lands.
  - **Compensating enforcement (secondary, advisory — planned):** nightly CI lane running `cargo +nightly clippy --workspace --all-targets -- -D warnings`. Non-blocking on merge (nightly churn is not a repo-quality signal) but reviewed on regression — this is the only environment where the deny actually fires today.
  - **Migration path:** when the lint stabilises upstream, drop the grep audit and promote the deny to blocking-on-stable. If MSRV is ever lowered below 1.74, migrate the deny to per-crate `#![deny(non_exhaustive_omitted_patterns)]` in every `lib.rs` and `main.rs` (same nightly-only caveat applies until stabilisation).
- **Clippy:** workspace `Cargo.toml` pins deny list (`-D warnings` includes `clippy::all`).
- **Coverage:** `cargo llvm-cov --workspace --fail-under-lines 70` in CI. Line coverage, not branch (branch coverage on Rust is experimental and flaky).
- **Security tests:** each crate with substitution has a `tests/injection_negative.rs` file with both positive- and negative-control cases; missing-file pre-merge grep check is _planned_ (not yet wired — same `.github/workflows/check.yml` landing as the lint audit above). Until the CI lands, the rule is reviewer-enforced.
- **Time-determinism in tests (new, party mode — split into 7a/7b in Round 2):**
  - **Rule 7a — In-process tokio tests:** Timing-sensitive `#[tokio::test]` / `#[cfg(test)]` blocks use `tokio::time::pause()` + `advance()` or deterministic handshakes instead of wall-clock sleeps. `tokio::time::sleep(…)` is **banned** inside integration-test files. _Enforcement status: `.github/workflows/check.yml` runs the blocking `scripts/audit-patterns.sh` Rule7a grep over `**/tests/**/*.rs`._ Rationale: real sleep produces flaky tests and wastes CI wall-clock; virtual time/handshakes are deterministic and fast.
  - **Rule 7b — Out-of-process `assert_cmd` tests:** Integration tests that shell out via `assert_cmd` cannot use `tokio::time::pause()` because the subprocess has its own runtime. These tests MUST specify `.timeout(Duration::from_secs(N))` on every `Command` — a deliberate ceiling that fails loud if exceeded. Never invoke `.output()` or `.status()` without a `.timeout()`.
- **CI audit checks (new, party mode Round 3 — planned home `.github/workflows/check.yml`):** The following rules convert reviewer-enforced discipline into pre-merge audits. Each audit name (G1–G6) is stable so story tickets and review comments can reference them. **Honesty caveat:** G1/G2/G3/G5 are ripgrep-based and will miss exotic call shapes (macro-expanded tracing calls, runtime-constructed argv, re-exported aliases); they are tier-A checks that catch the common drift cases, not a proof. G4 is a Rust `cargo xtask` binary with real lexing (via `syn`) and is authoritative for what it covers.
  - **G1 — Env-value leak guard (ripgrep, blocking):** pattern `rg -U --pcre2 'tracing::(info|warn|error|debug|trace)(_span)?!\s*\([^)]*(\benv\s*=|[?%]env\b)'`. Catches `env = %env_map`, `env = ?env`, and the `?env` / `%env` shorthand inside tracing macros. Any match is a block.
  - **G2 — Secret field guard (ripgrep, blocking):** scoped to tracing macros only — pattern `rg -U --pcre2 'tracing::(info|warn|error|debug|trace)(_span)?!\s*\([^)]*\b(api_key|secret|token|password|credential)\b\s*=\s*%'`. The `%` Display-format prefix on those field names is a block; the `?` Debug-format prefix is also flagged by a sibling pattern. Scoping to `tracing::*!` invocations prevents false positives on unrelated `api_key =` variable assignments.
  - **G3 — Shell-string joining guard (ripgrep, blocking):** pattern `rg -U --pcre2 'Command::new\(\s*"(sh|bash|zsh|dash)"\s*\)[\s\S]{0,200}\.arg\(\s*"-c"\s*\)'`. Allowlist: `**/injection_negative.rs` (the negative-control test intentionally invokes the shell). Any other match is a block.
  - **G4 — Emit-before-IO order (cargo xtask, blocking):** `cargo xtask audit-emit-before-io` is a Rust binary that parses the harness crate with `syn`, walks each `async fn` AST, and verifies that for every `self.lifecycle.*.await` call there is a preceding `self.emit(Event::…)` or `self.session.append(…).await?` on the same function. Scope is currently the `crates/stepyard-harness` `lifecycle` field. Coverage explicitly includes `cancel` and `finalize` paths (not only the happy path). Low-level wrapper functions whose caller owns the persistence boundary may opt out only with an explicit `// audit: emit-before-io exempt - reason: ...` comment near the await site. Future `session.append_many(...)`, `emit_if(...)`, or auto-discovered lifecycle-field support will extend the xtask recognizer list.
  - **G5 — `Other(...)` construction outside classifier (ripgrep, blocking):** pattern `rg -U --pcre2 '(TerminationReason|SandboxError)::Other\s*\('`. Allowlist: files whose paths match `*_errors.rs` in each crate (the classifier modules). Any other match is a block.
  - **G6 — Send-future compile check (per-crate compile-time test, blocking):** each library crate has a `#[cfg(test)] mod compile_asserts` with `fn assert_send<T: Send>(_: T) {}` and one line per public `async fn` (e.g., `assert_send(Engine::run(&mut engine, ctx));`). Per-crate boilerplate is accepted; auto-generation is deferred. `'static` is deliberately NOT asserted — see Async Safety above. A compile failure here blocks the build.

### Pattern Examples

**Good Examples:**

```rust
// Event variant — D5 compliant
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct IdleTimeoutFired {
    pub step_index: u32,
    pub idle_threshold_ms: u64,
}

// Error variant — D9 compliant
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("step {step_index} failed: {reason}")]
    StepFailed { step_index: u32, reason: TerminationReason },
}

// Argv-only subprocess call — D6/D7 compliant
let output = Command::new("docker")
    .arg("exec")
    .args(env.iter().flat_map(|(k, v)| ["--env".into(), format!("{k}={v}")]))
    .arg(&container_id)
    .args(&cmd)
    .output().await?;

// Synchronous emit-before-IO — party-mode compliant
async fn cancel_step(&mut self, signal: &str) -> Result<(), EngineError> {
    self.session
        .append(Event::SignalReceived { signal: signal.into() })
        .await?;                                    // synchronous append
    self.lifecycle.destroy(&self.sandbox_id).await  // same .await chain
        .map_err(EngineError::from)
}

// YAML-safe template substitution — party-mode compliant
use std::collections::HashMap;

use stepyard_core::template::substitute_workflow_vars;

let vars = HashMap::from([("BRANCH_NAME".to_string(), "feat/work".to_string())]);
let rendered = substitute_workflow_vars(
    "branch_name: {{BRANCH_NAME}}\n",
    &vars,
    "workflow.yaml",
)?;

// Time-determinism in tests — Rule 7a compliant (virtual time)
#[tokio::test(start_paused = true)]
async fn step_times_out_at_configured_threshold() {
    let engine = Engine::new(/* elided — see engine.rs:142 for full constructor */);
    tokio::time::advance(Duration::from_secs(301)).await;  // virtual time, <1ms wall
    // assert TerminationReason::StepTimeout appears in session events
}

// Bounded subprocess wait — Rule 7b compliant (out-of-process integration test)
#[test]
fn cli_cancel_cleans_up_container() {
    let output = Command::cargo_bin("stepyard").unwrap()
        .args(["cancel", "--session", "test-session-id"])
        .timeout(Duration::from_secs(10))   // Rule 7b: always bound
        .output().unwrap();
    assert!(output.status.success());
}
```

**Anti-Patterns** _(each annotated with **Why this fails** per Round-2 documentation-quality rule)_:

```rust
// ❌ Flat error variant — breaks D9 (TerminationReason sub-enum)
pub enum EngineError {
    StepTimeout { step_index: u32 },        // Should be TerminationReason variant
    StepCancelled { step_index: u32 },      // Should be TerminationReason variant
}
// Why this fails: every consumer (CLI formatter, replay, subscriber) must
// match on N termination variants separately instead of one StepFailed
// variant with a reason sub-enum. Adding a new termination cause in the
// future forces updating every consumer. Central dispatch on `reason` is
// what the sub-enum buys us.

// ❌ Shell-interpolated command — breaks D6/D7 (argv-not-shell)
let cmd = format!("docker exec --env {}={} {} {}", k, v, container, cmd);
Command::new("sh").arg("-c").arg(cmd).output().await?;
// Why this fails: a user-provided value like `$(rm -rf /)` in `v` or `cmd`
// is interpreted by `sh` as a subshell expansion and executes arbitrary code
// on the host — command injection. argv elements are NOT shell-interpreted.

// ❌ Mutex across .await — violates async safety
let guard = self.state.lock().unwrap();
self.lifecycle.exec(/* elided — see engine.rs:247 */).await?;  // guard still held!
// Why this fails: the future becomes !Send (std::MutexGuard is !Send), so
// the tokio runtime cannot move it across threads. Prior bug in this
// codebase: Mutex<Option<Instant>> made Engine futures !Send. Use
// Arc<AtomicBool> or tokio::sync::Mutex instead.

// ❌ Spawned emit races with subsequent IO — breaks synchronous-append rule
tokio::spawn({
    let session = self.session.clone();
    async move {
        session.append(Event::SignalReceived {
            signal: "SIGTERM".into(),
        }).await
    }
});
self.lifecycle.destroy(&self.sandbox_id).await?;  // may run before append commits
// Why this fails: the spawned emit task and the destroy() call race.
// destroy() may complete (and even return success to the caller) before
// the append task reaches PostgreSQL. On crash at exactly that moment,
// session replay sees no SignalReceived event — the log lies about what
// happened. Synchronous append via `.await?` before destroy guarantees
// the event is durable before any observable IO side-effect.

// ❌ Raw-interpolated template substitution — enables YAML injection
let result = doc.replace("{{KEY}}", user_value);
// Where user_value = "foo\n---\nmalicious_step:\n  command: [evil]"
// Why this fails: the user value contains YAML structure characters. After
// replacement, the document now has a second YAML document boundary (`---`)
// and a new top-level key. The YAML parser sees TWO documents or an altered
// structure, bypassing every downstream argv-only protection. Always
// `serde_yaml::to_string(&user_value)` first — it quotes/escapes appropriately.

// ❌ Real sleep in tests — flaky, slow, non-deterministic (Rule 7a violation)
#[tokio::test]
async fn timeout_test() {
    tokio::time::sleep(Duration::from_secs(300)).await;  // banned
}
// Why this fails: CI wastes 5 minutes of wall-clock per test; if the CI
// runner is under load the real timeout may not fire in time and the test
// flakes. `tokio::time::pause()` + `advance()` simulates time in <1ms.
```

---

## Patterns Menu

You have reviewed the 9 revised implementation patterns (with 8 party-mode revisions). Choose:

- **[A]** Advanced Elicitation — explore additional consistency patterns
- **[P]** Party Mode again — further multi-agent review
- **[C]** Continue to Step 6 (Project Structure)

---

## Project Structure & Boundaries

### Workspace Layout Overview

This is a Rust cargo workspace with a hybrid layout inherited from Engine v2:

- **Root package** (`/minion-engine`) is both a library AND the `stepyard` binary. Contains the legacy monolith (`src/`) plus the binary entry point (`src/main.rs`).
- **Four decomposed crates** under `/minion-engine/crates/` hold the Engine v2 contracts being extracted from the monolith.
- **No new crates** are added by this PRD (hard constraint). All new code lands in the 4 existing crates or in `src/`.

**Binary entry point clarification:** The `stepyard` binary lives at `src/main.rs` at the workspace root, NOT at `crates/stepyard/src/main.rs`. The implementation patterns section's wording around startup/signal-handler location has been corrected to `src/startup.rs` (see Known Corrections table below).

### Complete Project Directory Structure

```
minion-engine/                                  # workspace root (also the `stepyard` binary crate)
├── Cargo.toml                                  # workspace + root-package manifest
│   # NEW: [workspace.lints.rust] — non_exhaustive_omitted_patterns = "deny"
│   # NEW: [workspace.lints.clippy] — explicit deny list
├── Cargo.lock
├── ARCHITECTURE.md                             # Engine v2 architecture (existing)
├── README.md
├── Dockerfile.sandbox
├── docker-compose.yml
├── .env.example                                # NEW: document env dict defaults
├── .stepyard/                                    # NEW (at user's repo, not this repo): workspace-scoped config
│   ├── defaults.yaml                           # NEW: default env vars, workspace_retention_hours
│   └── workspaces/                             # NEW: git worktree storage (D8)
│       └── <session-id>-<short-hash>/
├── workflows/                                  # existing YAML workflow templates
│   └── *.yaml                                  # NEW fields: timeout, idle_timeout, env, branch_strategy, branch_name
├── prompts/                                    # existing
├── docs/                                       # existing
├── src/                                        # WORKSPACE ROOT BINARY + LIBRARY (legacy monolith)
│   ├── main.rs                                 # binary entry — MODIFIED: install signal handler, call startup::reconcile()
│   ├── lib.rs                                  # library re-exports
│   ├── startup.rs                              # NEW (per D1/D2/Crash Recovery): reconcile() orchestrator
│   │   # - queries sessions WHERE status='running'
│   │   # - docker ps --filter name=stepyard-session-*
│   │   # - calls WorkspaceManager::prune_stale()
│   ├── signal.rs                               # NEW: broadcast::Sender<()> setup, SIGINT/SIGTERM installer
│   ├── cli/
│   │   ├── mod.rs
│   │   ├── commands.rs                         # MODIFIED: add `stepyard session list` subcommand (FR22)
│   │   ├── display.rs                          # MODIFIED: render new Event variants (StepTimeoutFired, etc.)
│   │   ├── harness_adapter.rs                  # MODIFIED: thread shutdown_rx into Engine::new()
│   │   ├── remote.rs                           # existing (SSH remote exec)
│   │   ├── session_setup.rs
│   │   ├── init_templates.rs
│   │   └── setup.rs
│   ├── config/
│   │   ├── mod.rs
│   │   ├── defaults.rs                         # MODIFIED: load .stepyard/defaults.yaml with env map
│   │   ├── manager.rs                          # MODIFIED: expose env_defaults, workspace_retention_hours
│   │   └── merge.rs                            # MODIFIED: cascade resolution (step > workflow > defaults > host)
│   ├── engine/                                 # LEGACY engine (being phased out; some code retained for Growth)
│   │   ├── mod.rs
│   │   ├── context.rs
│   │   ├── state.rs
│   │   └── template.rs                         # LEGACY template code (new substitution goes to stepyard-core)
│   ├── events/
│   │   ├── mod.rs
│   │   ├── subscribers.rs                      # MODIFIED: handle new Event variants (workspace lint forces coverage)
│   │   └── types.rs
│   ├── sandbox/
│   │   ├── mod.rs
│   │   ├── config.rs
│   │   ├── docker.rs                           # LEGACY docker code (being replaced by crates/stepyard-sandbox-orchestrator)
│   │   └── proxy.rs
│   ├── steps/                                  # step type implementations (existing, mostly untouched)
│   │   ├── mod.rs
│   │   ├── agent.rs                            # MODIFIED: propagate timeout + env to executor
│   │   ├── call.rs
│   │   ├── chat.rs
│   │   ├── cmd.rs                              # MODIFIED: argv-not-shell enforcement
│   │   ├── gate.rs
│   │   ├── map.rs
│   │   ├── parallel.rs
│   │   ├── repeat.rs
│   │   ├── script.rs
│   │   └── template_step.rs
│   ├── workflow/
│   │   ├── mod.rs
│   │   ├── parser.rs                           # MODIFIED: YAML-safe template substitution, placeholder detection
│   │   ├── schema.rs                           # MODIFIED: add timeout, idle_timeout, env, branch_strategy fields
│   │   └── validator.rs                        # MODIFIED: validate placeholder completeness pre-execution
│   ├── prompts/                                # existing prompts system
│   ├── plugins/                                # existing plugin loader
│   ├── claude/                                 # existing Claude integration
│   ├── slack/                                  # optional --features slack
│   ├── control_flow.rs
│   └── error.rs
├── tests/                                      # workspace integration tests
│   ├── fixtures/
│   │   └── prompts/
│   ├── injection_negative.rs                   # NEW (per patterns): template/env injection negative tests
│   ├── signal_handling.rs                      # NEW (FR5-FR8): SIGINT/SIGTERM integration tests (Rule 7b .timeout())
│   ├── startup_reconcile.rs                    # NEW (Crash Recovery): orphan container/session/worktree reconciliation
│   └── branch_strategy.rs                      # NEW (FR13-FR18, Growth): worktree + branch strategy E2E
│
├── crates/
│   ├── stepyard-core/                            # IO-free contracts
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── error.rs                        # MODIFIED: add TerminationReason sub-enum (D9), StepFailed variant
│   │   │   ├── event.rs                        # MODIFIED: add 8 new Event variants (D5)
│   │   │   ├── subscriber.rs
│   │   │   ├── workflow.rs                     # MODIFIED: add timeout/env/branch_strategy fields with #[serde(default)]
│   │   │   └── template.rs                     # NEW (FR19-FR21): {{KEY}} preprocessor, YAML-safe substitution
│   │   └── tests/
│   │       ├── contract.rs                     # existing
│   │       └── template_proptest.rs            # NEW: proptest for template substitution (required per patterns)
│   ├── stepyard-session/
│   │   ├── Cargo.toml
│   │   ├── migrations/                         # existing SQL migrations (no schema changes for MVP)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── session.rs                      # MODIFIED: append supports new event variants (no API change)
│   │   │   └── store.rs                        # existing PG store
│   │   └── tests/
│   │       ├── integration.rs
│   │       └── types.rs
│   ├── stepyard-sandbox-orchestrator/
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── sandbox.rs                      # SandboxLifecycle trait — MODIFIED: add exec_with_env default-impl method (D3)
│   │   │   ├── docker.rs                       # DockerLifecycle — MODIFIED: override exec_with_env with --env K=V
│   │   │   ├── local.rs                        # LocalLifecycle (no sandbox)
│   │   │   ├── mock.rs                         # MockLifecycle — MODIFIED: record env in MockLifecycleCall
│   │   │   ├── docker_errors.rs                # NEW (D9): stderr classifier → SandboxError variants
│   │   │   └── workspace.rs                    # NEW (D4, FR13-FR18): WorkspaceManager trait + GitWorktreeManager impl
│   │   └── tests/
│   │       ├── lifecycle.rs
│   │       ├── workspace.rs                    # NEW: GitWorktreeManager lifecycle tests
│   │       └── injection_negative.rs           # NEW: env value injection negative tests
│   └── stepyard-harness/
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs
│       │   ├── engine.rs                       # MODIFIED: fix finalise_cancel bug (line 413-416); add tokio::time::timeout; subscribe to shutdown_rx; env resolution in prepare_step
│       │   ├── executor.rs                     # MODIFIED: accept env HashMap; propagate to SandboxLifecycle::exec_with_env
│       │   └── workflow.rs
│       └── tests/
│           ├── concurrent_sessions.rs
│           ├── step_resume.rs
│           ├── step_timeout.rs                 # NEW (FR1-FR4): timeout enforcement tests using tokio::time::pause
│           └── cancel_cleanup.rs               # NEW (MVP bug fix verification): finalise_cancel correctness
│
├── Formula/                                    # existing (homebrew)
├── packages/                                   # existing Dashboard frontend/backend (NOT in scope for this PRD)
│   ├── api/
│   └── web/
└── target/                                     # cargo build output (gitignored)
```

### Architectural Boundaries

**Crate Boundary: `stepyard-core`**
- Contains: Event, Subscriber trait, EngineError + TerminationReason, WorkflowDef, WorkspaceSpec, template substitution
- Forbidden deps: `tokio`, `sqlx`, `reqwest`, `async_trait`, any IO library
- Allowed deps: `serde`, `serde_yaml`, `thiserror`, `uuid`, `chrono`
- **New surface area (this PRD):** `template.rs` module + 8 new Event variants + TerminationReason sub-enum + 3 new workflow fields

**Crate Boundary: `stepyard-session`**
- Contains: Session, SessionEvent, SessionId, PG append-only store
- Depends on: `stepyard-core`, `sqlx`, `tokio`
- **New surface area (this PRD):** None (new event variants flow through existing JSONB payload; no API or schema change)

**Crate Boundary: `stepyard-sandbox-orchestrator`**
- Contains: SandboxLifecycle trait, DockerLifecycle, MockLifecycle, LocalLifecycle, WorkspaceManager trait + GitWorktreeManager, docker_errors classifier
- Depends on: `stepyard-core`, `tokio`, `async_trait`
- Forbidden deps: `sqlx` (sandbox has no PG concerns)
- **New surface area (this PRD):** `workspace.rs` module, `docker_errors.rs` classifier, `exec_with_env` default-impl method on `SandboxLifecycle`

**Crate Boundary: `stepyard-harness`**
- Contains: Engine (step/resume/cancel), HarnessConfig, StepExecutor trait, CancelToken
- Depends on: `stepyard-core`, `stepyard-session`, `stepyard-sandbox-orchestrator`, `tokio`
- **New surface area (this PRD):** `tokio::time::timeout` wrapping in executor; env resolution in `prepare_step`; `shutdown_rx: broadcast::Receiver<()>` field on `HarnessConfig`; bug fix to `finalise_cancel`

**Binary Boundary: root package `minion-engine` (`src/`)**
- Contains: CLI commands, legacy step types (being phased out), signal handler, startup reconcile, workflow parser, prompts, plugins, sandbox/docker legacy code, Slack bot (optional feature)
- Depends on: ALL four crates + `anyhow` (only here)
- **New surface area (this PRD):** `startup.rs` (reconcile orchestrator), `signal.rs` (broadcast + SIGINT/SIGTERM handlers), modifications to `cli/commands.rs` (session list), `cli/display.rs` (new event rendering), `workflow/parser.rs` (YAML-safe substitution), `steps/cmd.rs` (argv-not-shell enforcement)

**Trait-Boundary Rules:**
- `WorkspaceManager` lives in `stepyard-sandbox-orchestrator` (D4), NOT `stepyard-core`, because it is IO-bound
- `Event` lives in `stepyard-core` (inherited) — every new variant must be added there, not in consumer crates
- Startup reconciliation orchestrator lives in the binary (`src/startup.rs`), NOT any crate — it composes trait objects from multiple crates

### Requirements to Structure Mapping

**FR1-FR4 — Step Execution Safety (MVP):**
- `crates/stepyard-harness/src/engine.rs` — timeout enforcement (`tokio::time::timeout`), fix `finalise_cancel` bug (line 413-416)
- `crates/stepyard-core/src/event.rs` — `StepTimeoutFired`, `IdleTimeoutFired` variants
- `crates/stepyard-core/src/error.rs` — `TerminationReason::StepTimeout`, `TerminationReason::IdleTimeout`
- `crates/stepyard-harness/tests/step_timeout.rs` — timeout enforcement tests (virtual time)
- `crates/stepyard-harness/tests/cancel_cleanup.rs` — MVP bug fix verification

**FR5-FR8 — Process Lifecycle (MVP):**
- `src/main.rs` — install `signal.rs` handlers; subscribe engines to `shutdown_rx`
- `src/signal.rs` — `broadcast::Sender<()>` construction, SIGINT/SIGTERM handlers
- `src/startup.rs` — reconcile() (crash recovery orphan cleanup)
- `crates/stepyard-harness/src/engine.rs` — subscribe to `shutdown_rx`; select on `recv()` during step execution
- `crates/stepyard-core/src/event.rs` — `SignalReceived` variant
- `tests/signal_handling.rs` — integration tests for SIGINT/SIGTERM (`assert_cmd` with `.timeout()`)
- `tests/startup_reconcile.rs` — crash-recovery orphan tests

**FR9-FR12 — Sandbox Environment (MVP):**
- `crates/stepyard-sandbox-orchestrator/src/sandbox.rs` — add `exec_with_env` default-impl method (D3)
- `crates/stepyard-sandbox-orchestrator/src/docker.rs` — override `exec_with_env` using `docker exec --env K=V`
- `crates/stepyard-sandbox-orchestrator/src/mock.rs` — override + record env in `MockLifecycleCall`
- `crates/stepyard-harness/src/engine.rs` — env resolution in `prepare_step`
- `crates/stepyard-harness/src/executor.rs` — accept env `HashMap`, propagate to lifecycle
- `src/config/merge.rs` — cascade resolution (step > workflow > defaults > host)
- `crates/stepyard-sandbox-orchestrator/tests/injection_negative.rs` — env injection negative cases

**FR13-FR18 — Git Workspace Management (Growth):**
- `crates/stepyard-sandbox-orchestrator/src/workspace.rs` — `WorkspaceManager` trait + `GitWorktreeManager` impl (D4)
- `crates/stepyard-core/src/event.rs` — `BranchCreated`, `MergeAttempted`, `MergeConflict`, `WorkspacePrepared`, `WorkspacePruned`
- `crates/stepyard-core/src/workflow.rs` — `branch_strategy:` + `branch_name:` fields
- `crates/stepyard-harness/src/engine.rs` — invoke `WorkspaceManager::prepare` before first step, `finalize` after last step
- `src/startup.rs` — call `prune_stale()` during reconcile
- `tests/branch_strategy.rs` — E2E for Head/MergeToHead/NamedBranch strategies

**FR19-FR21 — Workflow Configuration (Growth):**
- `crates/stepyard-core/src/template.rs` — `{{KEY}}` preprocessor, YAML-safe substitution
- `src/workflow/parser.rs` — integrate template.rs before YAML parse
- `src/workflow/validator.rs` — placeholder completeness check
- `src/cli/commands.rs` — `--var KEY=VAL` CLI flag parsing
- `crates/stepyard-core/tests/template_proptest.rs` — proptest for substitution (required per patterns)

**FR22-FR24 — Session Observability (Growth):**
- `crates/stepyard-core/src/event.rs` — (all 8 new variants already covered above)
- `src/events/subscribers.rs` — handle new variants (workspace lint forces coverage)
- `src/cli/commands.rs` — `stepyard session list --status` subcommand
- `src/cli/display.rs` — render new event variants in terminal output

**FR25-FR27 — Provider Extensibility (Expansion):**
- `crates/stepyard-sandbox-orchestrator/src/` — future `podman.rs`, cloud provider modules (out of MVP scope)
- Current `SandboxLifecycle` trait supports this without refinement for MVP

### Integration Points

**Internal Communication (within process):**
- Crates communicate ONLY via `stepyard-core` contracts — no direct inter-crate dependencies except through the root binary
- Cancel broadcast: `main()` owns `broadcast::Sender<()>`; engines hold `broadcast::Receiver<()>` subscribed at construction
- Session log: every state-changing action appends via `session.append(evt).await?` synchronously (emission rule)

**External Integrations:**
- **PostgreSQL** — via `sqlx` in `stepyard-session` only; `DATABASE_URL` env var required
- **Docker daemon** — via `tokio::process::Command` subprocess in `stepyard-sandbox-orchestrator/src/docker.rs` only (no bollard, no embedded client)
- **Git 2.30+** — via `tokio::process::Command` subprocess in `stepyard-sandbox-orchestrator/src/workspace.rs` only (no libgit2)
- **Host signals** — via `tokio::signal::unix` in `src/signal.rs` only
- **Host environment** — read only for `${VAR}` resolution in `src/config/merge.rs`; never passed through wholesale

**Data Flow — Step Execution Happy Path:**
1. `stepyard run workflow.yaml --var KEY=VAL` → `src/cli/commands.rs`
2. `src/workflow/parser.rs` → template substitute → parse → `WorkflowDef` (in `stepyard-core`)
3. `src/cli/harness_adapter.rs` → construct `Engine` (in `stepyard-harness`) with `shutdown_rx`
4. `Engine::run_step` → append `StepStarted` event → `prepare_step` resolves env → `executor.exec_with_env(…)` → `DockerLifecycle` runs container → return `ExecOutput`
5. Timeout wraps step: `tokio::time::timeout(configured_ms, exec_future)` → on timeout: append `StepTimeoutFired` + `StepFailed{reason: StepTimeout}` → destroy container
6. On successful step: append `StepCompleted` → loop to next step

**Data Flow — Signal Received:**
1. `SIGTERM` → `src/signal.rs` handler → `broadcast::Sender::send(())`
2. Each `Engine` receiver fires → aborts current `.await` via `select!`
3. Engine calls `finalise_cancel` → append `SignalReceived { signal: "SIGTERM" }` → `lifecycle.destroy(…)` (using ACTUAL session ID after bug fix)
4. `main()` waits up to `shutdown_grace_s` → exits with code 143

**Data Flow — Startup Crash Recovery:**
1. `src/main.rs` → call `startup::reconcile(pg_pool, lifecycle, workspace_manager)`
2. `reconcile` queries `SELECT id FROM sessions WHERE status='running'`
3. For each, calls `docker ps --filter name=stepyard-session-<id>`; if missing, append `SignalReceived { signal: "crash_recovery" }` and transition status='failed'
4. Destroy any container not matching a running session (orphan)
5. Call `WorkspaceManager::prune_stale(retention_hours)` for old worktrees

### File Organization Patterns

**Configuration Files:**
- Workspace-level: `Cargo.toml` (root), `Cargo.lock`, `.env.example`
- Application-level (at USER's repo, not this repo): `.stepyard/defaults.yaml` (env defaults, `workspace_retention_hours`)
- CI: `.github/workflows/` (existing)

**Source Organization:**
- **New code goes in existing crates** (PRD constraint) — never create a new crate
- **Errors co-located with types** — `WorkspaceError` in `workspace.rs`, not a separate `workspace_errors.rs`
- **One file per major trait** — `WorkspaceManager` → `workspace.rs`
- **Internal helpers** — sibling `_impl.rs` files when implementation grows beyond a single file

**Test Organization:**
- **Unit tests:** `#[cfg(test)] mod tests` inside each `src/` module (inherited)
- **Proptest:** `#[cfg(test)] mod proptest_tests` inside the module, OR in a dedicated `tests/*_proptest.rs` file for shared fixtures
- **Integration tests per crate:** `crates/*/tests/*.rs`
- **Workspace-level integration tests:** `/tests/*.rs` (root) — for cross-crate E2E like signal handling, reconciliation, branch strategies
- **Negative-control security tests:** `tests/injection_negative.rs` (workspace-level OR per-crate where substitution happens)

**Asset Organization (unchanged):**
- Workflows: `workflows/*.yaml` (user-editable templates)
- Prompts: `prompts/<category>/*.md`
- Test fixtures: `tests/fixtures/prompts/**/*.md`

### Development Workflow Integration

**Development Server:** N/A (CLI tool, not a server).

**Build Process:**
- `cargo build --release` — default build, no slack feature
- `cargo build --release --features slack` — enables Slack bot (axum + hmac deps)
- `cargo install --path .` → installs as `/usr/local/bin/stepyard`
- Workspace-level checks: `cargo clippy --workspace --all-targets -- -D warnings`
- Coverage: `cargo llvm-cov --workspace --fail-under-lines 70`

**Deployment Structure:**
- Binary: `/usr/local/bin/stepyard` on VPS (per saved memory: "minion-engine deploy topology")
- Runtime dependencies on host: Docker CE 20.10+, git 2.30+, PostgreSQL reachable via `DATABASE_URL`
- No installer; `cargo install --path .` is the deployment mechanism

### Known Corrections from Earlier Steps

During structure mapping, a patterns-doc reference was discovered to be imprecise:

| Location | Original Wording | Corrected Wording | Reason |
|---|---|---|---|
| Implementation Patterns § Crate Boundaries | `crates/stepyard/src/startup.rs` | `src/startup.rs` | The binary lives at the workspace root (`src/main.rs`), not at `crates/stepyard/`. `Cargo.toml` has `[[bin]] name = "stepyard" path = "src/main.rs"` — there is no `crates/stepyard/` directory. **This correction has been applied in-place in the patterns section above.** |

---

## Architecture Validation Results

### Coherence Validation ✅

**Decision Compatibility — all 10 decisions work together:**

- **D1 + D2 + Crash Recovery** — broadcast handles running cancellation; reconcile handles startup orphans; no overlap, no two-sources-of-truth.
- **D3 + Mock Extension Pattern** — `exec_with_env` default-impl is caught by the mutation-safeguard rule (mock must record env + test asserts on it), preventing silent parameter drops.
- **D4 + Startup Reconcile** — `WorkspaceManager` lives in `stepyard-sandbox-orchestrator`; binary composes it with `DockerLifecycle` + PG pool at `src/startup.rs`.
- **D5 + Workspace-wide lint** — new Event variants forced to be handled in subscribers via `non_exhaustive_omitted_patterns = "deny"` at workspace root.
- **D6 + D7 + argv-only rule** — all three rely on the same safety property (argv argument passing); cumulative defense-in-depth.
- **D8 + D4** — pruning at startup calls `WorkspaceManager::prune_stale()`.
- **D9 + D5** — `TerminationReason` carries error classification; `*TimeoutFired` / `SignalReceived` events carry observable facts; clean separation.
- **D10 + D4 + D5** — `branch_strategy:` YAML → `WorkspaceManager` → `BranchCreated`/`MergeAttempted`/`MergeConflict` events.

**Pattern Consistency:**
- Serde `snake_case` uniform across Events, YAML fields, enum values.
- `thiserror`/`anyhow` split respected everywhere (libs use thiserror; only `src/main.rs` uses anyhow).
- Synchronous-append-before-IO rule consistent with session-log-as-truth invariant.
- Test-placement rules align with crate boundaries.

**Structure Alignment:**
- IO-free rule for `stepyard-core` preserved (new `template.rs` uses only stdlib + `serde_yaml`).
- No-new-crates constraint honored (all new files in 4 existing crates or `src/`).
- Binary-orchestration boundary respected (`src/startup.rs` composes trait objects).

### Requirements Coverage Validation ✅

**FR1-FR12 (MVP) — fully covered:**
- FR1-FR4 (Step Execution Safety): D9 `TerminationReason` + `tokio::time::timeout` in `engine.rs` + `finalise_cancel` bug fix
- FR5-FR8 (Process Lifecycle): D1 broadcast + D2 signal handler + Crash Recovery reconcile
- FR9-FR12 (Sandbox Environment): D3 `exec_with_env` + D6 cascade resolution in `src/config/merge.rs`

**FR13-FR24 (Growth) — fully covered:**
- FR13-FR18 (Git Workspace): D4 `WorkspaceManager` + D8 startup pruning + D10 branch_strategy + D5 workspace events
- FR19-FR21 (Workflow Config): D7 template substitution + `stepyard-core/template.rs` + placeholder validation
- FR22-FR24 (Session Observability): D5 all 8 new Event variants + `session list --status` CLI + renderer updates

**FR25-FR27 (Expansion) — architecturally supported:**
- Existing `SandboxLifecycle` trait supports Podman/cloud providers without refinement for MVP. FR27 (TTY forwarding) may require trait refinement in Expansion phase; deferred.

**Non-Functional Requirements — all addressed:**
- Performance (signal <1s, timeout precision <1s, prune <30s/50 worktrees): broadcast pattern + `tokio::time::timeout` + startup reconcile
- Security (env opt-in, values not logged, argv-not-shell): D6/D7 + negative-case tests
- Reliability (crash recovery, idempotent cleanup, deterministic termination): `Session::replay` + `destroy_by_session` idempotency + `TerminationReason`
- Integration (Docker CLI, no schema migration, backward-compat YAML): preserved via `#[serde(default)]` + `#[non_exhaustive]`
- Maintainability (clippy clean, thiserror, no breaking changes): Enforcement Guidelines + additive APIs

### Implementation Readiness Validation ✅

- **Decision completeness:** all 10 decisions + Crash Recovery have rationale, Rust code shape, alternatives rejected
- **Structure completeness:** complete tree; every new file marked; FR → file mapping for all 27 FRs
- **Pattern completeness:** TL;DR (3 rules) + 9 scoped patterns + 6 good examples + 6 anti-patterns (all with `Why this fails:`)
- **Enforcement mechanisms:** workspace-wide lints, clippy, coverage gate, grep checks for banned patterns, negative-case test files

### Gap Analysis Results

**Critical Gaps:** None found. Architecture is ready for story creation.

**Important Gaps (3 — clarifications added inline; no decisions changed):**

1. **Legacy `DockerLifecycle::exec` uses `sh -c` (`crates/stepyard-sandbox-orchestrator/src/docker.rs:173`) — conflicts with the argv-not-shell rule.** Clarification: *the argv-only rule applies to NEW subprocess calls introduced by this PRD (`exec_with_env`, workspace git commands, startup reconcile). The legacy `exec(id, cmd)` method retains its `sh -c` shell semantics for backward compat; no breaking changes in MVP. Migration of legacy `exec` to argv-only is a post-MVP refactor and is captured as tech debt.*

2. **Idle timeout detection mechanism unspecified.** Clarification: *idle timeout uses `tokio::process::Command::stdout(Stdio::piped())` + `tokio::io::AsyncBufReadExt::read_until` wrapped in `tokio::time::timeout(idle_threshold_ms)` per read. Reset on every byte received. Fires `IdleTimeoutFired` + `TerminationReason::IdleTimeout` when timer exceeds threshold. Full implementation detail lives in the FR1-FR4 story.*

3. **`session list --status` query source.** Clarification: *`session list --status` is a PostgreSQL query against the `sessions` table's `status` column, NOT an in-memory registry lookup. Consistent with session-log-as-truth: the database is the authoritative view of session state. D1's ban on runtime registries does not preclude DB queries.*

**Nice-to-Have Gaps (3 — deferred to Growth/Expansion):**

1. **Mutation testing (`cargo mutants`)** — deferred to Growth phase as a separate quality-gate ADR
2. **Rate-limiting startup reconcile** — acceptable for MVP (expected orphan count <10); add concurrency limit in future iteration
3. **`serde_yaml` archived crate migration** — already flagged in Round 2 patterns; Growth-phase ADR

### Validation Issues Addressed

The 3 Important Gaps are documented inline as clarifications — **no decisions re-opened**. These are design-detail refinements rather than architectural changes. They will inform story writing in the next workflow phase.

### Architecture Completeness Checklist

**✅ Requirements Analysis**
- [x] Project context thoroughly analyzed (27 FRs across 6 categories + 5 NFR classes)
- [x] Scale and complexity assessed (high; 4 crates modified + 1 new trait)
- [x] Technical constraints identified (no new crates, Docker CLI only, async Send+Sync)
- [x] Cross-cutting concerns mapped (event emission, async safety, backward compat, error taxonomy, test coverage)

**✅ Architectural Decisions**
- [x] 10 core decisions documented with rationale, Rust shape, rejected alternatives
- [x] Technology stack fully specified (Rust 1.82+, tokio, sqlx, serde, clap; no new external crates)
- [x] Integration patterns defined (Docker CLI subprocess, git CLI subprocess, PG via sqlx)
- [x] Performance considerations addressed (broadcast <1s, timeout <1s precision, prune <30s)

**✅ Implementation Patterns**
- [x] TL;DR (3-rule summary) at top for new-contributor orientation
- [x] Naming conventions (Event variants, error variants, YAML fields, DB)
- [x] Structure patterns (crate boundaries, test placement, proptest scoping)
- [x] Communication patterns (synchronous emit-before-IO, broadcast, tracing fields)
- [x] Process patterns (async safety, error handling, argv-only security, mock extension, idempotency)
- [x] Enforcement mechanisms (workspace lints, coverage gate, clippy, time-determinism rules)

**✅ Project Structure**
- [x] Complete directory tree with all NEW/MODIFIED files marked
- [x] Component boundaries defined (4 crates + binary, with forbidden-deps rules)
- [x] Integration points mapped (internal broadcast, PG, Docker, git, signals, env)
- [x] FR → file mapping complete for all 27 FRs

### Architecture Readiness Assessment

**Overall Status:** READY FOR IMPLEMENTATION

**Confidence Level:** HIGH based on validation results.

**Key Strengths:**
- Session-log-as-truth invariant preserved (no runtime registries)
- Brownfield-safe (all additive APIs; no breaking changes to existing traits)
- Security posture explicit (argv-not-shell, YAML-safe substitution, negative-case tests mandated)
- Test discipline encoded (virtual time, bounded subprocess waits, proptest for user input, mutation-safeguard)
- Two rounds of party-mode review surfaced and resolved 13 revisions across decisions + patterns

**Areas for Future Enhancement (post-MVP):**
- `serde_yaml` → `serde_yml` migration (Growth-phase ADR)
- Legacy `DockerLifecycle::exec` → argv-only migration (post-MVP refactor)
- Mutation testing in CI (`cargo mutants`)
- Rate-limiting startup reconcile for high-orphan-count scenarios
- Distributed cancellation across processes (Expansion phase, when cloud providers land)

### Implementation Handoff

**AI Agent Guidelines:**
- Follow the TL;DR 3-rule summary for every story; rules outside the TL;DR are scoped
- Every new Event variant must land in `crates/stepyard-core/src/event.rs`; every new subscriber must handle it (enforced by workspace lint)
- Negative-control tests for substitution/env are MANDATORY in the relevant crates
- The MVP bug fix story (Cancel cleanup, ~3 lines in `stepyard-harness/src/engine.rs:413-416`) is the first implementation target

**First Implementation Priority:**

```bash
# MVP Feature #1 — Cancel cleanup fix (unblocks everything else)
# File: crates/stepyard-harness/src/engine.rs lines 413-416
# Change: pass self.session.id() instead of SandboxId::default()
# Add: test in crates/stepyard-harness/tests/cancel_cleanup.rs
cargo test --workspace cancel_cleanup
cargo clippy --workspace --all-targets -- -D warnings
```
