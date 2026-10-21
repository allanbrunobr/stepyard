---
stepsCompleted: ['step-01-extraction', 'step-02-epics', 'step-03-epic-1', 'step-03-epic-2', 'step-03-epic-3', 'step-03-epic-4', 'step-03-epic-5', 'step-03-epic-6', 'step-04-validation']
status: 'complete'
inputDocuments:
  - _bmad-output/sandcastle-features/prd.md
  - _bmad-output/sandcastle-features/architecture.md
workflowType: 'epics-and-stories'
project_name: 'Stepyard — Sandcastle-Inspired Features'
user_name: 'Bruno'
date: '2026-04-16'
sourceDocSet: 'sandcastle-features'
---

# Stepyard — Sandcastle-Inspired Features - Epic Breakdown

## Overview

This document provides the complete epic and story breakdown for Stepyard — Sandcastle-Inspired Features, decomposing the requirements from the PRD and Architecture into implementable stories. No UX Design document exists for this project (Rust CLI/library — no UI surface).

## Requirements Inventory

### Functional Requirements

**Scope tags:** FR1-FR12 = MVP, FR13-FR24 = Growth, FR25-FR27 = Expansion.

**Step Execution Safety (MVP):**

- **FR1:** The engine can enforce a configurable step timeout (wall-clock) per workflow step, terminating execution and emitting a failure event when the timeout elapses.
- **FR2:** The engine can detect idle steps (no stdout output for a configurable duration) and terminate them independently of the wall-clock step timeout.
- **FR3:** The engine can cancel a running step and destroy the correct sandbox container associated with the active session (not a default/empty container ID).
- **FR4:** The engine can report the reason for step termination (step timeout, idle timeout, cancellation, error) in the session event log.

**Process Lifecycle (MVP):**

- **FR5:** The engine can intercept OS termination signals (SIGTERM, SIGINT) and initiate graceful shutdown of all active sessions before process exit.
- **FR6:** The engine can maintain a registry of active sessions during runtime so that the signal handler knows which containers to destroy.
- **FR7:** The engine can destroy all sandbox containers owned by active sessions during graceful shutdown, tolerating already-destroyed containers.
- **FR8:** The engine can record a `SignalReceived` event in each active session's log before process exit, preserving the reason for cancellation.

**Sandbox Environment (MVP):**

- **FR9:** Workflow authors can declare environment variables per step in workflow YAML, and the engine passes them to the sandbox container at execution time.
- **FR10:** Workflow authors can declare default environment variables in `.stepyard/defaults.yaml` that apply to all steps unless overridden at the step level.
- **FR11:** The engine can resolve environment variable values from the host process environment using `${VAR}` syntax in workflow YAML.
- **FR12:** The engine can restrict environment variables passed to the sandbox to only those explicitly declared in the workflow or defaults (no full host env passthrough).

**Git Workspace Management (Growth):**

- **FR13:** The engine can create isolated git worktrees for each workflow session, enabling multiple agents to operate on the same repository concurrently.
- **FR14:** The engine can apply a configurable branch strategy (head, merge-to-head, named-branch) to determine how agent commits land in the repository.
- **FR15:** The engine can auto-merge a temporary branch back to the target branch when the workflow completes successfully (merge-to-head strategy).
- **FR16:** The engine can preserve a temporary branch and emit a conflict event when auto-merge fails, allowing manual resolution.
- **FR17:** The engine can prune stale worktrees on startup (two-phase: git metadata cleanup, then orphan directory removal).
- **FR18:** The engine can detect uncommitted changes in a worktree and preserve it for inspection instead of deleting it during cleanup.

**Workflow Configuration (Growth):**

- **FR19:** Workflow authors can use `{{KEY}}` placeholder syntax in step commands, resolved from CLI arguments or workflow-level variables at dispatch time.
- **FR20:** Workflow authors can define a completion signal string that, when detected in agent stdout, terminates the iteration loop early.
- **FR21:** The engine can validate that all referenced `{{KEY}}` placeholders have corresponding values before executing a step, failing fast with a clear error if any are missing.

**Session Observability (Growth):**

- **FR22:** The engine can emit new event types (`IdleTimeoutFired`, `SignalReceived`, `BranchCreated`, `MergeAttempted`, `MergeConflict`) as backward-compatible additions to the session event log.
- **FR23:** The engine can record branch strategy decisions and git operations as session events, making the full git lifecycle of parallel agents auditable via `Session::replay()`.
- **FR24:** Operators can list sessions by status (running, completed, failed, cancelled) and time range via CLI command.

**Provider Extensibility (Expansion):**

- **FR25:** The engine can support multiple sandbox providers (Docker, Podman, local shell) through the existing `SandboxLifecycle` trait without requiring a new abstraction layer.
- **FR26:** Sandbox providers can accept a configuration object at creation time specifying resource limits, volume mounts, and network policies.
- **FR27:** The engine can execute interactive sessions with TTY forwarding through sandbox providers that support it.

### NonFunctional Requirements

**Performance:**

- **NFR1:** Signal handler response — container cleanup initiated within 1s of SIGTERM (beat kernel's 30s SIGKILL deadline).
- **NFR2:** Step timeout precision — fire within 1s of configured threshold.
- **NFR3:** Step timeout overhead — <50ms added latency per step from `tokio::time::timeout()` wrapper.
- **NFR4:** Env var resolution — <10ms for resolving `${VAR}` references (O(1) host env lookup).
- **NFR5:** Worktree creation — <5s per worktree including git checkout (acceptable for workflow startup, not hot path).
- **NFR6:** Worktree pruning — <30s for up to 50 stale worktrees (runs at engine startup, not latency-sensitive).

**Security:**

- **NFR7:** Env var isolation — only variables explicitly declared in workflow YAML or `defaults.yaml` are forwarded to sandbox containers. The engine never passes its full `process.env` to Docker.
- **NFR8:** No credential in logs — environment variable values never written to session events. Event payloads record variable *names* only (e.g., `env_keys: ["GITHUB_TOKEN", "API_KEY"]`), not values.
- **NFR9:** Sandbox boundary preserved — all new features operate through the existing `docker exec` interface. No feature introduces direct filesystem access between host and container outside Docker volume mounts.
- **NFR10:** Signal handler safety — the signal handler does not access PostgreSQL (connection may be dead). It only sets the cancel broadcast and calls `docker rm -f` via subprocess. Session event recording is best-effort.

**Reliability:**

- **NFR11:** Crash recovery — after any process termination (SIGTERM, SIGINT, OOM, panic), the engine can be restarted and `Session::replay()` reconstructs correct state for all sessions. No manual database intervention required.
- **NFR12:** Idempotent cleanup — `destroy_by_session()` tolerates already-destroyed containers without error. Signal handler, cancel path, and normal completion can all call destroy without coordination.
- **NFR13:** Timeout determinism — step timeout and idle timeout always produce a `StepFailed` event with a specific reason string. No silent failures — every termination is recorded.
- **NFR14:** Worktree safety — worktrees with uncommitted changes are never deleted automatically. The engine preserves them and emits an event for manual inspection.

**Integration:**

- **NFR15:** Docker CLI compatibility — all new features use `docker run`, `docker exec --env`, and `docker rm -f` only. No Docker API or bollard dependency. Compatible with Docker CE 20.10+ and Docker Desktop.
- **NFR16:** PostgreSQL compatibility — new event types use the existing `session_events` table schema (JSONB payload). No schema migrations required for MVP features. New event variants are `#[non_exhaustive]` and `#[serde(other)]`-safe.
- **NFR17:** Git CLI compatibility — WorkspaceManager uses `git worktree add/remove/list/prune` commands. Compatible with git 2.30+. No libgit2 dependency.
- **NFR18:** Existing workflow YAML — all new YAML fields (`timeout`, `env`, `branch_strategy`) use `#[serde(default)]`. Existing workflows without these fields continue to work unchanged.

**Maintainability:**

- **NFR19:** Zero warnings — `cargo clippy --workspace --all-targets -- -D warnings` clean across entire workspace after all changes.
- **NFR20:** Test coverage — ≥70% unit coverage per modified crate. Each FR has at least one test that verifies its acceptance criteria.
- **NFR21:** Error types — all new public errors use `thiserror` derives. No `anyhow` in library crates.
- **NFR22:** Backward compatibility — no breaking changes to public trait signatures in MVP. `SandboxLifecycle::exec()` gains an optional `env` parameter via a new default-impl method (`exec_with_env`), not by changing the existing signature.

### Additional Requirements

**Architectural Invariants (hard constraints):**

- **No new crates.** All features land in existing `stepyard-core`, `stepyard-session`, `stepyard-sandbox-orchestrator`, `stepyard-harness`, or the root `stepyard` binary (`src/`). ADR-011 reserves the only planned future crate (`stepyard-mcp-proxy`) for a separate concern.
- **Binary layout.** `stepyard` binary lives at workspace root `src/main.rs` (NOT `crates/stepyard/src/main.rs`). Startup reconciler and signal handler go in `src/startup.rs` and `src/signal.rs`.
- **Session-log-as-truth.** Engine holds zero in-memory state between steps. Any new feature (timeout, branch state, env dict) must be expressible as session events. Runtime registries via `once_cell::sync::Lazy<DashMap<…>>` or `static` are banned.
- **Docker CLI subprocess only.** No bollard, no embedded Docker client. All operations via `tokio::process::Command` invoking `docker run`, `docker exec`, `docker rm -f`. Container naming: `stepyard-session-{uuid}`.
- **Async safety.** All types crossing task boundaries are `Send + Sync`. No `std::sync::Mutex` or `parking_lot::Mutex` held across `.await`. Use `Arc<AtomicBool>` for flags or `tokio::sync::Mutex` if coordination is unavoidable.
- **Backward compatibility.** MVP preserves all existing public trait signatures. Env dict added via new `exec_with_env` default-impl method (not by changing `exec`). YAML fields added via `#[serde(default)]`.

**Core Architectural Decisions (D1-D10 + Crash Recovery):**

- **D1 — Active-session cancel coordination:** per-process `Arc<tokio::sync::broadcast::Sender<()>>` owned by `main()` and passed into every `Engine::new()` via `HarnessConfig::shutdown_rx`. Each engine holds a `broadcast::Receiver<()>` field, subscribes during construction, aborts its current step when broadcast fires via `select!`. NO DashMap registry.
- **D2 — Signal handler:** `main()` installs `tokio::signal::unix` handlers for SIGINT/SIGTERM that fire `broadcast::Sender<()>::send(())` (best-effort, ignores `SendError`), wait up to `HarnessConfig::shutdown_grace_s` (default 10s) for in-flight engines to complete `finalise_cancel()`, then exit with code 130 (SIGINT) or 143 (SIGTERM).
- **D3 — Env extension via default-impl method:** `SandboxLifecycle::exec_with_env(id, cmd, env: &HashMap<String, String>)` added as default-impl method delegating to `exec(id, cmd)` (ignoring env). `DockerLifecycle` and `MockLifecycle` override. Callers inside `stepyard-harness` always invoke `exec_with_env` (empty map when no env configured). Existing `exec` signature unchanged.
- **D4 — WorkspaceManager location:** `pub trait WorkspaceManager` lives in `crates/stepyard-sandbox-orchestrator/src/workspace.rs` (NOT `stepyard-core`, NOT a new crate). `GitWorktreeManager` is the production impl.
- **D5 — 8 new Event variants, inline:** `StepTimeoutFired { step_index, configured_ms }`, `IdleTimeoutFired { step_index, idle_threshold_ms }`, `SignalReceived { signal: String }`, `BranchCreated { branch, base }`, `MergeAttempted { source, target }`, `MergeConflict { source, target, files: Vec<String> }`, `WorkspacePrepared { path, strategy }`, `WorkspacePruned { path, reason }`. Added directly to existing `stepyard_core::Event` enum. All `#[serde(rename_all = "snake_case")]`; `#[non_exhaustive]` + `#[serde(other)]` preserved for forward compat.
- **D6 — Env var cascade resolution:** `Engine::prepare_step` resolves env from step-level > workflow-level > `.stepyard/defaults.yaml` > host `${VAR}`. Values pass to subprocess as `--env K=V` argv elements — never shell-interpolated. Lifecycle stays dumb executor receiving resolved env map + resolved argv.
- **D7 — `{{KEY}}` template substitution:** pre-parse pass in `stepyard_core::template::substitute`, runs BEFORE `serde_yaml::from_str`. Substitution sources: CLI `--var KEY=VAL` flags first, then `.stepyard/defaults.yaml`. Missing placeholders produce `EngineError::PlaceholderUnresolved { key, found_at }`.
- **D8 — Worktree pruning at startup:** NOT per-session, NOT on a timer. Runs once at `main()` startup inside `stepyard::startup::reconcile()`. Two-phase: (1) `git worktree prune` for git metadata, (2) filesystem walk under `.stepyard/workspaces/` removing orphans without matching git entry. SKIP dirs with uncommitted changes (detected via `git status --porcelain`). Retention: `HarnessConfig::workspace_retention_hours` (default 24h).
- **D9 — Error taxonomy via `TerminationReason` sub-enum:** single `EngineError::StepFailed { step_index: u32, reason: TerminationReason }` variant; `TerminationReason` sub-enum carries `StepTimeout { configured_ms }`, `IdleTimeout { idle_ms }`, `Cancelled`, `SignalReceived(String)`, `Other(String)`. NO sibling variants like `EngineError::StepTimeout` or `EngineError::StepCancelled` at the top level.
- **D10 — Branch strategy config:** per-workflow top-level YAML field `branch_strategy:`. Values: `head | merge_to_head | named_branch` (snake_case matching Rust conventions, NOT PascalCase, NOT kebab-case). `named_branch` requires sibling `branch_name:` field (or `{{BRANCH_NAME}}` placeholder). Missing → `EngineError::PlaceholderUnresolved` at parse time. CLI flag `--branch-strategy` (Growth phase) overrides YAML.
- **Crash Recovery reconcile:** `stepyard::startup::reconcile()` runs before any `Engine::new()` in `main()`. Three sequential phases:
  1. **Session reconciliation** — `SELECT id FROM sessions WHERE status='running'`; for each, replay events to determine last-known state; if session's container is gone (phase 2), append `SignalReceived { signal: "crash_recovery" }` and transition `status='failed'`.
  2. **Container reconciliation** — `docker ps --filter "name=stepyard-session-*" --format "{{.Names}}"`; destroy any container not matching a running session (orphan from prior crash).
  3. **Worktree pruning** — per D8 two-phase protocol.

   All three phases idempotent. Exempt from synchronous-emit-before-IO rule (runs before any live session exists).

**Security Requirements:**

- **Argv-not-shell rule:** every user-provided string reaching a subprocess MUST pass as an argv element, never joined into a shell string at the stepyard layer. Use `Command::new("docker").args([…])`, NEVER `Command::new("sh").arg("-c").arg(format!(…))`.
- **Explicit shell escape hatch:** users needing pipes/redirects/shell expansion write `command: ["sh", "-c", "ls | grep foo"]` explicitly in workflow YAML. Security of contents inside `sh -c` is the user's responsibility, not stepyard's. Documented in workflow schema docs + reinforced by negative-control test.
- **YAML-safe template substitution:** `serde_yaml::to_string(&value)` every substituted value before embedding into YAML document. Never raw-string-interpolate user values into YAML text (would allow `---`, `&anchor`, `*alias`, `: ` to inject YAML structure).
- **Env var isolation:** only vars declared in workflow YAML or `.stepyard/defaults.yaml` forward to containers. Host `process.env` never passes wholesale. No `--env-host` passthrough flag.
- **Credential redaction:** environment variable *values* never written to session events. Event payloads record variable *names* only (e.g., `env_keys: ["GITHUB_TOKEN", "API_KEY"]`).
- **Legacy carveout:** `DockerLifecycle::exec(id, cmd)` at `crates/stepyard-sandbox-orchestrator/src/docker.rs:173` retains `sh -c` semantics for backward compat. The argv-only rule applies to NEW subprocess calls from this PRD (`exec_with_env`, workspace git commands, startup reconcile `docker ps`). Legacy migration to argv-only is scoped as post-MVP tech-debt refactor.

**Testing & Enforcement Mechanisms:**

- **Workspace-wide lints:** `[workspace.lints.rust] non_exhaustive_omitted_patterns = "deny"` in root `Cargo.toml`. Propagates to all consumer crates (per-site `#[deny(…)]` is insufficient — lint only fires within crate where enum is defined). Requires Rust 1.74+ (workspace pins 1.75, so this works).
- **Clippy gate:** `cargo clippy --workspace --all-targets -- -D warnings` required before declaring any story complete.
- **Coverage gate:** `cargo llvm-cov --workspace --fail-under-lines 70` in CI. Line coverage, not branch (branch coverage on Rust is experimental and flaky).
- **Proptest required** for: template substitution (`{{KEY}}` preprocessor), env var resolution (`${VAR}` expansion), YAML preprocessor, CLI argument parsing (`--var KEY=VAL`, `--timeout`, `--branch-strategy`). Placed in `#[cfg(test)] mod proptest_tests` inside the module OR dedicated `tests/*_proptest.rs` file. NOT required for cleanup logic, pure internal helpers, or functions whose inputs are caller-constrained (not user-driven).
- **Negative-control security tests:** `tests/injection_negative.rs` required in every crate with substitution responsibilities. Must include BOTH:
  1. Positive-control: user value `$(rm -rf /)` passed through `command: [$VAR]` appears as literal argv element and does NOT execute (stepyard's guarantee).
  2. Negative-control: user value passed through `command: ["sh", "-c", "$VAR"]` DOES execute, proving the escape hatch is user-owned.
- **Rule 7a — In-process tokio tests:** `tokio::time::pause()` + `tokio::time::advance()` for virtual time. `tokio::time::sleep(…)` is BANNED inside `#[tokio::test]` / `#[cfg(test)]` blocks (pre-merge grep check or custom clippy restriction).
- **Rule 7b — Out-of-process `assert_cmd` tests:** every `Command` must specify `.timeout(Duration::from_secs(N))`. NEVER invoke `.output()` or `.status()` without a `.timeout()`.
- **Mock extension pattern:** extend existing `MockLifecycle`, never create `MockLifecycleV2`. Add new fields to `MockLifecycleCall` struct with `#[serde(default)]`. When adding a default-impl method: mock override MUST record the new parameter AND at least one test MUST assert on that parameter (mutation-resistance — default impl that silently drops the parameter would otherwise pass every existing test).
- **Synchronous emit-before-IO:** every state-changing action calls `session.append(evt).await?` on the same `.await` chain as the subsequent external IO, in the same `async fn`. NEVER `tokio::spawn` the emit. Ordering: decision → `append(event).await?` → IO action. Exempt: read-only queries, cross-session reconciliation (startup reconcile).

**Structural / File Placement Requirements:**

- `WorkspaceManager` trait + `GitWorktreeManager` impl → `crates/stepyard-sandbox-orchestrator/src/workspace.rs`
- `{{KEY}}` template preprocessor → `crates/stepyard-core/src/template.rs`
- Startup reconcile orchestrator → `src/startup.rs` (workspace-root binary — composes `DockerLifecycle` + `GitWorktreeManager` + PG pool from concrete types)
- Signal handler + broadcast construction → `src/signal.rs`
- Docker stderr → `SandboxError` classifier → `crates/stepyard-sandbox-orchestrator/src/docker_errors.rs`
- All 8 new `Event` variants → `crates/stepyard-core/src/event.rs` only (never in consumer crates)
- Errors co-located with types: `WorkspaceError` inside `workspace.rs`, not a separate `workspace_errors.rs`
- One file per major trait (`workspace.rs`, `template.rs`)
- Internal helpers: sibling `{module}/internal.rs` or `{module}/_impl.rs`

**CLI Surface Additions:**

- `stepyard session list --status <running|completed|failed|cancelled> [--since <duration>]` — new CLI subcommand (FR24). Backed by PostgreSQL query against `sessions.status` column (NOT an in-memory registry lookup). Consistent with session-log-as-truth invariant.
- `stepyard run --var KEY=VAL` — new CLI flag for template substitution values (Growth phase, FR19).
- Workflow YAML field `timeout:` (u64 ms) — MVP, per-step wall-clock timeout (FR1).
- Workflow YAML field `idle_timeout:` (u64 ms) — Growth, per-step idle (no stdout) timeout (FR2).
- Workflow YAML field `env: { KEY: VAL }` — MVP, step-level environment variable map (FR9).
- Workflow YAML field `branch_strategy:` (enum string) — Growth, top-level (FR14).
- Workflow YAML field `branch_name:` (string) — Growth, top-level, required when `branch_strategy: named_branch` (FR14).
- CLI flag `--branch-strategy <head|merge-to-head|named-branch:<name>>` — Growth, overrides workflow YAML for one-off runs.

**Integration Clarifications (from Architecture validation):**

- **Idle timeout implementation:** `tokio::process::Command::stdout(Stdio::piped())` + `tokio::io::AsyncBufReadExt::read_until` wrapped in `tokio::time::timeout(idle_threshold_ms)` per read. Timer resets on every byte received. Fires `IdleTimeoutFired` + `TerminationReason::IdleTimeout` when threshold exceeded.
- **`session list --status` backing:** PostgreSQL query on `sessions.status` column — NOT an in-memory registry. D1's ban on runtime registries does not preclude DB queries (sessions table is the authoritative state store).
- **Docker error taxonomy:** `DockerLifecycle` parses stderr strings into `SandboxError::{ContainerNotFound, DaemonUnreachable, ImagePullFailed, Other}` via string-matching classifier in `docker_errors.rs`.
- **Tracing fields:** structured fields, not format strings. `tracing::info!(session_id = %id, step = step_index, "step started")` — NOT `tracing::info!("step {} started for session {}", …)`. Field names snake_case matching event field names when possible (correlates logs to events).

**First Implementation Target (MVP Feature #1):**

- **Bug fix (~3 lines):** `crates/stepyard-harness/src/engine.rs:413-416` — `finalise_cancel()` currently passes `SandboxId::default()` to `lifecycle.destroy()` instead of the actual session ID. Fix: pass `self.session.id()`. Add test at `crates/stepyard-harness/tests/cancel_cleanup.rs` verifying the correct container is destroyed. This story unblocks everything else.

### UX Design Requirements

_No UX Design document exists for this project. Stepyard is a Rust CLI/library — no user interface surface beyond command-line output rendering (which is covered by the CLI Surface Additions above: new event variant rendering in `src/cli/display.rs`, `session list` subcommand output format)._

### FR Coverage Map

| FR | Epic | Notes |
|---|---|---|
| FR1 | Epic 1 | Step timeout via `tokio::time::timeout()` wrapping step executor |
| FR2 | Epic 5 | Idle timeout via `AsyncBufReadExt::read_until` + per-read `tokio::time::timeout` |
| FR3 | Epic 1 | Cancel passes `self.session.id()` (MVP bug fix at `stepyard-harness/src/engine.rs:413-416`) |
| FR4 | Epic 1 + Epic 5 | `TerminationReason` sub-enum (D9): StepTimeout/Cancelled in Epic 1, IdleTimeout in Epic 5 |
| FR5 | Epic 2 | `tokio::signal::unix` handlers for SIGINT/SIGTERM in `src/signal.rs` |
| FR6 | Epic 2 | Per-process `broadcast::Sender<()>` in `main()`, receiver per Engine (D1/D2 — NOT DashMap) |
| FR7 | Epic 2 | Idempotent `destroy_by_session()` tolerates already-destroyed |
| FR8 | Epic 2 | `SignalReceived` event synchronously appended before destroy |
| FR9 | Epic 3 | Step-level `env:` YAML field with `#[serde(default)]` |
| FR10 | Epic 3 | `.stepyard/defaults.yaml` cascade in `src/config/merge.rs` |
| FR11 | Epic 3 | `${VAR}` host resolution in `src/config/merge.rs` |
| FR12 | Epic 3 | Opt-in-only forwarding; no `--env-host` flag; injection_negative.rs tests |
| FR13 | Epic 4 | `git worktree add` via `GitWorktreeManager` in `crates/stepyard-sandbox-orchestrator/src/workspace.rs` |
| FR14 | Epic 4 | `branch_strategy:` top-level YAML field (D10) — `head \| merge_to_head \| named_branch` |
| FR15 | Epic 4 | Auto-merge on `merge_to_head` strategy at workflow completion |
| FR16 | Epic 4 | Conflict preserves temp branch + emits `MergeConflict { source, target, files }` event |
| FR17 | Epic 4 | Startup two-phase prune via `stepyard::startup::reconcile()` (D8) |
| FR18 | Epic 4 | `git status --porcelain` detection before delete |
| FR19 | Epic 5 | `stepyard_core::template::substitute` pre-parse pass (D7), YAML-safe |
| FR20 | Epic 5 | Completion-signal string match on agent stdout |
| FR21 | Epic 5 | `EngineError::PlaceholderUnresolved { key, found_at }` at parse time |
| FR22 | Cross-cutting | Events land incrementally: Epic 1 (StepTimeoutFired), Epic 2 (SignalReceived), Epic 4 (BranchCreated/MergeAttempted/MergeConflict/WorkspacePrepared/WorkspacePruned), Epic 5 (IdleTimeoutFired) |
| FR23 | Epic 4 | Branch/git operations recorded as session events via `Session::append(evt).await?` |
| FR24 | Epic 2 | `stepyard session list --status` CLI backed by PG query on `sessions.status` column |
| FR25 | Epic 6 | Podman via existing `SandboxLifecycle` trait (no refinement needed for MVP) |
| FR26 | Epic 6 | Provider config object passed to `SandboxLifecycle::create()` |
| FR27 | Epic 6 | TTY forwarding through trait refinement (Expansion only) |

## Epic List

### Epic 1: Stuck-Agent Termination & Cancel Correctness
Operators can trust the engine not to hang. A step past its wall-clock timeout is terminated with a clear `TerminationReason`; cancel operations destroy the correct container associated with the active session (fixes the `SandboxId::default()` bug at `stepyard-harness/src/engine.rs:413-416`). Ships the `TerminationReason` sub-enum (D9), the `StepFailed` error structure, the first `Event` variant (`StepTimeoutFired`), and the workspace-wide `non_exhaustive_omitted_patterns = "deny"` lint — foundations for every later epic. **Minimum shippable scope:** Bug fix story alone (~3 lines) eliminates the worst production risk.
**Phase:** MVP (Micro-Release 1)
**FRs covered:** FR1, FR3, FR4 (partial — step-timeout + cancel reasons), FR22 (partial — `StepTimeoutFired` variant)

### Epic 2: Crash-Safe Process Lifecycle & Session Visibility
The engine survives OS-level termination (SIGTERM, SIGINT, OOM) without leaving orphaned containers. Active sessions receive a cancel broadcast and shut down gracefully; orphan containers and sessions from prior crashes are reconciled at startup via `stepyard::startup::reconcile()`. Operators query session state by status and time range through the new `stepyard session list --status` CLI subcommand — backed by PostgreSQL query, consistent with session-log-as-truth. Together these serve Journey 2 (VPS crash/OOM) and Journey 3 (DevOps audit).
**Phase:** MVP (Micro-Release 2a)
**FRs covered:** FR5, FR6, FR7, FR8, FR24, FR22 (partial — `SignalReceived` variant)

### Epic 3: Sandbox Environment Injection
Workflow authors declare environment variables per step in workflow YAML or default vars in `.stepyard/defaults.yaml`, resolved from the host via `${VAR}` syntax. The engine passes only declared variables to the sandbox via argv `--env K=V` flags — never shell-interpolated, never full host env passthrough. Ships alongside Epic 2 as parallel MVP work. Introduces the `SandboxLifecycle::exec_with_env` default-impl method (D3) and cascade resolution in `Engine::prepare_step` (D6) with mandatory `injection_negative.rs` negative-control tests.
**Phase:** MVP (Micro-Release 2b)
**FRs covered:** FR9, FR10, FR11, FR12

### Epic 4: Parallel Agent Isolation via Git Workspaces
N agents run concurrently on the same repository without interfering. Each session receives an isolated git worktree; branches merge cleanly on success via the configured strategy (`head | merge_to_head | named_branch`, D10) or preserve the temp branch and emit `MergeConflict` on failure. Stale worktrees are pruned at startup via the D8 two-phase protocol; worktrees with uncommitted changes are never auto-deleted. Introduces the `WorkspaceManager` trait + `GitWorktreeManager` impl in `stepyard-sandbox-orchestrator` (D4) and five new Event variants (`BranchCreated`, `MergeAttempted`, `MergeConflict`, `WorkspacePrepared`, `WorkspacePruned`). Serves Journey 1 (Bruno's parallel reviews).
**Phase:** Growth
**FRs covered:** FR13, FR14, FR15, FR16, FR17, FR18, FR23, FR22 (partial — 5 workspace/branch events)

### Epic 5: Workflow Templating & Idle Detection
One workflow YAML runs across N projects via `{{KEY}}` placeholder substitution (CLI `--var KEY=VAL` flags or `.stepyard/defaults.yaml`). Placeholders are validated pre-execution and fail fast with a clear error. Idle agents (no stdout output for a configurable threshold) are detected via output-based timeout complementing Epic 1's wall-clock timeout. Completion-signal strings let agents exit iteration loops early. Introduces `stepyard_core::template::substitute` pre-parse pass (D7) with YAML-safe substitution and mandatory proptest coverage. Serves Journey 4 (workflow author parameterization).
**Phase:** Growth
**FRs covered:** FR2, FR19, FR20, FR21, FR4 (partial — idle-timeout reason), FR22 (partial — `IdleTimeoutFired` variant)

### Epic 6: Multi-Provider & Interactive Sandboxes
Swap Docker for Podman or cloud providers (Vercel, Daytona-style) through the existing `SandboxLifecycle` trait — no abstraction-layer redesign. Providers accept a rich configuration object at creation time specifying volume mounts, resource limits, and network policies. Interactive debugging via TTY forwarding for sandbox types that support it.
**Phase:** Expansion
**FRs covered:** FR25, FR26, FR27

### Dependency Notes

- **Epic 1 is foundational** — lands `TerminationReason`, `StepFailed` structure, first `Event` variant, workspace-wide `non_exhaustive_omitted_patterns = "deny"` lint. Every subsequent epic builds on these primitives.
- **Epic 2 depends on Epic 1** — the cancel-path bug fix (FR3) must land before the signal handler uses `finalise_cancel()`; the `TerminationReason::Cancelled` variant is reused for shutdown semantics.
- **Epic 3 is parallel to Epic 2** — both are MVP Micro-Release 2; independent. Can ship in either order.
- **Epic 4 depends on Epics 1-2** — uses established `Event` emission patterns and the `startup::reconcile()` pattern from Epic 2's crash-recovery work.
- **Epic 5 depends on Epic 1** — extends `TerminationReason` with the `IdleTimeout` variant; adds the `IdleTimeoutFired` event alongside Epic 1's `StepTimeoutFired`.
- **Epic 6 depends on Epic 3** — new providers reuse the `exec_with_env` abstraction introduced by Epic 3.

## Epic 1: Stuck-Agent Termination & Cancel Correctness

Operators can trust the engine not to hang. A step past its wall-clock timeout is terminated with a clear `TerminationReason`; cancel operations destroy the correct container associated with the active session (fixes the `SandboxId::default()` bug at `stepyard-harness/src/engine.rs:413-416`). Ships the `TerminationReason` sub-enum (D9), the `StepFailed` error structure, the first `Event` variant (`StepTimeoutFired`), and the workspace-wide `non_exhaustive_omitted_patterns = "deny"` lint — foundations for every later epic. **Minimum shippable scope:** Bug fix story alone (~3 lines) eliminates the worst production risk.

**Phase:** MVP (Micro-Release 1)
**FRs covered:** FR1, FR3, FR4 (partial — step-timeout + cancel reasons), FR22 (partial — `StepTimeoutFired` variant)

### Story 1.1: Fix Cancel Cleanup to Destroy Correct Container

As a platform operator,
I want `finalise_cancel()` to destroy the container associated with the active session's actual UUID (not `SandboxId::default()`),
So that cancelling a session immediately frees its resources and doesn't leave orphaned containers.

**Acceptance Criteria:**

**Given** an active session with a live sandbox container
**When** `finalise_cancel()` executes in `crates/stepyard-harness/src/engine.rs` around lines 413-416
**Then** `lifecycle.destroy()` receives `self.session.id()` (converted to `SandboxId` if needed), NOT `SandboxId::default()`
**And** the container `stepyard-session-<session-uuid>` is removed by the cleanup deadline

**Given** `finalise_cancel()` is invoked when the container was already destroyed by another path
**When** `lifecycle.destroy()` returns a `ContainerNotFound` (or equivalent `SandboxError` variant)
**Then** the cancel path treats the error as success (idempotent per FR7 + NFR12 precedent)
**And** the session transitions to `cancelled` status without propagating the error to the caller

**Given** a new integration test file `crates/stepyard-harness/tests/cancel_cleanup.rs`
**When** the test drives a session through cancel with a `MockLifecycle` that records the `SandboxId` passed to `destroy()`
**Then** the recorded ID's underlying UUID matches the session's UUID (not `Uuid::nil()` / default)
**And** the assertion fails if future edits silently regress to `SandboxId::default()`

Coverage: FR3, NFR12 (idempotent cleanup)

### Story 1.2: Introduce `TerminationReason` Sub-Enum and `StepFailed` Error

As an engine maintainer,
I want a single `EngineError::StepFailed { step_index, reason: TerminationReason }` variant carrying a typed termination reason,
So that every step-termination path reports its cause through one taxonomy without sibling variants proliferating.

**Acceptance Criteria:**

**Given** the error taxonomy defined in `crates/stepyard-core/src/error.rs`
**When** `EngineError` is inspected
**Then** the only step-termination variant is `StepFailed { step_index: u32, reason: TerminationReason }`
**And** `TerminationReason` is a sibling enum in the same file deriving `Debug, Clone, thiserror::Error`
**And** `TerminationReason` carries variants `StepTimeout { configured_ms: u64 }`, `IdleTimeout { idle_ms: u64 }`, `Cancelled`, `SignalReceived(String)`, `Other(String)`
**And** no sibling top-level `EngineError::StepTimeout` / `EngineError::StepCancelled` / `EngineError::IdleTimeout` variants exist

**Given** the `#[error("…")]` attribute on each `TerminationReason` variant
**When** the variant's `Display` impl is rendered
**Then** messages are lowercase and have no trailing punctuation (e.g., `"step timeout after {configured_ms}ms"`, `"cancelled"`, `"signal received: {0}"`)
**And** all messages use structured field interpolation (no `format!` with arbitrary strings)

**Given** `EngineError` is `#[non_exhaustive]` (existing invariant)
**When** downstream consumers match on `EngineError::StepFailed { reason, .. }`
**Then** they must further match on `TerminationReason` which is ALSO `#[non_exhaustive]`
**And** the workspace-wide lint from Story 1.3 forces explicit handling of every `TerminationReason` variant in consumers

**Given** unit tests at `crates/stepyard-core/src/error.rs` (inline `#[cfg(test)] mod tests`)
**When** each `TerminationReason` variant is constructed and formatted
**Then** `Display` output matches the documented lowercase, no-trailing-punctuation format
**And** `Debug` output is stable for use in event payload assertions

Coverage: FR4 (infrastructure), D9

### Story 1.3: Add `StepTimeoutFired` Event and Workspace `non_exhaustive_omitted_patterns` Lint

As an engine maintainer,
I want the first new `Event` variant (`StepTimeoutFired`) defined alongside the workspace-wide lint that denies missing `#[non_exhaustive]` match arms,
So that future event variants can ship without silent breakage in subscribers and display code.

**Acceptance Criteria:**

**Given** `crates/stepyard-core/src/event.rs` owns the `Event` enum
**When** `StepTimeoutFired { step_index: u32, configured_ms: u64 }` is added as a new variant
**Then** the enum retains its existing `#[non_exhaustive]` and `#[serde(other)]` attributes
**And** the variant carries `#[serde(rename_all = "snake_case")]` so JSONB payloads serialize as `{"type":"step_timeout_fired","step_index":0,"configured_ms":300000}`
**And** no other event variants are added in this story (D5's eight new variants land incrementally across epics)

**Given** the workspace root `Cargo.toml`
**When** the `[workspace.lints.rust]` table is inspected
**Then** it contains `non_exhaustive_omitted_patterns = "deny"`
**And** the Rust toolchain pin in `rust-toolchain.toml` (or equivalent) is ≥1.74 (required for the lint; workspace already pins 1.75)
**And** every crate in the workspace inherits the lint via `[lints] workspace = true` in its `Cargo.toml`

**Given** event-consuming code at `src/events/subscribers.rs` and `src/cli/display.rs`
**When** the match on `Event` is inspected
**Then** an explicit arm for `Event::StepTimeoutFired { step_index, configured_ms }` exists
**And** CLI display renders `"step {step_index} timed out after {configured_ms}ms"` (or equivalent lowercase phrase)
**And** `cargo clippy --workspace --all-targets -- -D warnings` succeeds with the lint active

**Given** a unit test in `crates/stepyard-core/src/event.rs`
**When** `serde_json::to_value(Event::StepTimeoutFired { step_index: 2, configured_ms: 300_000 })` is called
**Then** the output is `{"type":"step_timeout_fired","step_index":2,"configured_ms":300000}`
**And** round-trip deserialization produces the identical variant

Coverage: FR22 (partial — `StepTimeoutFired`), testing-enforcement invariant (workspace lint)

### Story 1.4: Enforce Step Timeout via `tokio::time::timeout` Wrapper

As a workflow author,
I want the engine to enforce the `timeout:` YAML field per step by wrapping the step executor in `tokio::time::timeout`,
So that a stuck agent never runs past its configured wall-clock deadline and the session log records exactly why it stopped.

**Acceptance Criteria:**

**Given** a workflow YAML step with `timeout: 5000` (ms)
**When** `Engine::execute_step` runs the step executor
**Then** the executor is wrapped in `tokio::time::timeout(Duration::from_millis(5000), step_future)`
**And** the wrapper adds <50ms overhead per step (NFR3) — measured via the existing bench or an integration assertion on elapsed time
**And** missing / absent `timeout:` field means no wrapper is applied (backward compat per NFR18 — existing workflows continue unchanged via `#[serde(default)]` on the YAML field)

**Given** a step whose executor runs longer than the configured `timeout`
**When** the timeout elapses
**Then** the engine synchronously calls `self.session.append(Event::StepTimeoutFired { step_index, configured_ms }).await?` on the same `.await` chain
**And** immediately after (still on the same `.await` chain) calls `self.lifecycle.destroy(&self.sandbox_id).await` (tolerating `ContainerNotFound` per NFR12)
**And** returns `Err(EngineError::StepFailed { step_index, reason: TerminationReason::StepTimeout { configured_ms } })`
**And** the emit-before-IO ordering is never reversed (no `tokio::spawn` around the emit)

**Given** a step that completes before its `timeout` elapses
**When** `Engine::execute_step` returns
**Then** no `StepTimeoutFired` event is emitted
**And** no destroy call is triggered by the timeout path (normal completion cleanup handles that)
**And** the elapsed time measured inside the wrapper stays within NFR3's 50ms overhead budget

**Given** a test at `crates/stepyard-harness/tests/step_timeout.rs` annotated `#[tokio::test(start_paused = true)]`
**When** the test calls `tokio::time::advance(Duration::from_secs(301)).await` (simulated 301 seconds for a `timeout: 300000`)
**Then** the test observes an emitted `Event::StepTimeoutFired` BEFORE `MockLifecycle::destroy` was called
**And** the test observes `EngineError::StepFailed { reason: TerminationReason::StepTimeout { configured_ms: 300_000 }, .. }` returned
**And** the test file contains NO calls to `tokio::time::sleep(…)` (Rule 7a — pre-merge grep check would reject)
**And** the test uses `#[tokio::test(start_paused = true)]` or explicit `tokio::time::pause()` (virtual time only)

Coverage: FR1, FR4 (StepTimeout reason), FR22 (StepTimeoutFired emission path), NFR3 (overhead), NFR13 (deterministic recording)

## Epic 2: Crash-Safe Process Lifecycle & Session Visibility

The engine survives OS-level termination (SIGTERM, SIGINT, OOM) without leaving orphaned containers. Active sessions receive a cancel broadcast and shut down gracefully; orphan containers and sessions from prior crashes are reconciled at startup via `stepyard::startup::reconcile()`. Operators query session state by status and time range through the new `stepyard session list --status` CLI subcommand — backed by PostgreSQL query, consistent with session-log-as-truth. Together these serve Journey 2 (VPS crash/OOM) and Journey 3 (DevOps audit).

**Phase:** MVP (Micro-Release 2a)
**FRs covered:** FR5, FR6, FR7, FR8, FR24, FR22 (partial — `SignalReceived` variant)

### Story 2.1: Thread Cancel Broadcast Channel Through Engine Construction

As an engine maintainer,
I want a per-process `Arc<tokio::sync::broadcast::Sender<()>>` constructed in `main()` and a `broadcast::Receiver<()>` field on every `Engine` subscribed via `HarnessConfig::shutdown_tx`,
So that later stories can wire signal handlers and crash-recovery without introducing a runtime registry.

**Acceptance Criteria:**

**Given** `HarnessConfig` in `crates/stepyard-harness/src/config.rs`
**When** the struct is inspected
**Then** it gains `pub shutdown_tx: Arc<tokio::sync::broadcast::Sender<()>>`
**And** it gains `pub shutdown_grace_s: u64` defaulting to `10` via `#[serde(default = "…")]` (D2 default)

**Given** `Engine::new(HarnessConfig)` in `crates/stepyard-harness/src/engine.rs`
**When** constructed
**Then** it subscribes: `let shutdown_rx = config.shutdown_tx.subscribe();`
**And** stores `shutdown_rx: tokio::sync::broadcast::Receiver<()>` as a field on the Engine struct
**And** no `DashMap`, `once_cell::sync::Lazy<Mutex<…>>`, or `static` runtime registry is introduced (D1 invariant)

**Given** `main()` in `src/main.rs`
**When** main starts
**Then** it constructs `let (tx, _) = tokio::sync::broadcast::channel::<()>(16); let shutdown_tx = Arc::new(tx);`
**And** passes `shutdown_tx.clone()` into every `Engine::new(HarnessConfig { shutdown_tx: .., .. })`
**And** no Engine owns the `Sender` — only `main()` does (receivers are cloned through subscribe)

**Given** a unit test at `crates/stepyard-harness/tests/broadcast_plumbing.rs`
**When** multiple Engines are constructed from a shared `shutdown_tx` and the test calls `shutdown_tx.send(()).unwrap()`
**Then** every Engine's receiver observes exactly one message
**And** the test uses `#[tokio::test(start_paused = true)]` (Rule 7a) and contains no `tokio::time::sleep(…)` calls

Coverage: FR6, D1, D2 (infrastructure)

### Story 2.2: Install SIGINT/SIGTERM Handlers and Graceful Shutdown Deadline

As a platform operator,
I want the `stepyard` binary to intercept SIGINT and SIGTERM, fire the broadcast channel, wait up to `shutdown_grace_s` for in-flight engines, then exit with the canonical signal exit code,
So that container cleanup starts within 1s (NFR1) and never hits the kernel's 30s SIGKILL deadline.

**Acceptance Criteria:**

**Given** a new file `src/signal.rs`
**When** inspected
**Then** it exports `pub async fn install_handlers(shutdown_tx: Arc<tokio::sync::broadcast::Sender<()>>, grace_s: u64) -> ExitCode`
**And** uses `tokio::signal::unix::signal(SignalKind::interrupt())` and `SignalKind::terminate()` (not the cross-platform `tokio::signal::ctrl_c()` — D2 is Unix-only)
**And** on signal fire calls `let _ = shutdown_tx.send(());` (best-effort, ignoring `SendError`)
**And** records which signal fired so `main()` can pick the exit code

**Given** SIGINT fires
**When** the grace period elapses or all engines complete
**Then** the process exits with code `130`
**And** elapsed wall-clock from signal-to-exit ≤ `shutdown_grace_s + 1` seconds

**Given** SIGTERM fires
**When** the grace period elapses or all engines complete
**Then** the process exits with code `143`

**Given** NFR10 (signal handler safety)
**When** the handler body is inspected
**Then** it does NOT touch the PostgreSQL pool (connection may be dead during shutdown)
**And** it does NOT shell out to `docker rm -f` directly (that is the Engine's responsibility via Story 2.3)
**And** its only side effects are `shutdown_tx.send(())` and measuring wall-clock deadline

**Given** an integration test at `tests/signal_handler.rs` using `assert_cmd`
**When** the test spawns `stepyard run <trivial-workflow>` as a subprocess and sends SIGTERM after step start
**Then** the subprocess exits within `shutdown_grace_s + 1` seconds with code 143
**And** every `std::process::Command` / `assert_cmd::Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** elapsed time from signal-to-exit is <5s (NFR1: cleanup within 1s + grace margin)

Coverage: FR5, D2, NFR1, NFR10

### Story 2.3: Emit `SignalReceived` Event and Destroy Container on Broadcast

As an engine maintainer,
I want each `Engine` to `select!` on the broadcast receiver, synchronously emit `Event::SignalReceived` to its session log, then idempotently destroy its sandbox container,
So that SIGTERM / SIGINT cancellation produces an auditable session record before the process exits.

**Acceptance Criteria:**

**Given** `crates/stepyard-core/src/event.rs`
**When** inspected
**Then** it gains `Event::SignalReceived { signal: String }` with `#[serde(rename_all = "snake_case")]`
**And** subscribers at `src/events/subscribers.rs` and `src/cli/display.rs` gain explicit match arms (workspace `non_exhaustive_omitted_patterns = "deny"` lint from Story 1.3 enforces this)
**And** CLI display renders `"signal received: {signal}"` (lowercase, no trailing punctuation)

**Given** `Engine::run_step` (the main step loop) in `crates/stepyard-harness/src/engine.rs`
**When** the step executor future is in flight
**Then** it runs inside `tokio::select! { result = step_future => …, _ = self.shutdown_rx.recv() => …, }` with the two arms
**And** when the receiver arm fires, execution enters a finalise-cancel path
**And** the finalise path synchronously `self.session.append(Event::SignalReceived { signal: signal_name.clone() }).await?` BEFORE any container destroy call
**And** on the same `.await` chain (no `tokio::spawn`) calls `self.lifecycle.destroy(&self.sandbox_id).await` (tolerating `ContainerNotFound` per NFR12)
**And** returns `Err(EngineError::StepFailed { step_index, reason: TerminationReason::SignalReceived(signal_name) })` (from Story 1.2's taxonomy)

**Given** the signal name is propagated from the handler via a per-engine channel or config field
**When** `SignalReceived` is emitted
**Then** `signal` is one of `"sigterm"`, `"sigint"`, or (for Story 2.4) `"crash_recovery"` — lowercase snake_case
**And** the `TerminationReason::SignalReceived(String)` argument carries the identical string

**Given** `MockLifecycle` in `crates/stepyard-sandbox-orchestrator/src/mock.rs` (or test-common module)
**When** extended
**Then** `MockLifecycleCall::Destroy { id: SandboxId }` records the `SandboxId` parameter
**And** at least one test in this story asserts on `id` matching the session UUID (mock-extension safeguard — prevents silent regression to `SandboxId::default()`)

**Given** an integration test at `crates/stepyard-harness/tests/signal_cancel.rs`
**When** the test constructs an Engine with `MockLifecycle`, starts a long-running step, then fires `shutdown_tx.send(()).unwrap()`
**Then** the test observes `Event::SignalReceived { signal: "sigterm" }` appended to the session log
**And** the session event log records the emit happened BEFORE `MockLifecycleCall::Destroy` (emit-before-IO ordering)
**And** the returned error is `EngineError::StepFailed { reason: TerminationReason::SignalReceived(s), .. }` where `s == "sigterm"`
**And** the test uses `#[tokio::test(start_paused = true)]` and contains no `tokio::time::sleep(…)` (Rule 7a)

Coverage: FR7, FR8, FR22 (SignalReceived variant), NFR12 (idempotent destroy)

### Story 2.4: Startup Crash Recovery — Reconcile Orphan Sessions and Containers

As a platform operator,
I want `stepyard` to run a three-phase reconcile at startup that marks orphan `running` sessions as `failed`, destroys orphan containers, and stubs the worktree pruning slot (Epic 4 fills it),
So that a restart after OOM / crash / hard-kill leaves the engine in a consistent state without manual intervention.

**Acceptance Criteria:**

**Given** a new file `src/startup.rs`
**When** inspected
**Then** it exports `pub async fn reconcile(pg: &PgPool, lifecycle: &DockerLifecycle) -> Result<ReconcileReport, ReconcileError>`
**And** `main()` calls `reconcile(&pg, &lifecycle).await?` BEFORE constructing any `Engine::new()`
**And** the function runs three sequential phases in this exact order: session reconciliation → container reconciliation → worktree pruning

**Given** phase 1 (session reconciliation)
**When** executed
**Then** it runs `SELECT id FROM sessions WHERE status = 'running'`
**And** for each returned `session_id`, appends `Event::SignalReceived { signal: "crash_recovery".to_string() }` to the session's event log
**And** updates `UPDATE sessions SET status = 'failed', ended_at = NOW() WHERE id = $1`
**And** the phase is idempotent: running it again after all sessions already moved to `failed` returns `Ok` with zero changes

**Given** phase 2 (container reconciliation)
**When** executed
**Then** it runs `docker ps --filter "name=stepyard-session-*" --format "{{.Names}}"` via `tokio::process::Command` with argv-only (never `sh -c`)
**And** for each returned name, extracts the UUID suffix after `stepyard-session-`
**And** if that UUID does NOT correspond to a session whose `status = 'running'` in PG, runs `docker rm -f <name>` via argv
**And** tolerates "No such container" stderr as success (NFR12 idempotent cleanup)
**And** the phase is idempotent: two sequential runs produce identical final container state

**Given** phase 3 (worktree pruning — stub for Epic 4)
**When** executed
**Then** it returns `Ok(())` immediately with a `// TODO(Epic 4): D8 two-phase prune — see Epic 4 Story N.M` comment
**And** no filesystem access occurs in this story

**Given** the synchronous-emit-before-IO rule
**When** `reconcile()` is inspected
**Then** a comment at the top documents: `// Exempt from emit-before-IO rule: runs before any live session exists at startup`
**And** completion is logged via `tracing::info!(sessions_reconciled = n, containers_pruned = m, "startup reconcile complete")` (structured fields, not format strings)

**Given** an integration test at `tests/startup_reconcile.rs` (workspace-root `tests/` — uses real Docker + PG)
**When** the test seeds PG with a session `status='running'` whose container does not exist AND spawns an orphan container whose UUID has no matching session
**Then** after `reconcile()` runs: PG shows `status='failed'` for the seeded session; its event log contains `SignalReceived { signal: "crash_recovery" }`; the orphan container is destroyed
**And** a second `reconcile()` call produces no state change (idempotency)
**And** every `Command` in the test has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** the test skips gracefully (not fail) if Docker daemon / PG is unavailable (via env flag or `#[ignore]` with explicit opt-in)

Coverage: Crash Recovery (architecture.md), NFR11 (crash recovery), NFR12 (idempotent cleanup)

### Story 2.5: Add `stepyard session list --status` CLI Subcommand

As a DevOps engineer,
I want `stepyard session list --status <running|completed|failed|cancelled> [--since <duration>]` backed by a PostgreSQL query on `sessions.status`,
So that I can audit session outcomes and filter by time range without loading full event logs.

**Acceptance Criteria:**

**Given** a new subcommand in the CLI parser
**When** inspected
**Then** `SessionListArgs` derives `clap::Args` with `status: SessionStatus` (clap `ValueEnum` with variants `Running`, `Completed`, `Failed`, `Cancelled` — snake_case on CLI)
**And** `since: Option<humantime::Duration>` is an optional flag `--since <duration>` (parsed via `humantime::parse_duration`)

**Given** the user invokes `stepyard session list --status running`
**When** the handler runs
**Then** it executes `SELECT id, status, started_at, ended_at FROM sessions WHERE status = $1 ORDER BY started_at DESC` against PG
**And** prints one row per session in tabular format: `<id>  <status>  <started_at ISO-8601 UTC>  <ended_at ISO-8601 UTC or '-'>`
**And** uses `chrono::DateTime<Utc>` `to_rfc3339()` for timestamp formatting

**Given** the user passes `--since 24h`
**When** the handler runs
**Then** the query appends `AND started_at > NOW() - $2::INTERVAL` with the humantime duration bound
**And** invalid durations produce a clap-layer error at parse time (not runtime)

**Given** the session-log-as-truth invariant (D1)
**When** the handler implementation is inspected
**Then** it queries PostgreSQL directly (no `DashMap`, no `Lazy<Mutex<HashMap>>`, no in-memory session cache)
**And** an inline comment documents: `// session-log-as-truth (D1): query PG, never an in-memory registry`

**Given** invalid `--status foobar`
**When** the user invokes with an unsupported value
**Then** clap rejects at parse time with a clear error listing valid values (`running, completed, failed, cancelled`)
**And** exit code is `2` (clap's default for parse errors)

**Given** an integration test at `tests/session_list_cli.rs` using `assert_cmd`
**When** the test seeds PG with sessions in each status, then invokes `stepyard session list --status running`
**Then** stdout contains only `status='running'` rows
**And** output is ordered by `started_at DESC`
**And** `--since 1h` filters out sessions older than 1 hour
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** the test skips gracefully if PG is unavailable

Coverage: FR24

## Epic 3: Sandbox Environment Injection

Workflow authors declare environment variables per step in workflow YAML or default vars in `.stepyard/defaults.yaml`, resolved from the host via `${VAR}` syntax. The engine passes only declared variables to the sandbox via argv `--env K=V` flags — never shell-interpolated, never full host env passthrough. Ships alongside Epic 2 as parallel MVP work. Introduces the `SandboxLifecycle::exec_with_env` default-impl method (D3) and cascade resolution in `Engine::prepare_step` (D6) with mandatory `injection_negative.rs` negative-control tests.

**Phase:** MVP (Micro-Release 2b)
**FRs covered:** FR9, FR10, FR11, FR12

### Story 3.1: Extend `SandboxLifecycle` Trait with `exec_with_env` Default-Impl Method

As an engine maintainer,
I want `SandboxLifecycle` to gain an `exec_with_env(id, cmd, env: &HashMap<String, String>)` default-impl method that delegates to the existing `exec(id, cmd)` (ignoring env),
So that Epic 3 can inject env vars via the new method without changing the existing `exec` signature (NFR22 backward compat).

**Acceptance Criteria:**

**Given** `SandboxLifecycle` trait in `crates/stepyard-sandbox-orchestrator/src/lib.rs` (or wherever currently defined)
**When** inspected
**Then** it gains a new method: `async fn exec_with_env(&self, id: &SandboxId, cmd: &[String], env: &HashMap<String, String>) -> Result<ExecOutput, SandboxError>`
**And** the default impl is `self.exec(id, cmd).await` (env ignored — preserves existing behavior for unmigrated impls)
**And** the existing `exec` method signature is NOT changed (D3 explicit: extension via new method, not parameter addition)
**And** the trait retains `#[async_trait]` (project convention, already-locked)

**Given** the mock-extension safeguard
**When** `MockLifecycle` in `crates/stepyard-sandbox-orchestrator/src/mock.rs` (or test-common) is extended
**Then** `MockLifecycleCall::ExecWithEnv { id: SandboxId, cmd: Vec<String>, env: HashMap<String, String> }` is added as a variant
**And** `MockLifecycle::exec_with_env` override records the full `env` parameter (not dropped, not lossy)
**And** at least one unit test asserts on the recorded `env` contents — without this assertion a default impl that silently drops `env` would pass tests (mutation-resistance per testing-enforcement invariant)

**Given** consumers inside `stepyard-harness`
**When** any code invoking a lifecycle method is inspected
**Then** NEW invocation sites use `exec_with_env(id, cmd, &env)` (even when `env` is empty — pass `&HashMap::new()`)
**And** EXISTING invocation sites calling `exec(id, cmd)` are NOT refactored in this story (only Story 3.4 wires new call sites; backward-compat preserved)
**And** `DockerLifecycle` keeps the existing `exec` method unchanged (the `sh -c` legacy carveout documented in Security Requirements remains intact)

**Given** a unit test at `crates/stepyard-sandbox-orchestrator/src/mock.rs` (inline `#[cfg(test)] mod tests`)
**When** the test calls `mock.exec_with_env(&id, &["echo".to_string(), "hello".to_string()], &env_with_FOO=BAR).await`
**Then** `MockLifecycleCall::ExecWithEnv { env, .. }` records exactly `{"FOO": "BAR"}`
**And** calling the default impl on a type that did NOT override `exec_with_env` records the `exec` call (not ExecWithEnv) — proves default delegation works
**And** the test does NOT use `tokio::time::sleep(…)` (Rule 7a)

Coverage: FR9 infrastructure, D3, NFR22 (backward compat)

### Story 3.2: Implement `DockerLifecycle::exec_with_env` with Argv-Only `--env` Flags

As an engine maintainer,
I want `DockerLifecycle` to override `exec_with_env` with `docker exec --env K=V` argv-only invocations (one `--env` per key-value pair),
So that env vars pass as structured argv elements and are never shell-interpolated (argv-not-shell security rule).

**Acceptance Criteria:**

**Given** `DockerLifecycle` in `crates/stepyard-sandbox-orchestrator/src/docker.rs`
**When** the `exec_with_env` override is inspected
**Then** it builds `tokio::process::Command::new("docker")` with argv `["exec"]`, then one `["--env", "K=V"]` pair per entry in the `env: &HashMap<String, String>` argument (sorted by key for deterministic argv ordering in tests)
**And** then appends container ID as `args.push(container_name)` and finally the user command as individual argv elements: `args.extend_from_slice(cmd)`
**And** no part of the env value reaches a shell — `format!("{}={}", k, v)` is an argv element passed to `.args()`, never concatenated into an `sh -c "…"` string

**Given** an env entry with shell metacharacters in its value (e.g., `{"MSG": "$(rm -rf /)"}`)
**When** `exec_with_env` invokes docker
**Then** the child process sees `MSG=$(rm -rf /)` as a literal string in its env table
**And** `$(…)` is NOT expanded by any shell (argv-only guarantee)
**And** the same applies to `` ` ``, `&&`, `;`, `|`, newlines, `>`, `<`

**Given** the command itself contains user-supplied strings (e.g., `cmd = ["bash", "-c", "echo $FOO"]`)
**When** `exec_with_env` invokes docker
**Then** those strings pass as argv elements to `docker exec` — stepyard does NOT wrap or escape them
**And** if the user chose `sh -c` / `bash -c` as their command, expansion happens inside the sandbox (user responsibility per explicit escape hatch)
**And** stepyard's layer never adds its own shell wrapper

**Given** the legacy carveout (`DockerLifecycle::exec` at `docker.rs:173`)
**When** that method is inspected in this story
**Then** it is NOT migrated — the `sh -c` legacy carveout documented in architecture.md remains (post-MVP tech debt)
**And** `exec_with_env` is fully argv-only regardless of what `exec` does

**Given** an integration test at `crates/stepyard-sandbox-orchestrator/tests/exec_with_env_docker.rs` (opt-in — skips if Docker unavailable)
**When** the test runs `exec_with_env(id, &["printenv".to_string(), "FOO".to_string()], &env_with_FOO=bar)` against a live container
**Then** stdout contains exactly `bar\n`
**And** running with `env_with_FOO="$(rm -rf /)"` shows `printenv` output of `$(rm -rf /)` literally — the host filesystem is untouched (positive-control security assertion)
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** deterministic argv ordering is asserted (env keys sorted): repeating the same call produces identical argv

Coverage: FR9 executor-side, NFR7 (env isolation at exec layer), argv-not-shell rule

### Story 3.3: Extend Workflow YAML Schema with `env:` Fields and `.stepyard/defaults.yaml` Loader

As a workflow author,
I want to declare step-level `env: { KEY: VAL }` and workflow-level `env: { KEY: VAL }` in YAML, plus a `.stepyard/defaults.yaml` file that contributes default env pairs,
So that I can parameterize secrets and config per step, per workflow, or project-wide.

**Acceptance Criteria:**

**Given** the workflow YAML schema in `crates/stepyard-core/src/workflow.rs` (or `stepyard-harness` — wherever `Workflow` / `Step` structs live)
**When** inspected
**Then** `Step` gains `#[serde(default)] pub env: HashMap<String, String>`
**And** `Workflow` gains `#[serde(default)] pub env: HashMap<String, String>` (top-level)
**And** both use `#[serde(default)]` for strict backward compatibility (NFR18 — existing YAML without `env:` still parses)
**And** values are plain strings, not structured types — `${VAR}` substitution is a resolution-time concern (Story 3.4), not a parse-time one

**Given** a new file loader at `src/config/defaults.rs` (workspace-root binary) or `crates/stepyard-core/src/defaults.rs`
**When** inspected
**Then** it exports `pub fn load_defaults(path: &Path) -> Result<Defaults, DefaultsError>` where `Defaults { pub env: HashMap<String, String> }`
**And** if the file does not exist, returns `Ok(Defaults::default())` with empty env (missing file is not an error — defaults are optional)
**And** if the file exists but is malformed, returns `Err(DefaultsError::Parse { path, source })` with a clear error chain

**Given** `DefaultsError` in the same file
**When** inspected
**Then** it derives `thiserror::Error` (NOT `anyhow` — library code per NFR21)
**And** variants: `Io { path: PathBuf, source: std::io::Error }`, `Parse { path: PathBuf, source: serde_yaml::Error }`
**And** `#[error("…")]` messages are lowercase, no trailing punctuation

**Given** existing workflows without an `env:` field
**When** parsed
**Then** they parse successfully as before (no breaking change)
**And** `workflow.env` is `HashMap::new()` by default
**And** `step.env` is `HashMap::new()` by default

**Given** a unit test at the loader module
**When** the test loads a fixture `.stepyard/defaults.yaml` with `env: { FOO: bar, BAZ: qux }`
**Then** `Defaults::env` contains exactly `{"FOO": "bar", "BAZ": "qux"}`
**And** loading a non-existent path returns `Ok(Defaults::default())` (empty env)
**And** loading a malformed YAML fixture returns `Err(DefaultsError::Parse { path, .. })` with path matching the input

Coverage: FR9 YAML side, FR10 (defaults.yaml), NFR18 (backward compat via `#[serde(default)]`)

### Story 3.4: Cascade Resolver in `Engine::prepare_step` with `${VAR}` Host Expansion

As an engine runtime,
I want `Engine::prepare_step` to resolve the effective env for a step by overlaying step > workflow > defaults.yaml and expanding `${VAR}` against host env,
So that one workflow YAML can declare opt-in env with clear precedence and secrets flow through without full host passthrough.

**Acceptance Criteria:**

**Given** `Engine::prepare_step` in `crates/stepyard-harness/src/engine.rs`
**When** inspected
**Then** it computes the effective env by overlaying in precedence order: `defaults.env` < `workflow.env` < `step.env` (step wins; defaults lose — later overlays overwrite earlier keys)
**And** after overlay, it expands any value matching the `${VAR}` syntax against `std::env::var(VAR)` (host process env)
**And** `${VAR}` pattern recognizes exact-form values (e.g., `"${GITHUB_TOKEN}"`) — NOT inline substitution like `"prefix-${VAR}-suffix"` (simplicity for MVP; document in YAML schema docs)
**And** after expansion, passes the resolved `HashMap<String, String>` to `lifecycle.exec_with_env(id, cmd, &env)` (from Story 3.1)

**Given** a `${VAR}` reference that does not exist in host env
**When** `Engine::prepare_step` resolves it
**Then** it returns `Err(EngineError::EnvVarUnresolved { key, source: VariableSource::Host })` (new error variant in `stepyard-core/src/error.rs`)
**And** `#[error("…")]` message is lowercase, no trailing punctuation: `"host env variable not set: {key}"`
**And** fails fast — no step executes with a partially-resolved env

**Given** NFR8 (no credential in logs)
**When** the event log records env-related state
**Then** no event payload contains env values — only key names
**And** if an event needs to record env presence (e.g., for audit), it uses a field like `env_keys: Vec<String>` (sorted)
**And** values never appear in `tracing::` log calls either — structured tracing fields use key names only

**Given** NFR4 (env var resolution performance)
**When** `prepare_step` is benchmarked
**Then** cascade resolution for a step with ≤20 env entries completes in <10ms (O(1) `std::env::var` lookup per `${VAR}` reference)

**Given** a unit test at `crates/stepyard-harness/tests/env_cascade.rs`
**When** the test constructs workflow YAML with `workflow.env = {"FOO": "workflow-foo", "SHARED": "wf"}`, step env `{"FOO": "step-foo"}`, defaults `{"SHARED": "def", "ONLY_DEF": "x"}` + sets `GITHUB_TOKEN=abc123` in host env and `step.env = {"TOKEN": "${GITHUB_TOKEN}"}`
**Then** effective env is `{"FOO": "step-foo", "SHARED": "wf", "ONLY_DEF": "x", "TOKEN": "abc123"}` (step wins for FOO; workflow wins for SHARED; defaults contribute ONLY_DEF; TOKEN expands from host)
**And** the test asserts no env value appears in any event payload (NFR8)
**And** unresolved `${MISSING}` produces `EngineError::EnvVarUnresolved { key: "MISSING", .. }`
**And** the test uses `#[serial_test::serial]` (or equivalent) to avoid races on `std::env::set_var` parallel test contamination

Coverage: FR9, FR10, FR11, FR12, D6, NFR4, NFR7, NFR8

### Story 3.5: Negative-Control Security Tests in `tests/injection_negative.rs`

As a security reviewer,
I want a dedicated negative-control test file that proves (a) user env values reach the container as argv elements and never execute as shell commands at the stepyard layer, and (b) the `sh -c` escape hatch IS user-owned (proves the boundary),
So that any future regression reintroducing shell interpolation is caught at CI time.

**Acceptance Criteria:**

**Given** a new test file `crates/stepyard-harness/tests/injection_negative.rs` (OR `tests/injection_negative.rs` at workspace root — per structural requirements)
**When** inspected
**Then** it contains BOTH a positive-control AND a negative-control test (per Security Requirements)
**And** both tests are marked `#[tokio::test]` with `#[ignore]` or behind an opt-in env flag if they require a live Docker daemon (integration-tier)

**Given** the positive-control test (stepyard's guarantee)
**When** the test runs a workflow with `env: { MSG: "$(touch /tmp/stepyard-pwned-$$)" }` and `command: ["printenv", "MSG"]`
**Then** after execution, `/tmp/stepyard-pwned-*` does NOT exist on the host (the `$(…)` was NOT interpreted — stepyard passed it as argv)
**And** the captured stdout literally contains `$(touch /tmp/stepyard-pwned-…)\n` — proving stepyard's argv-only guarantee
**And** the test asserts on this exact stdout substring match

**Given** the negative-control test (escape hatch IS user-owned)
**When** the test runs a workflow with `env: { MSG: "pwned" }` and `command: ["sh", "-c", "echo $MSG"]`
**Then** stdout is exactly `pwned\n` — the `sh -c` DID expand `$MSG` inside the sandbox (user's responsibility)
**And** the test comment explicitly documents: `// Escape hatch behavior — user chose sh -c, user owns expansion safety`
**And** this test proves stepyard does NOT paternalistically escape values; the boundary is clear

**Given** a second positive-control for CLI template substitution (if Story 5.x's `{{KEY}}` lands — otherwise gated)
**When** the test uses `stepyard run --var MSG='$(rm -rf /)'` against a workflow `command: ["echo", "{{MSG}}"]`
**Then** stdout is literally `$(rm -rf /)\n`
**And** host filesystem is untouched
**And** this story documents the test file has a placeholder section for Epic 5's substitution tests to extend later

**Given** the test file's assert_cmd usage
**When** any `Command` is constructed
**Then** `.timeout(Duration::from_secs(N))` is attached (Rule 7b — required for out-of-process tests)
**And** the file contains no `tokio::time::sleep(…)` calls (Rule 7a — for in-process async sections)

**Given** CI integration
**When** the test file is committed
**Then** `cargo test -p stepyard-harness --test injection_negative` passes locally (and in CI when Docker is available)
**And** the README or contributor docs note: "new crates with user-value substitution MUST add an `injection_negative.rs` with both positive and negative controls"
**And** the existing `non_exhaustive_omitted_patterns = "deny"` lint and `-D warnings` clippy gate apply (NFR19)

Coverage: FR12, NFR7, NFR9, argv-not-shell rule, explicit shell escape hatch rule

## Epic 4: Parallel Agent Isolation via Git Workspaces

N agents run concurrently on the same repository without interfering. Each session receives an isolated git worktree; branches merge cleanly on success via the configured strategy (`head | merge_to_head | named_branch`, D10) or preserve the temp branch and emit `MergeConflict` on failure. Stale worktrees are pruned at startup via the D8 two-phase protocol; worktrees with uncommitted changes are never auto-deleted. Introduces the `WorkspaceManager` trait + `GitWorktreeManager` impl in `stepyard-sandbox-orchestrator` (D4) and five new Event variants (`BranchCreated`, `MergeAttempted`, `MergeConflict`, `WorkspacePrepared`, `WorkspacePruned`). Serves Journey 1 (Bruno's parallel reviews).

**Phase:** Growth
**FRs covered:** FR13, FR14, FR15, FR16, FR17, FR18, FR23, FR22 (partial — 5 workspace/branch events)

### Story 4.1: Define `WorkspaceManager` Trait and `GitWorktreeManager` Skeleton

As an engine maintainer,
I want a `WorkspaceManager` trait in `crates/stepyard-sandbox-orchestrator/src/workspace.rs` and a concrete `GitWorktreeManager` struct with stub method bodies,
So that subsequent stories can fill in `prepare` / `finalize` / `prune` behaviors against a stable trait contract without introducing a new crate (D4).

**Acceptance Criteria:**

**Given** a new file `crates/stepyard-sandbox-orchestrator/src/workspace.rs`
**When** inspected
**Then** it defines `pub trait WorkspaceManager: Send + Sync + 'static` with `#[async_trait]` (project convention)
**And** the trait is `#[non_exhaustive]`-style extensibility via its error type (not the trait itself; traits can't carry that attribute) — future-proof via `#[async_trait]` default impls
**And** the trait declares three methods: `async fn prepare(&self, session_id: &SessionId, strategy: &BranchStrategy) -> Result<Workspace, WorkspaceError>`, `async fn finalize(&self, workspace: &Workspace, outcome: WorkflowOutcome) -> Result<FinalizeReport, WorkspaceError>`, `async fn prune(&self) -> Result<PruneReport, WorkspaceError>`

**Given** the same file
**When** the supporting types are inspected
**Then** `BranchStrategy` is a `#[non_exhaustive]` enum with variants `Head`, `MergeToHead { target: String }`, `NamedBranch { name: String }` (snake_case in YAML per D10)
**And** `Workspace` is a struct `{ pub path: PathBuf, pub branch: Option<String>, pub session_id: SessionId }`
**And** `WorkflowOutcome` enum with `Success`, `Failure` variants
**And** `FinalizeReport` + `PruneReport` are structs with timestamp + counts (extensible, `#[non_exhaustive]`)

**Given** `WorkspaceError` co-located in the same file (per structural requirements — errors live with types, not separate `workspace_errors.rs`)
**When** inspected
**Then** it derives `thiserror::Error` (NOT `anyhow` — library crate per NFR21)
**And** is `#[non_exhaustive]` with variants: `GitCommand { op: String, stderr: String }`, `WorktreeExists { path: PathBuf }`, `UncommittedChanges { path: PathBuf }`, `TargetBranchNotFound { target: String }`, `MergeConflict { files: Vec<String> }`, `Io { source: std::io::Error }`
**And** `#[error("…")]` messages are lowercase, no trailing punctuation

**Given** `GitWorktreeManager` struct in the same file
**When** inspected
**Then** it has `pub fn new(repo_root: PathBuf, workspaces_dir: PathBuf, retention_hours: u64) -> Self`
**And** implements `WorkspaceManager` with each method returning `unimplemented!("Story 4.{N}")` — stubs will be filled in sequentially
**And** the module is re-exported from `crates/stepyard-sandbox-orchestrator/src/lib.rs`

**Given** a unit test in the same file (`#[cfg(test)] mod tests`)
**When** the test constructs `let mgr: Arc<dyn WorkspaceManager> = Arc::new(GitWorktreeManager::new(...));`
**Then** it compiles (trait is object-safe)
**And** asserts trait methods are reachable via `Arc<dyn WorkspaceManager>`
**And** contains no `tokio::time::sleep(…)` calls (Rule 7a)

Coverage: FR13 infrastructure, D4

### Story 4.2: Implement `GitWorktreeManager::prepare` with `WorkspacePrepared` + `BranchCreated` Events

As a workflow runtime,
I want `Engine` to emit `WorkspacePrepared` (and `BranchCreated` when a branch is actually created) synchronously before invoking `GitWorktreeManager::prepare`, which then runs `git worktree add` via argv-only subprocess,
So that every workspace and branch decision is in the session log before the git IO happens, and parallel agents operate on isolated working trees.

**Acceptance Criteria:**

**Given** `crates/stepyard-core/src/event.rs`
**When** inspected
**Then** two new variants exist: `WorkspacePrepared { path: String, strategy: String }` and `BranchCreated { branch: String, base: String }`, both with `#[serde(rename_all = "snake_case")]`
**And** strategy serializes as `"head" | "merge_to_head" | "named_branch"` (snake_case per D10)
**And** subscribers at `src/events/subscribers.rs` and `src/cli/display.rs` gain explicit match arms (workspace lint enforces)
**And** CLI display renders e.g. `"workspace prepared at {path} (strategy: {strategy})"` and `"branch created: {branch} from {base}"` (lowercase, no trailing punctuation)

**Given** `Engine` orchestrates workspace preparation for a new session
**When** the session starts
**Then** it synchronously `session.append(Event::WorkspacePrepared { path: target_path.display().to_string(), strategy: strategy.as_str().to_string() }).await?` BEFORE calling `workspace_manager.prepare(...).await?`
**And** for `BranchStrategy::MergeToHead { target }` or `BranchStrategy::NamedBranch { name }`, it ALSO synchronously `session.append(Event::BranchCreated { branch: resolved_branch, base: resolved_base }).await?` BEFORE the prepare call
**And** both emits happen on the same `.await` chain as the subsequent IO (no `tokio::spawn`)
**And** for `BranchStrategy::Head`, no `BranchCreated` is emitted (no branch created)

**Given** `GitWorktreeManager::prepare` in `workspace.rs`
**When** inspected
**Then** it runs `git worktree add` via `tokio::process::Command::new("git")` with argv-only (never `sh -c`)
**And** for `Head`: argv is `["worktree", "add", <path>, "HEAD"]`
**And** for `NamedBranch { name }`: argv is `["worktree", "add", "-b", <name>, <path>, <repo_head_ref>]`
**And** for `MergeToHead { target }`: argv is `["worktree", "add", "-b", format!("stepyard/session-{session_uuid}"), <path>, <target>]` (temp branch named after session UUID)
**And** stderr on non-zero exit maps to `WorkspaceError::GitCommand { op: "worktree add".into(), stderr }`

**Given** NFR5 (worktree creation performance)
**When** `prepare` is benchmarked against a small repo
**Then** p50 wall-clock <5s per worktree (acceptable for workflow startup, not hot path)

**Given** an integration test at `crates/stepyard-sandbox-orchestrator/tests/worktree_prepare.rs` (opt-in — requires real git binary + temp repo)
**When** the test creates a temp git repo, runs `prepare` with `BranchStrategy::NamedBranch { name: "feat/test".into() }`
**Then** `<workspaces_dir>/stepyard-session-<uuid>` exists as a git worktree
**And** `git branch` inside that worktree shows `feat/test` checked out
**And** running `prepare` twice with the same session_id returns `Err(WorkspaceError::WorktreeExists { .. })` (no silent overwrite)
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)

**Given** an integration test for the Engine emit-before-IO ordering
**When** the test uses a fake `WorkspaceManager` recording `prepare()` invocations and the session's event log
**Then** `Event::WorkspacePrepared` appears in the log BEFORE `prepare()` was invoked
**And** for `NamedBranch`, `Event::BranchCreated` also precedes `prepare()`
**And** the test asserts on the exact event ordering (event N comes before event N+1)

Coverage: FR13, FR22 (WorkspacePrepared + BranchCreated), FR23 (git operations as events), NFR5

### Story 4.3: Workflow `branch_strategy:` YAML Schema and CLI Override

As a workflow author,
I want a top-level `branch_strategy:` YAML field (`head | merge_to_head | named_branch`) with a required sibling `branch_name:` when using `named_branch`, plus a `--branch-strategy` CLI flag that overrides YAML per run,
So that I can declare how agent commits land in the repo without hardcoding per-workflow branching logic.

**Acceptance Criteria:**

**Given** the `Workflow` struct in `crates/stepyard-core/src/workflow.rs` (or wherever it lives)
**When** inspected
**Then** it gains `#[serde(default)] pub branch_strategy: BranchStrategyYaml` (using snake_case serde — D10 explicit)
**And** `BranchStrategyYaml` enum has `#[serde(rename_all = "snake_case")]` with variants `Head`, `MergeToHead`, `NamedBranch`
**And** a sibling `#[serde(default)] pub branch_name: Option<String>` field (validated separately)
**And** a sibling `#[serde(default)] pub base_branch: Option<String>` field (for `merge_to_head` target; defaults to `"main"` if None)
**And** default value is `BranchStrategyYaml::Head` (backward compat — workflows without the field run on HEAD)

**Given** YAML parsing
**When** `branch_strategy: named_branch` is specified without `branch_name:`
**Then** parsing produces `EngineError::PlaceholderUnresolved { key: "branch_name".into(), found_at: "<workflow-file>".into() }`
**And** parsing fails fast — no workflow executes with an ambiguous named-branch strategy
**And** `branch_name: "{{BRANCH_NAME}}"` placeholder is preserved as a literal for Epic 5's template substitution to resolve later (this story does NOT resolve `{{KEY}}` — that's Epic 5)

**Given** CLI flag `--branch-strategy`
**When** the user invokes `stepyard run --branch-strategy head` / `--branch-strategy merge-to-head` / `--branch-strategy named-branch:feat/foo`
**Then** clap parses the value into `BranchStrategy` (note: CLI uses kebab-case `named-branch`, YAML uses snake_case `named_branch` — both standard for their formats)
**And** for `named-branch:<name>`, the `<name>` portion populates `BranchStrategy::NamedBranch { name: <name> }`
**And** the CLI value overrides the workflow YAML `branch_strategy:` field
**And** invalid formats (e.g., `--branch-strategy foo`) produce a clap parse-time error

**Given** Engine's session-setup path
**When** resolving the effective strategy
**Then** precedence is: CLI flag > workflow YAML > default (`Head`)
**And** the resolved `BranchStrategy` value passes to `workspace_manager.prepare(session_id, &strategy)` (Story 4.2)
**And** the same resolved value appears in `Event::WorkspacePrepared { strategy }` (one source of truth)

**Given** proptest coverage for CLI parsing
**When** random inputs test `--branch-strategy <arbitrary-string>`
**Then** the parser either produces a valid `BranchStrategy` or rejects with a parse-time error (never panics)
**And** proptest lives in `crates/stepyard/src/cli/branch_strategy.rs` (or equivalent) `#[cfg(test)] mod proptest_tests` (proptest required for CLI argument parsing per testing-enforcement invariant)

**Given** a unit test at `crates/stepyard-core/src/workflow.rs`
**When** the test parses YAML fixtures: `branch_strategy: head`, `branch_strategy: merge_to_head`, `branch_strategy: named_branch\nbranch_name: feat/test`
**Then** each parses successfully with the expected struct value
**And** a fixture with `branch_strategy: named_branch` MISSING `branch_name:` produces `EngineError::PlaceholderUnresolved`
**And** a fixture without any `branch_strategy:` field parses with default `Head`

Coverage: FR14, D10

### Story 4.4: Auto-Merge on `MergeToHead` and Conflict Preservation with `MergeAttempted`/`MergeConflict` Events

As a workflow author,
I want successful `merge_to_head` sessions to auto-merge the temp branch back to the target with `MergeAttempted` emitted first, and conflicts to preserve the temp branch + emit `MergeConflict` with the affected files,
So that parallel agents converge automatically on clean work and conflicts are surfaced for manual resolution without losing the branch.

**Acceptance Criteria:**

**Given** `crates/stepyard-core/src/event.rs`
**When** inspected
**Then** two new variants exist: `MergeAttempted { source: String, target: String }` and `MergeConflict { source: String, target: String, files: Vec<String> }`, both with `#[serde(rename_all = "snake_case")]`
**And** subscribers at `src/events/subscribers.rs` and `src/cli/display.rs` gain explicit match arms (workspace lint enforces)
**And** CLI display renders e.g. `"merge attempted: {source} → {target}"` and `"merge conflict: {source} → {target} ({n} files)"` (lowercase, no trailing punctuation)

**Given** `GitWorktreeManager::finalize` is invoked with `WorkflowOutcome::Success` on a session that used `BranchStrategy::MergeToHead { target }`
**When** the finalize path runs
**Then** BEFORE any git merge IO, `Engine` synchronously `session.append(Event::MergeAttempted { source: temp_branch.clone(), target: target.clone() }).await?`
**And** on the same `.await` chain, `GitWorktreeManager` runs `git -C <repo_root> merge --no-ff <temp_branch>` via argv-only subprocess (operating on the main working tree at `target`, not the worktree)
**And** on exit code 0 (clean merge): finalize returns `FinalizeReport { merged: true, conflicts: vec![] }`
**And** on exit code non-zero stderr containing "CONFLICT" (or `git merge` returns exit code 1 with conflict markers detectable via `git diff --name-only --diff-filter=U`): enter the conflict path (below)

**Given** a merge conflict
**When** the finalize path hits it
**Then** `Engine` synchronously `session.append(Event::MergeConflict { source: temp_branch, target, files }).await?` where `files = git diff --name-only --diff-filter=U` output
**And** the temp branch is PRESERVED — no `git branch -D stepyard/session-<uuid>`, no `git worktree remove`
**And** the main working tree at `target` is reset to pre-merge state via `git merge --abort` (so the working tree is clean for the user's next operation)
**And** `finalize` returns `Err(WorkspaceError::MergeConflict { files })` (not `Ok` — conflict IS a failure from the engine's POV, even though the branch survives)

**Given** the session's event log after a conflicted merge
**When** `Session::replay()` is called
**Then** the reconstructed state shows `MergeAttempted` followed by `MergeConflict` with the same `source`/`target` pair
**And** the `files: Vec<String>` list is identical to `git diff --name-only --diff-filter=U` output (deterministic)
**And** NFR14 (worktree safety) — the temp branch and worktree still exist on disk for manual inspection

**Given** `BranchStrategy::Head` or `BranchStrategy::NamedBranch` on finalize
**When** finalize runs
**Then** no merge is attempted (Head: no branch to merge; NamedBranch: user owns the branch, stepyard doesn't touch it)
**And** no `MergeAttempted` / `MergeConflict` events are emitted
**And** finalize returns `FinalizeReport { merged: false, conflicts: vec![] }` with appropriate strategy flag

**Given** an integration test at `crates/stepyard-sandbox-orchestrator/tests/worktree_merge.rs` (opt-in — uses real git)
**When** the test creates a temp repo with two commits on `main`, starts a session with `MergeToHead { target: "main".into() }`, makes a conflicting change on the temp branch, then finalizes
**Then** `finalize` returns `Err(WorkspaceError::MergeConflict { files })` with the expected conflict file list
**And** the temp branch `stepyard/session-<uuid>` still exists post-finalize (`git branch --list` confirms)
**And** the session event log contains `MergeAttempted` immediately followed by `MergeConflict` with matching source/target
**And** a second test with a clean merge returns `Ok(FinalizeReport { merged: true, .. })` and the temp branch is cleaned up afterwards (if strategy dictates — document behavior)
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)

Coverage: FR15, FR16, FR22 (MergeAttempted + MergeConflict), FR23, NFR14

### Story 4.5: D8 Two-Phase Startup Prune with `WorkspacePruned` Event and Uncommitted-Changes Preservation

As a platform operator,
I want the workspace pruning slot in `src/startup.rs` phase 3 (stubbed in Story 2.4) to execute the D8 two-phase protocol — `git worktree prune` then filesystem walk — skipping any worktree with uncommitted changes and emitting `WorkspacePruned` for each removed dir,
So that stale worktrees from crashed sessions are reclaimed without risking loss of uncommitted work.

**Acceptance Criteria:**

**Given** `crates/stepyard-core/src/event.rs`
**When** inspected
**Then** one new variant exists: `WorkspacePruned { path: String, reason: String }` with `#[serde(rename_all = "snake_case")]`
**And** subscribers at `src/events/subscribers.rs` and `src/cli/display.rs` gain explicit match arms
**And** CLI display renders `"workspace pruned: {path} ({reason})"` (lowercase, no trailing punctuation)
**And** `reason` values are controlled strings from a small set: `"orphan_no_git_entry"`, `"stale_past_retention"`, `"git_prune_dangling"` (no free-form strings — enforced via helper constructor)

**Given** `src/startup.rs` phase 3 (the stub from Story 2.4)
**When** replaced
**Then** the first sub-phase runs `git -C <repo_root> worktree prune` via `tokio::process::Command` argv-only (cleans dangling git metadata)
**And** exit-code-0 / stderr "nothing to prune" both treated as success (idempotent)
**And** the second sub-phase walks `<workspaces_dir>/stepyard-session-*` directory entries

**Given** the filesystem walk sub-phase
**When** each directory entry is evaluated
**Then** if it has no matching `git worktree list --porcelain` entry AND its mtime is older than `HarnessConfig::workspace_retention_hours` (default 24h, D8) → candidate for removal
**And** for each candidate, run `git -C <path> status --porcelain` — if output is non-empty (uncommitted changes): skip removal, log via `tracing::warn!(path = %path, "worktree preserved due to uncommitted changes")`, NOT emit a `WorkspacePruned` event (event is reserved for actual removals)
**And** if status is clean: synchronously append `Event::WorkspacePruned { path: path.display().to_string(), reason: "orphan_no_git_entry".into() }` (reconcile context exempts this from session-log emit-before-IO, but the event is still logged to a workspace-level audit trail per architecture — a dedicated `reconcile_log` sink or stdout trace)

**Given** NFR6 (worktree pruning performance)
**When** the phase runs against 50 stale worktrees
**Then** total wall-clock <30s (includes 50 `git status --porcelain` subprocess calls)
**And** phase is idempotent: second invocation produces zero additional removals
**And** the phase runs at startup only — NOT per-session, NOT on a timer (D8)

**Given** NFR14 (worktree safety)
**When** a worktree has uncommitted changes
**Then** it is NEVER removed automatically by this phase
**And** the operator can manually inspect / resolve / remove it after the fact
**And** a `tracing::warn!` with structured fields (`path`, `uncommitted_files_count`) makes it discoverable in logs

**Given** the Story 2.4 comment `// TODO(Epic 4): D8 two-phase prune`
**When** this story lands
**Then** the TODO is removed and replaced with the implementation
**And** the `// Exempt from emit-before-IO rule: runs before any live session exists at startup` comment on `reconcile()` still applies (covers this phase too)

**Given** an integration test at `tests/startup_reconcile.rs` (extending Story 2.4's test file)
**When** the test seeds `<workspaces_dir>/` with three directories: one valid git worktree (present in `git worktree list`), one orphan older than retention (no git entry, mtime 48h ago), one orphan with uncommitted changes
**Then** after `reconcile()` runs: the valid worktree survives; the stale orphan is removed with `WorkspacePruned { reason: "orphan_no_git_entry", .. }` emitted; the orphan with uncommitted changes is PRESERVED with a warning trace
**And** a second `reconcile()` call produces zero additional prunes (idempotency)
**And** the test uses `tempfile::TempDir` for the workspaces directory
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** the test contains no `tokio::time::sleep(…)` calls (Rule 7a)

Coverage: FR17, FR18, FR22 (WorkspacePruned), NFR6, NFR14, D8

## Epic 5: Workflow Templating & Idle Detection

One workflow YAML runs across N projects via `{{KEY}}` placeholder substitution (CLI `--var KEY=VAL` flags or `.stepyard/defaults.yaml`). Placeholders are validated pre-execution and fail fast with a clear error. Idle agents (no stdout output for a configurable threshold) are detected via output-based timeout complementing Epic 1's wall-clock timeout. Completion-signal strings let agents exit iteration loops early. Introduces `stepyard_core::template::substitute` pre-parse pass (D7) with YAML-safe substitution and mandatory proptest coverage. Serves Journey 4 (workflow author parameterization).

**Phase:** Growth
**FRs covered:** FR2, FR19, FR20, FR21, FR4 (partial — idle-timeout reason), FR22 (partial — `IdleTimeoutFired` variant)

### Story 5.1: Add `IdleTimeoutFired` Event + `ExecOptions` Type + `exec_with_options` Default-Impl

As an engine maintainer,
I want `IdleTimeoutFired { step_index, idle_threshold_ms }` added to the event enum, an `ExecOptions { env, idle_timeout }` struct in the lifecycle module, and an `exec_with_options` default-impl method on `SandboxLifecycle` that delegates to `exec_with_env` (ignoring `idle_timeout`),
So that Story 5.2 can wire real streaming idle detection without breaking the existing `exec_with_env` signature (D3 extension pattern).

**Acceptance Criteria:**

**Given** `crates/stepyard-core/src/event.rs`
**When** inspected
**Then** it gains `Event::IdleTimeoutFired { step_index: u32, idle_threshold_ms: u64 }` with `#[serde(rename_all = "snake_case")]`
**And** subscribers at `src/events/subscribers.rs` and `src/cli/display.rs` gain explicit match arms (workspace `non_exhaustive_omitted_patterns = "deny"` lint enforces)
**And** CLI display renders `"step {step_index} idle for {idle_threshold_ms}ms — terminated"` (lowercase, no trailing punctuation)
**And** this completes the D5 set of 8 new event variants (StepTimeoutFired Epic 1 + SignalReceived Epic 2 + 5 workspace events Epic 4 + IdleTimeoutFired Epic 5 = 8)

**Given** `crates/stepyard-sandbox-orchestrator/src/lib.rs` (or wherever `SandboxLifecycle` lives)
**When** inspected
**Then** a new struct `ExecOptions { pub env: HashMap<String, String>, pub idle_timeout: Option<Duration> }` exists, deriving `Debug, Clone, Default`
**And** the trait gains `async fn exec_with_options(&self, id: &SandboxId, cmd: &[String], opts: &ExecOptions) -> Result<ExecOutput, SandboxError>`
**And** the default impl is `self.exec_with_env(id, cmd, &opts.env).await` (idle_timeout ignored — preserves Story 3.1 behavior for unmigrated impls)
**And** the trait retains `#[async_trait]` (project convention)
**And** the existing `exec_with_env` (Story 3.1) and `exec` signatures are NOT changed (D3 extension via new method)

**Given** `SandboxError` in the same module
**When** inspected
**Then** it gains `IdleTimeout { idle_ms: u64 }` variant
**And** the variant is `#[non_exhaustive]`-safe (added before the `#[non_exhaustive]` boundary the existing enum already uses)
**And** `#[error("…")]` message is lowercase, no trailing punctuation: `"sandbox idle for {idle_ms}ms — terminated by orchestrator"`

**Given** the `Step` struct (Epic 3 / Story 3.3 already added `env`)
**When** inspected
**Then** it gains `#[serde(default)] pub idle_timeout: Option<u64>` (milliseconds)
**And** Engine plumbs `step.idle_timeout` into `ExecOptions::idle_timeout` (`step.idle_timeout.map(Duration::from_millis)`)
**And** existing workflows without `idle_timeout:` continue to parse (backward compat per NFR18)

**Given** the mock-extension safeguard (testing-enforcement invariant)
**When** `MockLifecycle` is extended
**Then** `MockLifecycleCall::ExecWithOptions { id: SandboxId, cmd: Vec<String>, opts: ExecOptions }` is added as a variant
**And** `MockLifecycle::exec_with_options` override records the FULL `opts` struct (not just env, not just idle_timeout — the whole thing)
**And** at least one unit test asserts on `opts.idle_timeout` to prevent silent regression where the default impl drops the field

**Given** a unit test at `crates/stepyard-sandbox-orchestrator/src/mock.rs`
**When** the test calls `mock.exec_with_options(&id, &cmd, &ExecOptions { env: env_map, idle_timeout: Some(Duration::from_secs(30)) }).await`
**Then** `MockLifecycleCall::ExecWithOptions { opts, .. }` records `opts.idle_timeout == Some(Duration::from_secs(30))`
**And** calling on a type that did NOT override `exec_with_options` records the `ExecWithEnv` call instead (proves default delegation)
**And** the test does not use `tokio::time::sleep(…)` (Rule 7a)

Coverage: FR2 infrastructure, FR22 (IdleTimeoutFired completes the D5 set), D3 extension pattern

### Story 5.2: Implement `DockerLifecycle::exec_with_options` Streaming and Engine Wiring for `IdleTimeoutFired`

As a workflow author,
I want the engine to detect when an agent stops producing stdout for `idle_timeout` milliseconds, synchronously emit `IdleTimeoutFired`, then destroy the container and return `EngineError::StepFailed { reason: TerminationReason::IdleTimeout { idle_ms } }`,
So that idle agents (e.g., infinite waits, deadlocks) terminate deterministically without indefinite resource consumption.

**Acceptance Criteria:**

**Given** `DockerLifecycle::exec_with_options` in `crates/stepyard-sandbox-orchestrator/src/docker.rs`
**When** inspected
**Then** it builds `tokio::process::Command::new("docker")` with `.stdout(Stdio::piped()).stderr(Stdio::piped())`
**And** for each `--env K=V` pair (sorted by key), argv-only as in Story 3.2
**And** spawns the child via `.spawn()`
**And** uses `tokio::io::AsyncBufReadExt::read_until(b'\n', &mut buf)` wrapped in `tokio::time::timeout(idle_timeout)` per read iteration
**And** the timer resets on every successful read (any byte received)
**And** on read timeout: kills the child via `child.start_kill()` + `child.wait().await`, returns `Err(SandboxError::IdleTimeout { idle_ms: idle_timeout.as_millis() as u64 })`
**And** when `idle_timeout` is `None`: skips the timeout wrapper entirely (acts identically to `exec_with_env`)

**Given** Engine's step executor invocation in `crates/stepyard-harness/src/engine.rs`
**When** it calls `lifecycle.exec_with_options(&self.sandbox_id, cmd, &opts).await`
**Then** on `Err(SandboxError::IdleTimeout { idle_ms })`, Engine synchronously calls `self.session.append(Event::IdleTimeoutFired { step_index, idle_threshold_ms: idle_ms }).await?` BEFORE the destroy call
**And** on the same `.await` chain, calls `self.lifecycle.destroy(&self.sandbox_id).await` (idempotent per NFR12 — tolerates `ContainerNotFound` since the lifecycle already killed the inner subprocess)
**And** returns `Err(EngineError::StepFailed { step_index, reason: TerminationReason::IdleTimeout { idle_ms } })` (Story 1.2's taxonomy)
**And** the emit-before-IO ordering is never reversed (no `tokio::spawn`)

**Given** the per-step interaction with the wall-clock `tokio::time::timeout` from Story 1.4
**When** both `timeout` (wall-clock) AND `idle_timeout` are set on the same step
**Then** both wrappers compose: wall-clock `timeout` wraps the entire `exec_with_options` future; `idle_timeout` is applied per-read inside the lifecycle
**And** whichever fires first determines the termination reason: `StepTimeout` (wall-clock) vs `IdleTimeout` (idle)
**And** the corresponding event (`StepTimeoutFired` or `IdleTimeoutFired`) is emitted, not both

**Given** the idle detection captures stdout/stderr
**When** the step completes normally (no idle, no wall-clock timeout)
**Then** `ExecOutput { stdout, stderr, exit_code }` is returned with the buffered output
**And** `stdout` contains every byte emitted by the agent (no truncation due to streaming)

**Given** an integration test at `crates/stepyard-harness/tests/idle_timeout.rs`
**When** the test uses a `MockLifecycle` configured to return `Err(SandboxError::IdleTimeout { idle_ms: 30000 })` from `exec_with_options`
**Then** Engine's session log contains `Event::IdleTimeoutFired { step_index: 0, idle_threshold_ms: 30000 }` BEFORE `MockLifecycleCall::Destroy`
**And** the returned error is `EngineError::StepFailed { reason: TerminationReason::IdleTimeout { idle_ms: 30000 }, .. }`
**And** the test uses `#[tokio::test(start_paused = true)]` (Rule 7a) with no `tokio::time::sleep(…)` calls

**Given** an integration test at `crates/stepyard-sandbox-orchestrator/tests/docker_idle.rs` (opt-in — requires Docker)
**When** the test runs `exec_with_options(id, &["sleep".to_string(), "300".to_string()], &ExecOptions { idle_timeout: Some(Duration::from_secs(2)), .. })` against a live container
**Then** `exec_with_options` returns `Err(SandboxError::IdleTimeout { idle_ms: 2000 })` within ~2-3 seconds wall-clock (NOT after 300s)
**And** the container's inner `sleep` process is killed (verified via `docker ps`)
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)

Coverage: FR2, FR4 (IdleTimeout reason), FR22 (IdleTimeoutFired emission path)

### Story 5.3: `{{KEY}}` Template Substitution Preprocessor with YAML-Safe Output

As a workflow author,
I want a `stepyard_core::template::substitute(text, &vars) -> Result<String, TemplateError>` pre-parse pass that replaces every `{{KEY}}` with the YAML-encoded value of `vars[KEY]` (via `serde_yaml::to_string`), running BEFORE `serde_yaml::from_str`,
So that one workflow YAML file can be parameterized across N projects without risking YAML structure injection from raw value substitution (D7).

**Acceptance Criteria:**

**Given** a new file `crates/stepyard-core/src/template.rs`
**When** inspected
**Then** it exports `pub fn substitute(text: &str, vars: &HashMap<String, String>) -> Result<String, TemplateError>`
**And** the function is pure (no IO, no globals, no `unsafe`, no `tokio::spawn`)
**And** the function recognizes `{{KEY}}` tokens via a deterministic parser (NOT a regex with backtracking — use `nom` or a hand-written scanner per project conventions)
**And** `KEY` matches `[A-Z_][A-Z0-9_]*` (uppercase identifier convention; documented in workflow schema docs)

**Given** YAML-safe substitution
**When** a `{{KEY}}` is replaced
**Then** the value is encoded via `serde_yaml::to_string(&value)` and the resulting YAML scalar is spliced in (not raw-string-interpolated)
**And** if the value contains `:`, ` -`, `&anchor`, `*alias`, `---`, `: `, `\n`, `"`, `'`, the YAML encoding produces a properly quoted/escaped scalar (e.g., `"a: b"` becomes `'a: b'` in single-quoted YAML form)
**And** a trailing newline added by `serde_yaml::to_string` is stripped before splicing (so the substitution sits inline in its containing YAML document)

**Given** `TemplateError` co-located in the same file
**When** inspected
**Then** it derives `thiserror::Error` (NOT `anyhow` per NFR21)
**And** is `#[non_exhaustive]` with variants: `Unresolved { key: String, found_at: String }`, `InvalidPlaceholder { token: String, position: usize }`, `YamlEncoding { source: serde_yaml::Error }`
**And** `#[error("…")]` messages are lowercase, no trailing punctuation
**And** `Unresolved` carries `found_at` for diagnostic context (e.g., `"workflow.steps[2].command"`)

**Given** the workflow loader at `crates/stepyard-core/src/workflow.rs` (or wherever `Workflow::from_yaml_str` lives)
**When** loading a workflow
**Then** the order is: `template::substitute(raw_yaml, &vars)?` → `serde_yaml::from_str::<Workflow>(&substituted_yaml)?`
**And** if substitution fails, the loader returns `Err(EngineError::PlaceholderUnresolved { .. })` (Story 5.4 maps `TemplateError::Unresolved` to `EngineError::PlaceholderUnresolved`)
**And** if YAML parsing fails AFTER substitution, the loader returns the standard YAML parse error (no special handling — substitution succeeded; the YAML was structurally invalid)

**Given** proptest coverage (testing-enforcement invariant — substitution requires proptest)
**When** proptest tests run at `crates/stepyard-core/src/template.rs` `#[cfg(test)] mod proptest_tests`
**Then** for every random `(text: String, vars: HashMap<String, String>)` input, `substitute` either returns `Ok(_)` or `Err(TemplateError::Unresolved { key, .. })` where `key` is a real `{{KEY}}` substring in `text` and is missing from `vars`
**And** `substitute` NEVER panics
**And** if `substitute(text, &vars)` returns `Ok(s)`, then `s` is parseable as YAML when `text` was parseable as YAML and all values are scalar strings
**And** the proptest seed is logged on failure for reproducibility

**Given** a unit test for security-critical YAML escaping
**When** the test calls `substitute("name: {{ATTACK}}", &{"ATTACK".into() => "x\nfoo: y".into()})`
**Then** the output parses as YAML with `name` field containing the literal string `"x\nfoo: y"` — NOT a top-level `foo: y` field injected (this is the critical assertion that justifies the YAML encoding step)
**And** another test with value `{"ATTACK".into() => "*alias_ref".into()}` produces output where `name` is the literal string `"*alias_ref"`, NOT a YAML alias reference

Coverage: FR19 substitution side, D7, security: YAML-safe substitution

### Story 5.4: CLI `--var KEY=VAL` Flag + Defaults Source + `EngineError::PlaceholderUnresolved` Validation

As a workflow author,
I want a `stepyard run --var KEY=VAL` CLI flag (multi-value) and `.stepyard/defaults.yaml` providing the value sources for `{{KEY}}` substitution, with `EngineError::PlaceholderUnresolved { key, found_at }` failing fast at parse time when any placeholder is missing,
So that I can run one workflow across N projects with explicit per-run parameters and get clear errors when a key is forgotten.

**Acceptance Criteria:**

**Given** the `stepyard run` CLI in the workspace-root binary
**When** inspected
**Then** it accepts `--var KEY=VAL` as a multi-value flag (`clap::ArgAction::Append`)
**And** the parser splits each value on the first `=` (so `--var MSG=hello=world` produces `KEY=MSG`, `VAL=hello=world`)
**And** `KEY` must match `[A-Z_][A-Z0-9_]*` — clap validates at parse time and rejects with a clear error otherwise
**And** invalid forms (e.g., `--var FOO` without `=`) produce a clap parse error (exit code 2)

**Given** the value-source resolution
**When** building the `vars: HashMap<String, String>` for `template::substitute`
**Then** the order is: `.stepyard/defaults.yaml` `vars:` field FIRST (lowest precedence), then CLI `--var KEY=VAL` flags (overrides defaults)
**And** the `defaults.yaml` schema gains `#[serde(default)] pub vars: HashMap<String, String>` alongside the `env:` field from Story 3.3
**And** if `defaults.yaml` is missing, `vars` is `HashMap::new()` (no error — defaults are optional)

**Given** `crates/stepyard-core/src/error.rs`
**When** inspected
**Then** `EngineError` gains `PlaceholderUnresolved { key: String, found_at: String }` variant (already referenced in Story 4.3 and Story 5.3 — this story finalizes it)
**And** `#[error("…")]` message: `"placeholder {{key}} unresolved at {found_at}"` (lowercase, no trailing punctuation; `{{key}}` is the literal `{{X}}` form for clarity)
**And** `From<TemplateError>` impl maps `TemplateError::Unresolved { key, found_at }` → `EngineError::PlaceholderUnresolved { key, found_at }`

**Given** placeholder validation
**When** the workflow loader encounters a `{{KEY}}` reference whose `KEY` is not in the resolved `vars` map
**Then** loading FAILS at parse time (before any step executes) with `EngineError::PlaceholderUnresolved { key, found_at }`
**And** `found_at` describes the YAML location (e.g., `"workflow.steps[1].command[2]"`) — derived from the substitution position in the raw YAML text
**And** no partial substitution is attempted: either ALL placeholders resolve, or the loader returns `Err`

**Given** the negative-control extension (per Story 3.5's placeholder section)
**When** `tests/injection_negative.rs` is extended
**Then** a new test case runs `stepyard run --var MSG='$(rm -rf /)'` against a workflow with `command: ["echo", "{{MSG}}"]`
**Then** stdout is literally `$(rm -rf /)\n` (no shell expansion at substitution time — the value passes through to argv)
**And** the host filesystem under `/tmp/` is untouched
**And** another negative test runs `stepyard run --var ATTACK='\nfoo: bar'` with `command: ["echo", "{{ATTACK}}"]`
**Then** the workflow loads cleanly; `echo` outputs the literal string (proving YAML escaping in Story 5.3 worked)

**Given** a unit test at `src/cli/run.rs` (or wherever the run subcommand lives)
**When** the test invokes `stepyard run --var FOO=bar --var BAZ=qux <workflow.yaml>`
**Then** the resolved vars passed to `template::substitute` is `{"FOO": "bar", "BAZ": "qux"}`
**And** if `defaults.yaml` had `vars: { BAZ: from-defaults, ONLY: x }`, the resolved vars is `{"FOO": "bar", "BAZ": "qux", "ONLY": "x"}` (CLI overrides defaults; defaults contribute ONLY)
**And** running with `--var BAR=hello` against a workflow referencing `{{MISSING}}` produces `EngineError::PlaceholderUnresolved { key: "MISSING", .. }` at load time
**And** every assert_cmd `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)

Coverage: FR19 CLI side, FR21 (PlaceholderUnresolved validation), D7

### Story 5.5: Completion-Signal String Detection on Agent Stdout

As a workflow author,
I want a per-workflow `completion_signal: "<string>"` YAML field that, when matched as a substring of the agent's stdout, terminates the iteration loop early with a successful exit,
So that agents (e.g., LLM loops) can self-signal task completion without relying solely on subprocess exit codes or external orchestration.

**Acceptance Criteria:**

**Given** the `Workflow` struct in `crates/stepyard-core/src/workflow.rs`
**When** inspected
**Then** it gains `#[serde(default)] pub completion_signal: Option<String>` (top-level field — applies to the whole workflow's iteration loop, not per-step)
**And** existing workflows without `completion_signal:` continue to parse (backward compat per NFR18)
**And** the field is a plain string (no regex; substring match only — keeps semantics simple and safe)

**Given** Engine's iteration loop
**When** a step produces stdout (via the streaming reader from Story 5.2)
**Then** if `workflow.completion_signal` is `Some(signal)` AND any line of agent stdout CONTAINS `signal` as a substring
**Then** Engine synchronously appends a new event `Event::CompletionSignaled { step_index: u32, signal: String }` (added to `crates/stepyard-core/src/event.rs` — note: this is a 9th event variant beyond D5's original 8; rationale documented inline)
**And** the step terminates gracefully — Engine calls `lifecycle.destroy(&self.sandbox_id).await` (idempotent)
**And** the iteration loop exits with success (workflow status `completed`, NOT `failed`)
**And** the emit-before-IO ordering applies: `session.append(Event::CompletionSignaled { .. }).await?` BEFORE the destroy call

**Given** subscribers at `src/events/subscribers.rs` and `src/cli/display.rs`
**When** inspected
**Then** they gain explicit match arms for `Event::CompletionSignaled` (workspace lint enforces)
**And** CLI display renders `"completion signal matched at step {step_index}: {signal}"` (lowercase, no trailing punctuation)

**Given** the emit-before-IO interaction with idle/wall-clock timeouts
**When** completion signal matches WHILE the wall-clock or idle timeout is active
**Then** completion-signal detection fires first (since it's checked on every read in the streaming loop) and short-circuits the timeout wrappers
**And** in the rare race where the timeout future fires concurrently with a signal-match read, the FIRST one to acquire the `select!` arm wins (deterministic from `tokio::select!` semantics — document this race in code comment)

**Given** NFR8 (no credential in logs)
**When** the `signal` field is logged
**Then** the literal `signal` string from the workflow YAML is recorded in `Event::CompletionSignaled` (this is workflow-author-controlled config, not a credential — safe to log)
**And** the matched stdout line itself is NOT included in the event (could contain agent-emitted PII or secrets)

**Given** an integration test at `crates/stepyard-harness/tests/completion_signal.rs`
**When** the test constructs a workflow with `completion_signal: "TASK_COMPLETE"` and uses a `MockLifecycle` whose `exec_with_options` writes `"working...\nTASK_COMPLETE\n"` to a streaming channel
**Then** Engine's session log contains `Event::CompletionSignaled { step_index: 0, signal: "TASK_COMPLETE" }` BEFORE `MockLifecycleCall::Destroy`
**And** the workflow status is `completed` (not `failed`)
**And** the iteration loop did not run a second time (signal terminated after one match)

**Given** another integration test
**When** the agent stdout NEVER contains the signal AND the wall-clock timeout fires
**Then** the wall-clock timeout wins, `StepTimeoutFired` is emitted, and the workflow status is `failed` (Story 1.4's path applies)
**And** no `CompletionSignaled` event is emitted
**And** the test uses virtual time (`#[tokio::test(start_paused = true)]`) and contains no `tokio::time::sleep(…)` (Rule 7a)

Coverage: FR20

**Note on D5 event count:** Story 5.5 adds `CompletionSignaled` as a 9th event variant beyond the original D5 list of 8. This is intentional — completion-signal detection is a behavior introduced in Epic 5 that requires its own audit event, and D5's "8 new variants" list is descriptive (the variants enumerated at decision time), not prescriptive (a hard cap). The architectural pattern (`#[non_exhaustive]`, `#[serde(rename_all = "snake_case")]`, workspace-wide lint) applies identically. Recommend updating architecture.md's D5 narrative to "9 new event variants" in a follow-up doc commit.

## Epic 6: Multi-Provider & Interactive Sandboxes

Swap Docker for Podman or cloud providers (Vercel, Daytona-style) through the existing `SandboxLifecycle` trait — no abstraction-layer redesign. Providers accept a rich configuration object at creation time specifying volume mounts, resource limits, and network policies. Interactive debugging via TTY forwarding for sandbox types that support it. Pure trait-extension epic — validates FR25's claim that the trait was designed for multi-provider from the start.

**Phase:** Expansion
**FRs covered:** FR25, FR26, FR27

### Story 6.1: Implement `PodmanLifecycle` to Validate Multi-Provider Support via Existing Trait

As a platform operator,
I want a `PodmanLifecycle` struct implementing the existing `SandboxLifecycle` trait (without changing the trait), shippable as an alternate provider via a `--sandbox-provider <docker|podman>` CLI flag,
So that Docker-restricted hosts can run stepyard via Podman and we prove FR25's claim that the trait supports multiple providers without abstraction-layer redesign.

**Acceptance Criteria:**

**Given** a new file `crates/stepyard-sandbox-orchestrator/src/podman.rs`
**When** inspected
**Then** it defines `pub struct PodmanLifecycle { /* same shape as DockerLifecycle */ }` and implements `SandboxLifecycle` with `#[async_trait]`
**And** every method (`create`, `destroy`, `exec`, `exec_with_env` from Story 3.1, `exec_with_options` from Story 5.1) is implemented
**And** the implementation uses `tokio::process::Command::new("podman")` with argv-only invocations identical to `DockerLifecycle` (podman CLI is intentionally Docker-compatible — same flags, same semantics for our use cases)
**And** stderr classifier reuses the same `docker_errors.rs` module (or a new `podman_errors.rs` if classification differs — document either choice)
**And** the existing `SandboxLifecycle` trait is NOT modified (validates FR25 — no trait refinement needed for additional provider)

**Given** the workspace-root binary `src/main.rs` and CLI parser
**When** inspected
**Then** a new CLI flag `--sandbox-provider <docker|podman>` is added to `stepyard run` (clap `ValueEnum` with `Docker`, `Podman` variants)
**And** the default is `Docker` (backward compat — existing invocations don't change)
**And** the chosen provider produces a concrete `Arc<dyn SandboxLifecycle>` passed into `Engine::new(HarnessConfig { lifecycle: .., .. })`
**And** the choice is recorded in a `tracing::info!(provider = %provider_name, "sandbox provider selected")` log line at startup (structured field, not format string)

**Given** the legacy carveout pattern (Story 3.2 documented `DockerLifecycle::exec` retains `sh -c`)
**When** `PodmanLifecycle::exec` is implemented
**Then** it does NOT inherit Docker's legacy `sh -c` carveout — Podman is new code, so it uses argv-only from day one
**And** `exec_with_env` and `exec_with_options` are argv-only as in Docker
**And** an inline comment documents: `// Podman is new code — no legacy sh -c carveout (unlike DockerLifecycle::exec)`

**Given** the container naming convention from architecture (`stepyard-session-{uuid}`)
**When** `PodmanLifecycle::create` runs
**Then** it uses the same naming convention (`podman run --name stepyard-session-<uuid>`)
**And** the startup reconcile from Story 2.4 phase 2 works for Podman containers identically (uses `podman ps --filter "name=stepyard-session-*"` when provider is Podman)
**And** `src/startup.rs` reconcile is generalized to take an `Arc<dyn SandboxLifecycle>` rather than a concrete `DockerLifecycle` (or accepts a provider enum that picks the correct CLI binary)

**Given** the mock-extension safeguard
**When** `MockLifecycle` is exercised with this story's changes
**Then** no MockLifecycle changes are needed (it already covers `exec_with_options` from Story 5.1)
**And** existing MockLifecycle tests pass unchanged

**Given** an integration test at `crates/stepyard-sandbox-orchestrator/tests/podman_lifecycle.rs` (opt-in — skips if `podman` binary not on PATH)
**When** the test creates a session via `PodmanLifecycle::create`, runs `exec_with_env` with a non-trivial env, then destroys
**Then** the container is created with the expected `stepyard-session-<uuid>` name (verified via `podman ps`)
**And** `exec_with_env` returns expected stdout from `printenv FOO` matching the env-injected value
**And** `destroy` removes the container; second `destroy` returns idempotent success (NFR12)
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** the test uses `tokio::test(flavor = "multi_thread")` if needed for parallel safety

Coverage: FR25, NFR15 (Docker CLI compatibility extends to Podman CLI compatibility — same argv-only contract)

### Story 6.2: Add `CreateOptions` Struct and `create_with_options` Default-Impl Method

As a workflow author,
I want a `CreateOptions` struct (volume mounts, resource limits, network policy) passed to a new `create_with_options` default-impl method on `SandboxLifecycle`, with workflow YAML support for `sandbox: { volumes, limits, network }`,
So that providers can accept rich creation config without breaking the existing `create` signature (D3 extension pattern applied a fourth time).

**Acceptance Criteria:**

**Given** `crates/stepyard-sandbox-orchestrator/src/lib.rs` (or wherever lifecycle types live)
**When** inspected
**Then** new types exist: `pub struct CreateOptions { pub volumes: Vec<VolumeMount>, pub resource_limits: Option<ResourceLimits>, pub network_policy: NetworkPolicy }`
**And** `pub struct VolumeMount { pub host_path: PathBuf, pub container_path: PathBuf, pub read_only: bool }` deriving `Debug, Clone, Deserialize, Serialize`
**And** `pub struct ResourceLimits { pub memory_bytes: Option<u64>, pub cpu_cores: Option<f64>, pub pids: Option<u64> }` deriving same
**And** `pub enum NetworkPolicy { #[non_exhaustive] Bridge, Host, None, Custom(String) }` deriving `Debug, Clone, Deserialize, Serialize` with `#[serde(rename_all = "snake_case")]`
**And** `CreateOptions` derives `Debug, Clone, Default, Deserialize, Serialize`

**Given** the `SandboxLifecycle` trait
**When** inspected
**Then** it gains `async fn create_with_options(&self, opts: &CreateOptions) -> Result<SandboxId, SandboxError>` as default-impl method
**And** the default impl is `self.create().await` (opts ignored — preserves existing behavior; new providers/configurations are opt-in)
**And** the existing `create()` signature is NOT changed (D3 extension via new method)

**Given** `DockerLifecycle::create_with_options` and `PodmanLifecycle::create_with_options` overrides
**When** inspected
**Then** they translate `CreateOptions` to argv flags: `--volume <host>:<container>[:ro]` per volume mount; `--memory <bytes>` if `resource_limits.memory_bytes`; `--cpus <cores>` if `resource_limits.cpu_cores`; `--pids-limit <n>` if `resource_limits.pids`; `--network <bridge|host|none|<custom>>` from `network_policy`
**And** all flags pass as argv elements (never `sh -c` interpolation) — argv-not-shell rule (NFR9)
**And** invalid combinations (e.g., empty volumes list) are normalized to "no flag" (omit the flag entirely; don't pass `--volume` with empty value)

**Given** the workflow YAML schema
**When** inspected
**Then** the `Workflow` struct gains `#[serde(default)] pub sandbox: SandboxConfig` where `SandboxConfig { #[serde(default)] pub volumes: Vec<VolumeMount>, #[serde(default)] pub limits: Option<ResourceLimits>, #[serde(default)] pub network: NetworkPolicy }` (default `NetworkPolicy::Bridge`)
**And** Engine maps `workflow.sandbox` to `CreateOptions` and calls `lifecycle.create_with_options(&opts)` (instead of `create()`) when any field is non-default
**And** existing workflows without a `sandbox:` field continue to use `create()` directly (zero behavior change for legacy YAML)

**Given** the mock-extension safeguard
**When** `MockLifecycle` is extended
**Then** `MockLifecycleCall::CreateWithOptions { opts: CreateOptions }` is added
**And** `MockLifecycle::create_with_options` override records the FULL `opts` struct
**And** at least one unit test asserts on `opts.volumes` / `opts.resource_limits.memory_bytes` to prevent silent regression

**Given** a unit test at `crates/stepyard-core/src/workflow.rs`
**When** the test parses a YAML fixture with `sandbox: { volumes: [{host_path: /tmp/x, container_path: /workspace, read_only: false}], limits: { memory_bytes: 1073741824 }, network: bridge }`
**Then** the parsed `Workflow.sandbox.volumes[0]` matches the input
**And** `Workflow.sandbox.limits.memory_bytes == Some(1073741824)`
**And** `Workflow.sandbox.network == NetworkPolicy::Bridge`
**And** an existing fixture without `sandbox:` parses with `Workflow.sandbox` at default (empty volumes, no limits, Bridge network)

**Given** an integration test at `crates/stepyard-sandbox-orchestrator/tests/create_with_options.rs` (opt-in — requires Docker)
**When** the test calls `create_with_options(&CreateOptions { volumes: vec![..], resource_limits: Some(ResourceLimits { memory_bytes: Some(256 * 1024 * 1024), .. }), .. })`
**Then** the resulting container has `--memory=256m` flag (verified via `docker inspect <id>` parsing memory limit)
**And** the volume mount appears in `docker inspect` mount list
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)

Coverage: FR26, NFR9 (sandbox boundary preserved), NFR22 (backward compat via default-impl)

### Story 6.3: TTY Forwarding via `exec_interactive` Default-Impl Method

As a DevOps engineer,
I want a new `exec_interactive` default-impl method on `SandboxLifecycle` (returning `Err(InteractiveNotSupported)` by default) overridden by `DockerLifecycle` and `PodmanLifecycle` to use `docker exec -it` / `podman exec -it` for TTY-forwarded interactive sessions, exposed via a `stepyard exec --interactive <session-id>` CLI subcommand,
So that I can debug a running session interactively without bypassing stepyard's container abstraction.

**Acceptance Criteria:**

**Given** the `SandboxLifecycle` trait
**When** inspected
**Then** it gains `async fn exec_interactive(&self, id: &SandboxId, cmd: &[String]) -> Result<ExitCode, SandboxError>` as default-impl method
**And** the default impl returns `Err(SandboxError::InteractiveNotSupported)` (a new variant added in this story)
**And** existing providers without TTY support get the no-op default; only opt-in providers override

**Given** `SandboxError`
**When** inspected
**Then** it gains `InteractiveNotSupported` variant
**And** `#[error("…")]` message is lowercase, no trailing punctuation: `"sandbox provider does not support interactive sessions"`
**And** the variant is `#[non_exhaustive]`-safe

**Given** `DockerLifecycle::exec_interactive` and `PodmanLifecycle::exec_interactive` overrides
**When** inspected
**Then** they invoke `docker exec -it <container> <cmd...>` / `podman exec -it <container> <cmd...>` via `tokio::process::Command` argv-only
**And** the child process inherits stdin/stdout/stderr from the parent (`Stdio::inherit()` for all three)
**And** the parent process awaits the child's exit and returns the exit code wrapped in `Ok(ExitCode::from(code))`
**And** if the container does not exist, returns `Err(SandboxError::ContainerNotFound)` (existing variant from Docker error classifier)

**Given** a new CLI subcommand `stepyard exec --interactive <session-id> -- <cmd...>`
**When** inspected
**Then** clap parses it as `ExecInteractiveArgs { session_id: SessionId, cmd: Vec<String> }`
**And** the handler queries PG for the session's container ID (NOT in-memory registry — D1 invariant)
**And** if the session is not in `status='running'`, returns an error: `"session is not running (status: {status})"`
**And** otherwise calls `lifecycle.exec_interactive(&sandbox_id, &cmd).await` and exits with the returned code

**Given** the cancel/signal-handler interaction (Epic 2)
**When** an interactive session is in progress AND SIGTERM fires
**Then** the broadcast channel is fired (Story 2.2)
**And** the parent `stepyard exec --interactive` invocation receives the signal — clap/tokio's `tokio::signal` propagates it to the child process via terminal control (Ctrl+C → child via TTY)
**And** the parent process exits with code 130 (SIGINT) or 143 (SIGTERM) per Story 2.2's exit code convention

**Given** the mock-extension safeguard
**When** `MockLifecycle` is extended
**Then** `MockLifecycleCall::ExecInteractive { id: SandboxId, cmd: Vec<String> }` is added
**And** `MockLifecycle::exec_interactive` override records the call (returns mock `ExitCode::SUCCESS` or configurable)
**And** at least one unit test asserts on the recorded `cmd` to prevent silent regression

**Given** an integration test at `tests/exec_interactive_cli.rs` using `assert_cmd` (opt-in — requires Docker + a running session fixture)
**When** the test seeds PG with a running session whose container exists and invokes `stepyard exec --interactive <session-id> -- echo hello`
**Then** stdout contains `hello\n`
**And** exit code is 0
**And** invoking the same command against a non-running session returns a clear error and exit code != 0
**And** invoking against a session whose container was destroyed returns `"sandbox container not found"` and exit code != 0
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)

**Given** TTY-specific limitations
**When** the documentation describes `exec_interactive`
**Then** it notes: stdin/stdout MUST be a TTY for `docker exec -it` to work correctly — if the parent process's stdin is piped (not a TTY), the `-it` flag may produce an error or non-interactive behavior
**And** the CLI subcommand prints a warning if `!io::stdin().is_terminal()` (using `std::io::IsTerminal` trait — Rust 1.70+, available in workspace pin 1.75)

Coverage: FR27, NFR9 (sandbox boundary preserved — interactive runs through `docker/podman exec`, not direct host access)
