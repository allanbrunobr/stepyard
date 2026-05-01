# Features

<!-- Generated from BMAD artifacts by /hive:md-from-bmad -->
<!-- Source: _bmad-output/sandcastle-features/epics.md -->
<!-- Date: 2026-04-17 -->

## Feature 1: Fix Cancel Cleanup to Destroy Correct Container
- Description: [Epic 1: Stuck-Agent Termination & Cancel Correctness, Story 1.1] As a platform operator, I want finalise_cancel() to pass the active session's UUID to lifecycle.destroy() (not SandboxId::default()), so that cancels free resources immediately and never orphan containers. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: none
- Status: done

## Feature 2: Introduce TerminationReason Sub-Enum and StepFailed Error
- Description: [Epic 1: Stuck-Agent Termination & Cancel Correctness, Story 1.2] As an engine maintainer, I want a single EngineError::StepFailed { step_index, reason: TerminationReason } variant, so that every step-termination path reports its cause through one taxonomy without sibling variants proliferating. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 1
- Status: done

## Feature 3: Add StepTimeoutFired Event and Workspace non_exhaustive_omitted_patterns Lint
- Description: [Epic 1: Stuck-Agent Termination & Cancel Correctness, Story 1.3] As an engine maintainer, I want the first StepTimeoutFired variant and the workspace-wide non_exhaustive_omitted_patterns = deny lint, so that future event variants ship without silent breakage in subscribers and display code. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 2
- Status: done

## Feature 4: Enforce Step Timeout via tokio::time::timeout Wrapper
- Description: [Epic 1: Stuck-Agent Termination & Cancel Correctness, Story 1.4] As a workflow author, I want the engine to enforce the step timeout YAML field via tokio::time::timeout, so that a stuck agent never runs past its wall-clock deadline and the session log records exactly why it stopped. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 3
- Status: done

## Feature 5: Thread Cancel Broadcast Channel Through Engine Construction
- Description: [Epic 2: Crash-Safe Process Lifecycle & Session Visibility, Story 2.1] As an engine maintainer, I want a per-process broadcast::Sender<()> in main() and a broadcast::Receiver<()> on every Engine subscribed via HarnessConfig::shutdown_tx, so that later stories wire signal handlers and crash-recovery without introducing a runtime registry. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 4
- Status: done

## Feature 6: Install SIGINT/SIGTERM Handlers and Graceful Shutdown Deadline
- Description: [Epic 2: Crash-Safe Process Lifecycle & Session Visibility, Story 2.2] As a platform operator, I want the stepyard binary to intercept SIGINT/SIGTERM, fire the broadcast channel, wait up to shutdown_grace_s for in-flight engines, then exit with the canonical signal exit code, so that container cleanup starts within 1s and never hits the kernel's 30s SIGKILL deadline. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 5
- Status: done

## Feature 7: Emit SignalReceived Event and Destroy Container on Broadcast
- Description: [Epic 2: Crash-Safe Process Lifecycle & Session Visibility, Story 2.3] As an engine maintainer, I want each Engine to select! on the broadcast receiver, synchronously emit Event::SignalReceived, then idempotently destroy its sandbox container, so that SIGTERM/SIGINT cancellation produces an auditable session record before the process exits. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 6
- Status: done

## Feature 8: Startup Crash Recovery — Reconcile Orphan Sessions and Containers
- Description: [Epic 2: Crash-Safe Process Lifecycle & Session Visibility, Story 2.4] As a platform operator, I want stepyard to run a three-phase reconcile at startup that marks orphan running sessions as failed, destroys orphan containers, and stubs the worktree pruning slot, so that a restart after OOM/crash/hard-kill leaves the engine consistent without manual intervention. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 7
- Status: done

## Feature 9: Add stepyard session list --status CLI Subcommand
- Description: [Epic 2: Crash-Safe Process Lifecycle & Session Visibility, Story 2.5] As a DevOps engineer, I want stepyard session list --status <running|completed|failed|cancelled> [--since <duration>] backed by a PostgreSQL query on sessions.status, so that I can audit session outcomes and filter by time range without loading full event logs. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 8
- Status: done

## Feature 10: Extend SandboxLifecycle Trait with exec_with_env Default-Impl Method
- Description: [Epic 3: Sandbox Environment Injection, Story 3.1] As an engine maintainer, I want SandboxLifecycle to gain exec_with_env(id, cmd, env) as a default-impl method delegating to exec(id, cmd) (ignoring env), so that Epic 3 can inject env vars via the new method without changing the existing exec signature. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 4
- Status: done

## Feature 11: Implement DockerLifecycle::exec_with_env with Argv-Only --env Flags
- Description: [Epic 3: Sandbox Environment Injection, Story 3.2] As an engine maintainer, I want DockerLifecycle to override exec_with_env with docker exec --env K=V argv-only invocations (one --env per pair, sorted), so that env vars pass as argv elements and are never shell-interpolated (argv-not-shell rule). Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 10
- Status: done

## Feature 12: Extend Workflow YAML Schema with env: Fields and .stepyard/defaults.yaml Loader
- Description: [Epic 3: Sandbox Environment Injection, Story 3.3] As a workflow author, I want step-level env: {KEY: VAL} and workflow-level env: {KEY: VAL} in YAML plus a .stepyard/defaults.yaml file contributing default env pairs, so that I can parameterize secrets and config per step, per workflow, or project-wide. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 11
- Status: done

## Feature 13: Cascade Resolver in Engine::prepare_step with ${VAR} Host Expansion
- Description: [Epic 3: Sandbox Environment Injection, Story 3.4] As an engine runtime, I want Engine::prepare_step to resolve the effective env by overlaying step > workflow > defaults.yaml and expanding ${VAR} against host env, so that one workflow YAML declares opt-in env with clear precedence and secrets flow through without full host passthrough. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 12
- Status: done

## Feature 14: Negative-Control Security Tests in tests/injection_negative.rs
- Description: [Epic 3: Sandbox Environment Injection, Story 3.5] As a security reviewer, I want a dedicated negative-control test file proving user env values reach the container as argv (never executed as shell) and that sh -c IS user-owned, so that any future regression reintroducing shell interpolation is caught at CI time. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 13
- Status: done

## Feature 15: Define WorkspaceManager Trait and GitWorktreeManager Skeleton
- Description: [Epic 4: Parallel Agent Isolation via Git Workspaces, Story 4.1] As an engine maintainer, I want a WorkspaceManager trait in stepyard-sandbox-orchestrator/src/workspace.rs and a GitWorktreeManager struct with stub method bodies, so that subsequent stories fill in prepare/finalize/prune against a stable trait contract without a new crate. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 9
- Status: done

## Feature 16: Implement GitWorktreeManager::prepare with WorkspacePrepared + BranchCreated Events
- Description: [Epic 4: Parallel Agent Isolation via Git Workspaces, Story 4.2] As a workflow runtime, I want Engine to emit WorkspacePrepared (and BranchCreated when a branch is created) synchronously before invoking GitWorktreeManager::prepare which runs git worktree add via argv-only subprocess, so that every workspace decision is logged before git IO and parallel agents get isolated working trees. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 15
- Status: done

## Feature 17: Workflow branch_strategy: YAML Schema and CLI Override
- Description: [Epic 4: Parallel Agent Isolation via Git Workspaces, Story 4.3] As a workflow author, I want a top-level branch_strategy: YAML field (head|merge_to_head|named_branch) with required sibling branch_name: when named_branch plus a --branch-strategy CLI override, so that I can declare how agent commits land in the repo without hardcoding per-workflow branching logic. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 16
- Status: done

## Feature 18: Auto-Merge on MergeToHead and Conflict Preservation with MergeAttempted/MergeConflict Events
- Description: [Epic 4: Parallel Agent Isolation via Git Workspaces, Story 4.4] As a workflow author, I want successful merge_to_head sessions to auto-merge the temp branch with MergeAttempted emitted first, and conflicts to preserve the temp branch + emit MergeConflict with affected files, so that parallel agents converge automatically on clean work and conflicts surface without losing the branch. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 17
- Status: done

## Feature 19: D8 Two-Phase Startup Prune with WorkspacePruned Event and Uncommitted-Changes Preservation
- Description: [Epic 4: Parallel Agent Isolation via Git Workspaces, Story 4.5] As a platform operator, I want the workspace pruning slot in startup phase 3 to execute the D8 two-phase protocol (git worktree prune then filesystem walk) skipping worktrees with uncommitted changes and emitting WorkspacePruned per removed dir, so that stale worktrees reclaim without risking loss of uncommitted work. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 18
- Status: done

## Feature 20: Add IdleTimeoutFired Event + ExecOptions Type + exec_with_options Default-Impl
- Description: [Epic 5: Workflow Templating & Idle Detection, Story 5.1] As an engine maintainer, I want IdleTimeoutFired added to the event enum, an ExecOptions {env, idle_timeout} struct, and an exec_with_options default-impl on SandboxLifecycle delegating to exec_with_env, so that Story 5.2 can wire real streaming idle detection without breaking the existing exec_with_env signature. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 14
- Status: done

## Feature 21: Implement DockerLifecycle::exec_with_options Streaming and Engine Wiring for IdleTimeoutFired
- Description: [Epic 5: Workflow Templating & Idle Detection, Story 5.2] As a workflow author, I want the engine to detect when an agent stops producing stdout for idle_timeout ms, synchronously emit IdleTimeoutFired, then destroy the container and return StepFailed { reason: IdleTimeout }, so that idle agents terminate deterministically without indefinite resource consumption. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 20
- Status: done

## Feature 22: {{KEY}} Template Substitution Preprocessor with YAML-Safe Output
- Description: [Epic 5: Workflow Templating & Idle Detection, Story 5.3] As a workflow author, I want a stepyard_core::template::substitute(text, &vars) pre-parse pass that replaces every {{KEY}} with the YAML-encoded value via serde_yaml::to_string running BEFORE serde_yaml::from_str, so that one workflow YAML can be parameterized across N projects without YAML structure injection from raw value substitution. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 21
- Status: done

## Feature 23: CLI --var KEY=VAL Flag + Defaults Source + EngineError::PlaceholderUnresolved Validation
- Description: [Epic 5: Workflow Templating & Idle Detection, Story 5.4] As a workflow author, I want stepyard run --var KEY=VAL as a multi-value CLI flag and .stepyard/defaults.yaml providing value sources for {{KEY}}, with EngineError::PlaceholderUnresolved failing fast at parse time when any placeholder is missing, so that I can run one workflow across N projects with explicit per-run parameters and clear errors on missing keys. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 22
- Status: pending

## Feature 24: Completion-Signal String Detection on Agent Stdout
- Description: [Epic 5: Workflow Templating & Idle Detection, Story 5.5] As a workflow author, I want a per-workflow completion_signal: "<string>" YAML field that terminates the iteration loop early on substring match in agent stdout with a new CompletionSignaled event, so that agents (e.g., LLM loops) can self-signal task completion without relying solely on subprocess exit codes. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 23
- Status: pending

## Feature 25: Implement PodmanLifecycle to Validate Multi-Provider Support via Existing Trait
- Description: [Epic 6: Multi-Provider & Interactive Sandboxes, Story 6.1] As a platform operator, I want a PodmanLifecycle struct implementing the existing SandboxLifecycle trait (without changing the trait) shippable as an alternate provider via --sandbox-provider <docker|podman> CLI flag, so that Docker-restricted hosts can run stepyard via Podman and we prove the trait supports multiple providers without abstraction-layer redesign. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 24
- Status: pending

## Feature 26: Add CreateOptions Struct and create_with_options Default-Impl Method
- Description: [Epic 6: Multi-Provider & Interactive Sandboxes, Story 6.2] As a workflow author, I want a CreateOptions struct (volume mounts, resource limits, network policy) passed to a new create_with_options default-impl on SandboxLifecycle with workflow YAML sandbox: { volumes, limits, network } support, so that providers accept rich creation config without breaking the existing create signature. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 25
- Status: pending

## Feature 27: TTY Forwarding via exec_interactive Default-Impl Method
- Description: [Epic 6: Multi-Provider & Interactive Sandboxes, Story 6.3] As a DevOps engineer, I want a new exec_interactive default-impl on SandboxLifecycle (Err(InteractiveNotSupported) by default) overridden by DockerLifecycle/PodmanLifecycle to use docker/podman exec -it for TTY-forwarded sessions, exposed via stepyard exec --interactive <session-id>, so that I can debug a running session interactively without bypassing stepyard's container abstraction. Source: _bmad-output/sandcastle-features/epics.md
- Dependencies: Feature 26
- Status: pending
