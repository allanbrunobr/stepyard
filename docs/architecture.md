# Architecture — Minion Engine

_Generated: 2026-03-13 | Type: CLI + Library | Pattern: Pipeline / Step Executor_

---

## Executive Summary

Minion Engine is a Rust async workflow engine built on tokio. It parses YAML workflow definitions, resolves a 4-layer configuration hierarchy, and dispatches steps through a trait-based executor pattern. The engine supports 10 step types, Docker sandbox isolation, Claude Code CLI orchestration, and hierarchical context scoping.

## Fundamental Principle

> **"Engine decides what runs, Agent only works when engine commands."**

The engine orchestrates the workflow; individual step executors only execute when dispatched. This separation ensures deterministic workflow control while allowing AI agents to operate within bounded scopes.

## Architecture Diagram

```
┌──────────────────────────────────────────────────────────────┐
│                          CLI Layer                            │
│  main.rs → Cli → commands.rs → display.rs                    │
└──────────────┬───────────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────────┐
│                      Engine Core                              │
│  engine/mod.rs (1,655 LOC) — Orchestrator                    │
│  ├── engine/context.rs — Context tree + Tera rendering       │
│  ├── engine/template.rs — Preprocessing (?, !, from())       │
│  └── engine/state.rs — Workflow state persistence            │
└──────┬──────────┬──────────┬──────────┬──────────────────────┘
       │          │          │          │
┌──────▼───┐ ┌───▼────┐ ┌──▼───┐ ┌───▼──────────┐
│  Steps   │ │ Config │ │Events│ │   Sandbox     │
│ 10 types │ │ 4-layer│ │  Bus │ │ Docker        │
│ 3,533 LOC│ │  merge │ │      │ │ isolation     │
└──────────┘ └────────┘ └──────┘ └───────────────┘
```

## Core Components

### 1. Engine (`engine/mod.rs` — 1,655 LOC)

The main orchestrator. Responsibilities:
- Parse and validate workflow YAML
- Initialize sandbox (if enabled)
- Dispatch steps to appropriate executors
- Manage hierarchical context tree
- Handle control flow (skip, break, fail)
- Emit lifecycle events
- Produce JSON output (--json mode)

**Key types:** `Engine`, `EngineOptions`, `StepRecord`, `WorkflowJsonOutput`

### 2. Context System (`engine/context.rs` — 490 LOC)

Hierarchical context tree with parent-child scoping:
- Variables inherited from parent contexts via `Arc<Context>`
- Step outputs stored and accessible via `{{ steps.name.field }}`
- Tera template rendering with custom preprocessing
- Chat session management for conversation continuity

**Key types:** `Context`, `ChatMessage`, `ChatHistory`

### 3. Step Executors (`steps/` — 3,533 LOC)

Trait-based extensibility:

```rust
#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn execute(&self, step: &StepDef, config: &StepConfig, ctx: &Context)
        -> Result<StepOutput, StepError>;
}

#[async_trait]
pub trait SandboxAwareExecutor: Send + Sync {
    async fn execute_sandboxed(&self, step: &StepDef, config: &StepConfig,
        ctx: &Context, sandbox: &SharedSandbox)
        -> Result<StepOutput, StepError>;
}
```

All outputs unified through `StepOutput` enum with 6 variants: `Cmd`, `Agent`, `Chat`, `Gate`, `Scope`, `Empty`.

### 4. Workflow Schema (`workflow/schema.rs` — 168 LOC)

YAML structure:
- `WorkflowDef` → top-level with name, version, config, scopes, steps
- `StepDef` → individual step with type, run/prompt, condition, config
- `StepType` enum → 10 variants (Cmd, Agent, Chat, Gate, Repeat, Map, Parallel, Call, Template, Script)
- `OutputType` enum → 5 variants (Text, Json, Integer, Lines, Boolean)

### 5. Configuration (`config/` — 198 LOC)

4-layer merge hierarchy:
1. **Global** — `config.global` in workflow YAML
2. **Type-level** — `config.agent`, `config.cmd`, etc.
3. **Pattern** — Name-pattern matching rules
4. **Step inline** — Per-step `config:` block

Resolved by `ConfigManager` into immutable `StepConfig`.

### 6. Docker Sandbox (`sandbox/` — 887 LOC)

Isolated execution environment:
- **Modes**: Disabled, FullWorkflow, AgentOnly, Devbox
- **Lifecycle**: create → copy_workspace → run_command → copy_results
- **Smart copy-back**: Uses `git status` to detect changed files
- **GH_TOKEN auto-detection**: Automatically discovers and injects token
- **Resource limits**: CPU, memory, network policies

### 7. Event System (`events/` — 191 LOC)

Non-blocking lifecycle events via `tokio::broadcast`:
- 7 event types: StepStarted, StepCompleted, StepFailed, WorkflowStarted, WorkflowCompleted, SandboxCreated, SandboxDestroyed
- Subscribers: Webhook (HTTP POST), File (JSON append)

### 8. Plugin System (`plugins/` — 147 LOC)

Dynamic loading via C ABI (`libloading`):
- `PluginStep` trait with `execute()`, `validate()`, `config_schema()`
- Registry for plugin management
- Loader for `.dylib`/`.so` files

## Data Flow

```
YAML file → parser.rs → WorkflowDef → validator.rs → Engine
                                                        │
                                                   ┌────▼────┐
                                                   │ dispatch │
                                                   │  loop    │
                                                   └────┬────┘
                                                        │
        ┌───────┬───────┬───────┬───────┬───────┬──────▼───────┐
        │ cmd   │ agent │ chat  │ gate  │ map   │ template ... │
        └───┬───┘───┬───┘───┬───┘───┬───┘───┬───┘──────┬───────┘
            │       │       │       │       │          │
            └───────┴───────┴───────┴───────┴──────────┘
                              │
                        Context Store
                              │
                        StepOutput → next step
```

## Error Handling Strategy

- `StepError` enum (7 variants): Fail, ControlFlow, Timeout, Template, Sandbox, Config, Other
- `ControlFlow` is NOT an error — represents normal flow (Skip, Break, Next, Fail)
- `thiserror` for defining error types, `anyhow` for propagation
- Step executors return `Result<StepOutput, StepError>`, never `anyhow::Result`

## Testing Strategy

- **Inline tests**: `#[cfg(test)]` modules in 25 of 41 source files (58%)
- **Integration tests**: `tests/integration.rs` for cross-module tests
- **Async tests**: `#[tokio::test]` everywhere
- **Mocking**: `wiremock` for HTTP, `tempfile` for filesystem
- **No external test framework** — standard `#[test]` + assertions

## Deployment Architecture

- **crates.io**: Published as `minion-engine` crate
- **GitHub Releases**: Pre-compiled binaries for 5 targets (Linux x86_64/aarch64, macOS x86_64/aarch64, Windows x86_64)
- **CI/CD**: GitHub Actions (`release.yml`) triggered by version tags (`v*`)
- **Docker**: `Dockerfile.sandbox` for sandbox image (Ubuntu 22.04 + Node 20 + Rust + Claude CLI + gh)
- **Homebrew**: Formula in `Formula/` directory
