---
stepsCompleted: ['step-01-init', 'step-02-discovery', 'step-02b-vision', 'step-02c-executive-summary', 'step-03-success', 'step-04-journeys', 'step-05-domain', 'step-06-innovation', 'step-07-project-type', 'step-08-scoping', 'step-09-functional', 'step-10-nonfunctional', 'step-11-polish', 'step-12-complete']
completedAt: '2026-04-16T00:00:00Z'
vision:
  summary: "Most robust, crash-safe AI agent orchestrator — autonomous agents run in isolation, always clean up, never lose state"
  differentiator: "Sandcastle-style DX (pluggable providers, branch strategies, process safety) on top of Minion's unique append-only session durability"
  coreInsight: "Ergonomics and safety are not at odds with persistence and auditability — you can have both"
inputDocuments:
  - _bmad-output/planning-artifacts/architecture.md
  - _bmad-output/engine-v2/epics.md
  - ARCHITECTURE-MINION-ENGINE.md
  - minion-engine/ARCHITECTURE.md
  - _bmad-output/agent-dashboard/prd.md
  - external-ref:sandcastle-codebase-analysis
workflowType: 'prd'
project_name: 'Minion Engine — Sandcastle-Inspired Features'
user_name: 'Bruno'
date: '2026-04-15'
documentCounts:
  briefs: 0
  research: 0
  brainstorming: 0
  projectDocs: 5
classification:
  projectType: developer_tool
  domain: general
  complexity: high
  projectContext: brownfield
---

# Product Requirements Document - Minion Engine — Sandcastle-Inspired Features

**Author:** Bruno
**Date:** 2026-04-15

## Executive Summary

Minion Engine is a Rust-based workflow orchestrator that runs AI coding agents (Claude Code, Codex) inside Docker sandboxes with persistent, append-only session logs. Engine v2 (partially complete: session persistence, harness step/resume/cancel, sandbox orchestrator) provides crash-safe execution — but lacks the operational safety and developer ergonomics required for production autonomous agent workloads.

This PRD defines engine-level improvements inspired by Sandcastle (`@ai-hero/sandcastle`), a TypeScript framework for isolated AI agent execution. Analysis of Sandcastle revealed six critical gaps in Minion Engine: no step timeout (stuck agents block forever), orphaned containers on process crash (no signal handler), no environment variable passing to sandboxes, no git workspace isolation for parallel agents, no branch strategy abstraction, and no prompt templating for reusable workflows.

Target users: Bruno's team at EdenRed and Afya, running code review, bug fix, and security audit workflows via Slack bot dispatch to a KingHost VPS. The improvements directly address production incidents: agents that hang indefinitely, containers accumulating after crashes, and the inability to run parallel agents on separate branches.

### What Makes This Special

Unlike Sandcastle — which is ephemeral (run once, merge, discard) — Minion Engine persists every step as an append-only event log in PostgreSQL. Sessions survive process crashes and can be resumed from the exact step where they stopped. The Sandcastle-inspired features add what's missing on top of this durability: pluggable sandbox providers (Docker, Podman, future cloud runtimes), git worktree management with typed branch strategies (head, merge-to-head, named-branch), process-level signal handlers that guarantee container cleanup, step timeout that kills stuck agents, and environment variable cascading for parameterized workflows.

The core insight: ergonomics and operational safety are not at odds with persistence and auditability. Sandcastle proved the UX patterns; Minion's session-as-log architecture provides the durable substrate. Combining both produces an orchestrator where autonomous agents run in complete isolation, always clean up after themselves, and never lose state.

## Project Classification

- **Project Type:** Developer tool — Rust CLI + library for AI agent orchestration
- **Domain:** General (fintech/healthcare compliance inherited from consuming organizations)
- **Complexity:** High — container lifecycle management, process isolation, signal safety, multi-provider abstraction, concurrent agent execution, crash recovery
- **Project Context:** Brownfield — extending Engine v2 crate architecture (4 crates: `minion-core`, `minion-session`, `minion-sandbox-orchestrator`, `minion-harness`; Epics 1-2 complete)

## Success Criteria

### User Success

- **Zero-hang guarantee:** No workflow step blocks the engine indefinitely. A stuck agent or command is killed after a configurable step timeout (default 10 min), the step emits `StepFailed` with reason "step timeout", and the session remains resumable.
- **Crash-safe cleanup:** If the `minion` process is killed (SIGTERM, SIGINT, OOM), all sandbox containers owned by active sessions are destroyed within 5 seconds. Zero orphaned containers accumulate.
- **Parameterized workflows:** Users define environment variables in workflow YAML or `.minion/defaults.yaml` and they reach the sandbox container — no hardcoding `export FOO=bar` in step commands.
- **Parallel agent isolation:** Two agents triggered on the same repo run on separate git worktrees with independent branches, never interfering with each other's working directory.

### Business Success

- **Production stability:** Zero orphaned Docker containers after 30 days of continuous VPS operation with daily workflow dispatch.
- **Parallel capacity:** Ability to dispatch N concurrent code review workflows on the same repository, each producing isolated commits that auto-merge to the target branch.
- **Operational confidence:** Engineering team trusts the engine to run unattended overnight — timeout + cleanup + session durability eliminate the "check on it manually" pattern.

### Technical Success

- All features implemented within the existing v2 crate architecture (`minion-core`, `minion-session`, `minion-sandbox-orchestrator`, `minion-harness`) — no new crates required.
- Cancel path fix: `finalise_cancel()` destroys the correct container by session ID (not `SandboxId::default()`).
- Zero regression: all existing 16 tests (8 unit + 8 integration) continue to pass.
- New features covered by >=70% unit test coverage per modified crate.
- `cargo clippy -- -D warnings` clean across entire workspace.

### Measurable Outcomes

| Metric | Target | How Measured |
|--------|--------|-------------|
| Stuck agent detection | < 10 min (configurable) | Step timeout fires, `StepFailed` event emitted |
| Container cleanup on crash | < 5s | Signal handler + `destroy_by_session()` |
| Env var passthrough | 100% of declared vars | Integration test: step reads `$TEST_VAR` in container |
| Parallel agent isolation | 0 git conflicts from concurrent runs | Stress test: 5 agents x same repo x separate worktrees |
| Cancel correctness | Right container destroyed | Unit test: verify session UUID passed to `destroy()` |

## Product Scope

### MVP — Minimum Viable Product

1. **Step timeout** — wrap `StepExecutor::execute()` in `tokio::time::timeout()` with configurable duration per step/workflow
2. **Cancel cleanup fix** — pass actual session ID to `lifecycle.destroy()` in `finalise_cancel()`
3. **Env dict on exec** — extend `SandboxLifecycle::exec()` to accept `HashMap<String, String>`, thread through Docker `--env` flags
4. **Process signal handler** — register SIGTERM/SIGINT handler in CLI that walks active sessions and destroys their containers

### Growth Features (Post-MVP)

5. **Branch strategies** — `BranchStrategy` enum (Head, MergeToHead, NamedBranch) in `minion-core`, used by harness to manage git state pre/post workflow
6. **WorkspaceManager** — trait for git worktree create/prune/merge, enabling parallel agents on the same repo
7. **Prompt templating** — `{{KEY}}` placeholder substitution and `` !`cmd` `` shell expansion in workflow step definitions
8. **Completion signal** — agent emits a configurable string (`<COMPLETE>`) to exit iteration loop early
9. **Idle timeout** — output-based idle detection (reset timer on stdout activity), complementing the wall-clock step timeout from MVP

### Vision (Future)

10. **Multi-provider** — Podman provider (copy of Docker with SELinux/rootless adjustments), cloud provider trait (Vercel, Daytona-style)
11. **Interactive exec** — TTY/PTY forwarding for debugging sessions inside sandbox containers
12. **Provider config object** — rich config passed to `create()` (volume mounts, resource limits, network policies, user mapping)

## User Journeys

### Journey 1: Bruno (Workflow Author) — "Fire and forget parallel reviews"

**Opening Scene**: Bruno has 5 PRs queued for code review on the `payment-gateway` repo. Today he dispatches them one at a time via Slack bot, waiting for each to finish before starting the next. It takes 25 minutes. He wants to fire all 5 at once.

**Rising Action**: Bruno triggers `minion remote exec code-review --repo allanbrunobr/payment-gateway --branch pr/101 -- pr/102 pr/103 pr/104 pr/105`. The engine spawns 5 sessions, each on its own git worktree (`sandcastle/code-review/20260415-143022-1` through `-5`). Each session gets its own sandbox container. Environment variables (`GITHUB_TOKEN`, `ANTHROPIC_API_KEY`) are injected from `defaults.yaml` — no hardcoding.

**Climax**: PR #103's review agent gets stuck in an infinite loop analyzing a 2000-line generated file. After 10 minutes of no output, the step timeout fires. The step emits `StepFailed { error: "step timeout: 600s elapsed" }`, the container is destroyed, and the session is marked failed. The other 4 reviews complete normally. Each auto-merges its review comments to the target branch via `merge-to-head` strategy.

**Resolution**: Bruno checks `minion remote status` and sees 4 successes, 1 timeout. He adjusts the workflow for PR #103 (adds `--exclude "generated/**"`) and re-dispatches just that one. Total time: 6 minutes instead of 25. No orphaned containers, no manual cleanup.

### Journey 2: VPS Process Crash — "The OOM killer strikes"

**Opening Scene**: It's 3 AM. The VPS is running a heavy security audit workflow that spawns a Claude Code agent inside a Docker container. The agent consumes too much memory, and the Linux OOM killer terminates the `minion` process.

**Rising Action**: The SIGTERM signal arrives. The process-level signal handler activates. It walks the active session registry, finds one running session (`session-7a3f...`), and calls `lifecycle.destroy_by_session(7a3f...)`. The Docker container `minion-session-7a3f...` is force-removed. The session status in PostgreSQL is set to `cancelled`.

**Climax**: At 9 AM, Bruno checks `minion remote status` and sees the cancelled session with reason "process killed (SIGTERM)". He runs `minion remote exec security-audit --resume session-7a3f...`. The engine loads the session log, replays events, discovers that 3 of 5 steps completed successfully, and resumes from step 4. A fresh container is created. The audit completes.

**Resolution**: Zero orphaned containers. Zero lost progress. The append-only session log preserved exactly where the workflow stopped. Bruno didn't need to SSH into the VPS or manually clean up anything.

### Journey 3: DevOps Engineer — "Audit the fleet"

**Opening Scene**: The team's compliance review is next week. The DevOps engineer needs to verify that the Minion Engine deployment is clean: no orphaned containers, no stuck sessions, no resource leaks.

**Rising Action**: He SSHs into the VPS and runs `docker ps --filter name=minion-session-`. Zero containers — the signal handler and step timeout have been doing their job. He checks `minion session list --status=running` — zero stale sessions. He checks `minion session list --status=failed --since 30d` — 3 failures, all with clear error messages (2 timeouts, 1 API rate limit).

**Climax**: He runs `docker system df` and sees Docker disk usage is stable — worktrees are pruned after workflow completion, containers don't accumulate. The PostgreSQL session_events table has clean audit trails: every step has a start event, a completion or failure event, and a duration.

**Resolution**: Compliance report written in 15 minutes: "AI agent execution is fully auditable. Sessions survive crashes. Containers are deterministically cleaned up. No resource leaks observed in 30 days of continuous operation."

### Journey 4: Workflow Developer — "Parameterize once, reuse everywhere"

**Opening Scene**: A developer wants to create a reusable `code-review` workflow that works across all repos without editing the YAML per-project. Today, environment variables are hardcoded in step commands.

**Rising Action**: He edits `code-review.yaml` and replaces hardcoded values with placeholders: `command: "gh pr review {{PR_NUMBER}} --repo {{REPO}}"`. In `.minion/defaults.yaml`, he adds `env: { GITHUB_TOKEN: "${GITHUB_TOKEN}" }` for cascading resolution. When dispatched, the engine resolves `{{PR_NUMBER}}` from the CLI args and `GITHUB_TOKEN` from the host environment, passing both to the sandbox via `docker exec --env`.

**Climax**: The same workflow YAML runs against `payment-gateway`, `user-service`, and `billing-api` — zero edits. Each dispatch gets the right env vars, the right PR number, and the right repo. The sandbox container receives exactly the declared variables, nothing more (no env leakage).

**Resolution**: One YAML, three repos, zero duplication. The developer shares the workflow with the team via git. Everyone uses it as-is — only the CLI args change.

### Journey Requirements Summary

| Journey | Capabilities Revealed | Key FRs |
|---------|----------------------|---------|
| Bruno (parallel reviews) | Step timeout, branch strategies (merge-to-head), env dict injection, WorkspaceManager (parallel worktrees), concurrent session execution | FR1, FR3, FR9-FR11, FR13-FR15 |
| VPS crash (OOM) | Process signal handler, session resume from log, container cleanup by session ID, cancel path correctness | FR3, FR5-FR8 |
| DevOps (audit) | Session list/status CLI, clean container lifecycle, worktree pruning, audit trail via session events | FR17, FR22-FR24 |
| Workflow dev (parameterize) | Env var cascading (YAML > defaults > host), prompt templating (`{{KEY}}`), sandbox env isolation | FR9-FR12, FR19, FR21 |

## Domain-Specific Requirements

> **Scope:** These requirements are specific to the domain of container-based AI agent orchestration — not inherited regulatory requirements (PCI-DSS, LGPD apply to the consuming organizations, not to the engine itself).

### Technical Constraints

- **Rust async safety:** All types crossing task boundaries must be `Send + Sync`. No `Mutex<T>` held across `.await` points (prior bug: `Mutex<Option<Instant>>` made `Engine` futures `!Send`). Use `&mut self` exclusivity or `AtomicBool` for shared state.
- **Container runtime dependency:** Engine assumes `docker` CLI is on `$PATH`. No embedded Docker client (bollard) — subprocess via `tokio::process::Command` is the interface. Error messages are string-parsed, not typed.
- **Session-log-as-truth:** The engine holds zero in-memory state between steps. All progress is reconstructed from `Session::replay()`. Any new feature (timeout, branch strategy, env dict) must be expressible as events in the session log — otherwise resume-after-crash cannot reconstruct state.
- **Single-binary constraint:** Features land in existing crates. No new binaries (ADR-011 reserves `minion-mcp-proxy` for MCP credential isolation). No new crate unless justified.

### Integration Requirements

- **Docker CLI:** `docker run`, `docker exec`, `docker rm -f` are the only required Docker commands. New features (env passing) must work with these primitives only.
- **PostgreSQL:** Session events table (`session_events`) is the only persistent store. New event types (e.g., `IdleTimeoutFired`, `SignalReceived`) must be backward-compatible variants of the `Event` enum (`#[non_exhaustive]`, `#[serde(other)]`).
- **Existing CLI:** New flags (e.g., `--timeout`, `--branch-strategy`) must coexist with the existing CLI surface without breaking `minion execute` / `minion remote exec`.

### Risk Register

This consolidated register covers technical, innovation, and implementation risks across all phases.

| # | Risk | Phase | Impact | Mitigation |
|---|------|-------|--------|------------|
| R1 | Step timeout kills a step that was making slow progress (false positive) | MVP | Workflow failure | Default 10 min is generous. Growth phase adds output-based idle detection (FR2) as a more precise alternative. Users configure both. |
| R2 | Signal handler races with active step execution | MVP | Partial cleanup, corrupted session state | Signal handler sets `CancelToken` (existing mechanism), waits for current step to drain (up to 5s), then force-destroys. Session log records cancellation event before process exit. |
| R3 | Env var leakage — sensitive vars reaching wrong container | MVP | Security boundary violation | Explicit opt-in only: only vars declared in workflow YAML or `defaults.yaml` are forwarded. No `--env-host` passthrough of entire host environment. (FR12) |
| R4 | Worktree accumulation from crashed sessions | Growth | Disk exhaustion on VPS | `WorkspaceManager::prune_stale()` runs on engine startup — two-phase cleanup (git worktree prune + orphan directory removal). Mirrors Sandcastle's proven pattern. (FR17) |
| R5 | Branch merge conflict during auto-merge | Growth | Agent work lost in unmerged branch | Preserve temporary branch, emit `MergeConflict` event with branch name. User resolves manually. No auto-retry. (FR16) |
| R6 | Output-based idle detection too noisy (agents that think silently for minutes) | Growth | False timeout kills | Fall back to wall-clock step timeout as secondary. User configures both: idle timeout + max step duration. |
| R7 | Session event log grows too large for long-running agents | Growth | Slow replay on resume | Pagination on `Session::replay()`. Only load events after last completed step for resume path. |
| R8 | `tokio::signal::ctrl_c()` + Unix signal handling behaves unexpectedly with Docker subprocesses | MVP | Cleanup fails silently | Integration test: `kill -TERM` during `docker exec`, verify container is destroyed. |

## Innovation & Novel Patterns

The features in this PRD are not individually novel — Sandcastle, OpenHands, and Codex CLI each implement subsets. The innovation lies in combining them with Minion's unique session durability.

### Detected Innovation Areas

1. **Persistent-session agent orchestration**: Existing tools (Sandcastle, OpenHands, Codex CLI) are ephemeral — run once, get results, state is gone. Minion combines Sandcastle's ergonomic patterns with an append-only session log that survives crashes, enabling `resume` as a first-class operation. No current open-source agent orchestrator has this.

2. **Output-based idle detection with session durability**: Sandcastle's idle timeout is process-local (lost on crash). Minion can emit `IdleTimeoutFired` as a session event — meaning the timeout decision is auditable and the exact moment of kill is recorded for post-mortem. This turns a safety mechanism into a diagnostic tool.

3. **Branch strategies as session-log events**: Sandcastle's branch strategies are runtime-only (merge-to-head happens at process exit). Minion can record `BranchCreated`, `MergeAttempted`, `MergeConflict` as session events — making the git lifecycle of parallel agents fully traceable and resumable.

### Competitive Landscape

| Tool | Ephemeral | Session Persistence | Resume | Branch Strategies | Step Timeout |
|------|-----------|-------------------|--------|-------------------|--------------|
| Sandcastle | Yes | No | No | Yes (3 types) | Yes (output-based) |
| OpenHands | Yes | No | No | No | No |
| Codex CLI | Yes | No | No | No | No |
| Claude Code | Yes | Partial (conversation) | No | No | No |
| **Minion Engine (post-PRD)** | **No** | **Yes (PostgreSQL)** | **Yes** | **Yes (3 types)** | **Yes (event-logged)** |

### Validation Approach

- **Step timeout**: Stress test with intentionally stuck agent (`sleep infinity`). Verify: timeout fires at configured interval, `StepFailed` event emitted, session resumable after timeout.
- **Crash recovery**: `kill -9` the minion process mid-step. Verify: signal handler cleans up containers, session status is `cancelled`, `resume` picks up from last completed step.
- **Branch strategy**: 5 parallel agents on same repo. Verify: separate worktrees, independent branches, merge-to-head produces clean history, merge conflict preserves temp branch.

## Technical Architecture

> This section covers architecture considerations specific to Minion Engine as a developer tool — Rust toolchain, distribution, and API surface changes.

### Rust Toolchain Requirements

- Minimum Rust edition: 2021
- Required features: `tokio` (async runtime), `sqlx` (PostgreSQL), `uuid` (v4 + v5), `serde` (serialization), `thiserror` (error types)
- New dependencies: `tokio::signal` (SIGTERM/SIGINT), `tokio::time` (timeout). No new external crates for env dict or branch strategies (stdlib + git CLI).
- Compilation: `cargo build --release --features slack` on VPS. Zero new feature flags.

### Distribution

- Primary: `cargo install --path .` on VPS, then `cp target/release/minion /usr/local/bin/minion` (dispatch uses this path)
- The engine creates Docker containers — it does not run inside one

### API Surface Changes

| Surface | Current | After Sandcastle Features |
|---------|---------|--------------------------|
| `SandboxLifecycle::exec()` | `(id, cmd) -> ExecOutput` | `(id, cmd, env: HashMap) -> ExecOutput` |
| `SandboxLifecycle::create()` | `(session_id) -> Sandbox` | No change (config lives in `DockerLifecycleConfig`) |
| `Engine::step()` | No timeout | `tokio::time::timeout(duration, ...)` wrapping exec |
| `Engine::finalise_cancel()` | `SandboxId::default()` | `self.session.id()` passed to `destroy_by_session()` |
| CLI: `minion execute` | No `--timeout` flag | `--timeout <seconds>` flag |
| CLI: `minion execute` | No `--branch-strategy` | `--branch-strategy head\|merge-to-head\|branch:<name>` (Growth) |
| `Event` enum | 7 variants | +3: `IdleTimeoutFired`, `SignalReceived`, `BranchCreated` (Growth) |
| New trait | — | `WorkspaceManager` (Growth) |

### Workflow YAML Schema Changes

```yaml
# Current (v2)
steps:
  - name: review
    command: "claude -p 'review this PR'"

# After MVP (env dict + step timeout)
steps:
  - name: review
    command: "claude -p 'review this PR'"
    timeout: 600  # seconds, wall-clock step timeout (new)
    env:          # new
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
      PR_NUMBER: "{{PR_NUMBER}}"

# After Growth (templating + branch strategy)
branch_strategy: merge-to-head  # new
steps:
  - name: review
    command: "gh pr review {{PR_NUMBER}} --repo {{REPO}}"
    timeout: 600
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
```

### Implementation Considerations

- **Backward compatibility:** Existing workflow YAML without `timeout`, `env`, or `branch_strategy` fields continues to work (defaults: no timeout, empty env, head strategy). Serde `#[serde(default)]` handles this.
- **Testing strategy:** Each new feature gets unit tests in its crate + one integration test in `tests/`. MockLifecycle already supports call recording — extend to verify env dict and timeout behavior.
- **Migration path:** No migration tool needed. New fields are additive. Existing sessions in PostgreSQL are unaffected (new event types are forward-compatible via `#[non_exhaustive]`).

## Project Scoping & Phased Development

### MVP Strategy

**Approach:** Problem-solving MVP — ship the 4 features (Product Scope #1-#4) that fix real production pain. No new abstractions, no new traits, no new crates. Minimal surface area change to existing APIs.

**Resource Requirements:** 1 developer (Bruno) with Claude Code agent assistance. All changes are in existing Rust crates.

**Core User Journeys Supported:** Journey 2 (VPS crash) — signal handler + cancel fix. Journey 4 (parameterize) — env dict (partial: env passing only, no `{{KEY}}` templating yet).

| Scope # | Feature | Crate | Lines Changed (est.) | Rationale |
|---------|---------|-------|---------------------|-----------|
| 1 | Step timeout | minion-harness | ~20 | Without this, a single stuck agent blocks the VPS indefinitely. |
| 2 | Cancel cleanup fix | minion-harness | ~3 | **Bug fix.** `SandboxId::default()` -> actual session ID. Blocks everything else. |
| 3 | Env dict on exec | minion-sandbox-orchestrator | ~30 | Without this, every workflow hardcodes secrets in command strings. |
| 4 | Process signal handler | minion-harness + CLI | ~50 | Without this, `kill -9` leaves orphaned containers forever. |

**Explicitly NOT in MVP:** Branch strategies, prompt templating, new Event variants, CLI `--branch-strategy` flag. Step timeout set via workflow YAML `timeout:` field, not CLI flag.

### Post-MVP Phases

**Phase 2 — Growth** (enables parallel agents): Product Scope #5-#9.

| Scope # | Feature | Dependency | Effort |
|---------|---------|------------|--------|
| 5 | `BranchStrategy` enum in `minion-core` | None | Low |
| 6 | `WorkspaceManager` trait + git impl | #5 | Medium |
| 7 | Prompt `{{KEY}}` templating | None | Low |
| 8 | Completion signal detection | None | Low |
| 9 | Idle timeout (output-based) | #1 (step timeout) | Medium |

**Phase 3 — Expansion** (platform extensibility): Product Scope #10-#12 plus additional enhancements.

| Scope # | Feature | Dependency | Effort |
|---------|---------|------------|--------|
| 10 | Podman provider | None (copy Docker) | Low |
| 11 | Interactive exec (TTY) | #12 | High |
| 12 | Provider config object | Trait redesign | Medium |

### Incremental Delivery

Features 1-2 (step timeout + cancel fix) can ship independently as a micro-release. Features 3-4 (env dict + signal handler) are a second micro-release. No all-or-nothing dependency. **Absolute minimum:** Ship just Features 1+2 (~23 lines). This alone eliminates the two worst production risks.

## Functional Requirements

> **Scope tags:** FR1-FR12 = MVP, FR13-FR24 = Growth, FR25-FR27 = Expansion.

### Step Execution Safety

- **FR1:** The engine can enforce a configurable step timeout (wall-clock) per workflow step, terminating execution and emitting a failure event when the timeout elapses.
- **FR2:** The engine can detect idle steps (no stdout output for a configurable duration) and terminate them independently of the wall-clock step timeout.
- **FR3:** The engine can cancel a running step and destroy the correct sandbox container associated with the active session (not a default/empty container ID).
- **FR4:** The engine can report the reason for step termination (step timeout, idle timeout, cancellation, error) in the session event log.

### Process Lifecycle

- **FR5:** The engine can intercept OS termination signals (SIGTERM, SIGINT) and initiate graceful shutdown of all active sessions before process exit.
- **FR6:** The engine can maintain a registry of active sessions during runtime so that the signal handler knows which containers to destroy.
- **FR7:** The engine can destroy all sandbox containers owned by active sessions during graceful shutdown, tolerating already-destroyed containers.
- **FR8:** The engine can record a `SignalReceived` event in each active session's log before process exit, preserving the reason for cancellation.

### Sandbox Environment

- **FR9:** Workflow authors can declare environment variables per step in workflow YAML, and the engine passes them to the sandbox container at execution time.
- **FR10:** Workflow authors can declare default environment variables in `.minion/defaults.yaml` that apply to all steps unless overridden at the step level.
- **FR11:** The engine can resolve environment variable values from the host process environment using `${VAR}` syntax in workflow YAML.
- **FR12:** The engine can restrict environment variables passed to the sandbox to only those explicitly declared in the workflow or defaults (no full host env passthrough).

### Git Workspace Management

- **FR13:** The engine can create isolated git worktrees for each workflow session, enabling multiple agents to operate on the same repository concurrently.
- **FR14:** The engine can apply a configurable branch strategy (head, merge-to-head, named-branch) to determine how agent commits land in the repository.
- **FR15:** The engine can auto-merge a temporary branch back to the target branch when the workflow completes successfully (merge-to-head strategy).
- **FR16:** The engine can preserve a temporary branch and emit a conflict event when auto-merge fails, allowing manual resolution.
- **FR17:** The engine can prune stale worktrees on startup (two-phase: git metadata cleanup, then orphan directory removal).
- **FR18:** The engine can detect uncommitted changes in a worktree and preserve it for inspection instead of deleting it during cleanup.

### Workflow Configuration

- **FR19:** Workflow authors can use `{{KEY}}` placeholder syntax in step commands, resolved from CLI arguments or workflow-level variables at dispatch time.
- **FR20:** Workflow authors can define a completion signal string that, when detected in agent stdout, terminates the iteration loop early.
- **FR21:** The engine can validate that all referenced `{{KEY}}` placeholders have corresponding values before executing a step, failing fast with a clear error if any are missing.

### Session Observability

- **FR22:** The engine can emit new event types (`IdleTimeoutFired`, `SignalReceived`, `BranchCreated`, `MergeAttempted`, `MergeConflict`) as backward-compatible additions to the session event log.
- **FR23:** The engine can record branch strategy decisions and git operations as session events, making the full git lifecycle of parallel agents auditable via `Session::replay()`.
- **FR24:** Operators can list sessions by status (running, completed, failed, cancelled) and time range via CLI command.

### Provider Extensibility

- **FR25:** The engine can support multiple sandbox providers (Docker, Podman, local shell) through the existing `SandboxLifecycle` trait without requiring a new abstraction layer.
- **FR26:** Sandbox providers can accept a configuration object at creation time specifying resource limits, volume mounts, and network policies.
- **FR27:** The engine can execute interactive sessions with TTY forwarding through sandbox providers that support it.

## Non-Functional Requirements

### Performance

| Metric | Target | Context |
|--------|--------|---------|
| Signal handler response | Container cleanup initiated within 1s of SIGTERM | FR5-FR7: graceful shutdown must beat the kernel's 30s SIGKILL deadline |
| Step timeout precision | Fire within 1s of configured threshold | FR1: timer resolution must be tight enough to be useful |
| Step timeout overhead | < 50ms added latency per step from timeout wrapper | FR1: `tokio::time::timeout()` should be negligible |
| Env var resolution | < 10ms for resolving `${VAR}` references | FR11: host env lookup is O(1), should not bottleneck step startup |
| Worktree creation | < 5s per worktree (including git checkout) | FR13: acceptable for workflow startup; not in hot path |
| Worktree pruning | < 30s for up to 50 stale worktrees | FR17: runs at engine startup, not latency-sensitive |

### Security

- **Env var isolation:** Only variables explicitly declared in workflow YAML or `defaults.yaml` are forwarded to sandbox containers. The engine never passes its full `process.env` to Docker. (FR12)
- **No credential in logs:** Environment variable values are never written to session events. Event payloads record variable *names* only (e.g., `env_keys: ["GITHUB_TOKEN", "API_KEY"]`), not values.
- **Sandbox boundary preserved:** All new features operate through the existing `docker exec` interface. No feature introduces direct filesystem access between host and container outside of Docker volume mounts.
- **Signal handler safety:** The signal handler does not access PostgreSQL (connection may be dead). It only sets `CancelToken` and calls `docker rm -f` via subprocess. Session event recording is best-effort.

### Reliability

- **Crash recovery:** After any process termination (SIGTERM, SIGINT, OOM, panic), the engine can be restarted and `Session::replay()` reconstructs correct state for all sessions. No manual database intervention required. (FR3, FR8)
- **Idempotent cleanup:** `destroy_by_session()` tolerates already-destroyed containers without error. Signal handler, cancel path, and normal completion can all call destroy without coordination. (FR7)
- **Timeout determinism:** Step timeout and idle timeout always produce a `StepFailed` event with a specific reason string. No silent failures — every termination is recorded. (FR4)
- **Worktree safety:** Worktrees with uncommitted changes are never deleted automatically. The engine preserves them and emits an event for manual inspection. (FR18)

### Integration

- **Docker CLI compatibility:** All new features use `docker run`, `docker exec --env`, and `docker rm -f` only. No Docker API or bollard dependency. Compatible with Docker CE 20.10+ and Docker Desktop.
- **PostgreSQL compatibility:** New event types use the existing `session_events` table schema (JSONB payload). No schema migrations required for MVP features. New event variants are `#[non_exhaustive]` and `#[serde(other)]`-safe. (FR22)
- **Git CLI compatibility:** WorkspaceManager uses `git worktree add/remove/list/prune` commands. Compatible with git 2.30+. No libgit2 dependency.
- **Existing workflow YAML:** All new YAML fields (`timeout`, `env`, `branch_strategy`) use `#[serde(default)]` — existing workflows without these fields continue to work unchanged.

### Maintainability

- **Zero warnings:** `cargo clippy -- -D warnings` clean across entire workspace after all changes.
- **Test coverage:** >= 70% unit coverage per modified crate. Each FR has at least one test that verifies its acceptance criteria.
- **Error types:** All new public errors use `thiserror` derives. No `anyhow` in library crates.
- **Backward compatibility:** No breaking changes to public trait signatures in MVP. `SandboxLifecycle::exec()` gains an optional `env` parameter via a new method with default implementation, not by changing the existing signature.
