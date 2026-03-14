# Minion Engine — Codebase Summary

## Project Overview

**minion-engine** (v0.2.1) is a Rust-based AI workflow engine that orchestrates Claude Code CLI with YAML workflows. It enables automating code review, refactoring, issue fixing, and PR creation through declarative workflow definitions.

**Author:** Allan Bruno
**Repository:** https://github.com/allanbrunobr/minion-engine
**Language:** Rust (Edition 2021)
**Runtime:** Tokio async
**License:** MIT

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                          CLI Layer                            │
│  cli/mod.rs → cli/commands.rs → cli/display.rs               │
└──────────────┬───────────────────────────────────────────────┘
               │
┌──────────────▼───────────────────────────────────────────────┐
│                      Engine Core                              │
│  engine/mod.rs (1655 LOC) — Orchestrator                     │
│  ├── engine/context.rs — Context tree + template rendering    │
│  ├── engine/template.rs — Tera preprocessing (?,!,from())    │
│  └── engine/state.rs — Workflow state persistence             │
└──────┬──────────┬──────────┬──────────┬──────────────────────┘
       │          │          │          │
┌──────▼───┐ ┌───▼────┐ ┌──▼───┐ ┌───▼──────────┐
│  Steps   │ │ Config │ │Events│ │   Sandbox     │
│ 10 types │ │ 4-layer│ │  Bus │ │ Docker        │
│ 3533 LOC │ │  merge │ │      │ │ isolation     │
└──────────┘ └────────┘ └──────┘ └───────────────┘
```

## Stats

| Metric | Value |
|--------|-------|
| Source files | 41 |
| Total lines of code | 9,504 |
| Workflow files | 9 |
| Workflow lines | 1,339 |
| Test files | 2 |
| Files with tests | 25 (58%) |
| Hub files (3+ importers) | 7 |

## Module Breakdown

### Core (4 files, 2,189 LOC)
- **engine/mod.rs** (1,655 LOC) — Main orchestrator: step dispatch, sandbox lifecycle, event emission, async step management
- **engine/context.rs** (490 LOC) — Hierarchical context tree with parent-child inheritance, template rendering via Tera
- **steps/mod.rs** (189 LOC) — StepExecutor/SandboxAwareExecutor traits, StepOutput enum, ParsedValue types
- **main.rs** (33 LOC) — CLI entrypoint

### Step Executors (10 files, 3,533 LOC)
| Step | File | LOC | Complexity | Description |
|------|------|-----|------------|-------------|
| cmd | steps/cmd.rs | 228 | 4 | Shell command execution with sandbox support |
| agent | steps/agent.rs | 442 | 5 | Claude Code CLI with streaming JSON parsing |
| chat | steps/chat.rs | 595 | 5 | Anthropic/OpenAI API with truncation strategies |
| gate | steps/gate.rs | 159 | 2 | Conditional branching with control flow |
| map | steps/map.rs | 747 | 5 | Parallel iteration with collect/reduce |
| parallel | steps/parallel.rs | 220 | 3 | Concurrent step execution |
| repeat | steps/repeat.rs | 316 | 4 | Retry loops with break conditions |
| call | steps/call.rs | 234 | 3 | Scope invocation |
| template | steps/template_step.rs | 118 | 2 | .md.tera file rendering |
| script | steps/script.rs | 285 | 4 | Rhai scripting with context access |

### Workflow Schema (4 files, 537 LOC)
- **schema.rs** — WorkflowDef, StepDef, StepType enum (10 variants), OutputType enum
- **parser.rs** — YAML file/string parsing
- **validator.rs** — Step validation, scope reference checking, cycle detection

### CLI (4 files, 1,153 LOC)
- **commands.rs** (612 LOC) — execute, validate, list, init, inspect + pre-flight validation
- **display.rs** — Colored terminal output with progress bars
- **init_templates.rs** — Built-in workflow templates

### Sandbox (3 files, 887 LOC)
- **docker.rs** (428 LOC) — Container lifecycle: create → copy_workspace → run_command → copy_results
- **config.rs** — SandboxConfig with network policies, resource limits, auto-excludes
- **mod.rs** — Mode resolution (Disabled, FullWorkflow, AgentOnly, Devbox)

### Supporting Modules
- **config/** (198 LOC) — 4-layer config merge: global → type → pattern → step inline
- **events/** (191 LOC) — EventBus with broadcast channel, webhook/file subscribers
- **plugins/** (147 LOC) — PluginStep trait, registry, dynamic .dylib/.so loading
- **claude/** (114 LOC) — Session manager for conversation continuity
- **error.rs** (120 LOC) — StepError enum with 7 variants
- **control_flow.rs** (20 LOC) — Skip/Fail/Break/Next exceptions

## Hub Files (imported by 3+ modules)

1. **src/engine/context.rs** — Imported by all step executors, template, engine
2. **src/engine/mod.rs** — Imported by CLI commands
3. **src/steps/mod.rs** — Imported by engine, context, state, all executors
4. **src/workflow/schema.rs** — Imported by parser, validator, config, steps
5. **src/error.rs** — Imported by engine, all step executors, template
6. **src/config/mod.rs** — Imported by all step executors, manager, plugins
7. **src/control_flow.rs** — Imported by error, gate, map, repeat

## Workflows

| Workflow | Complexity | Step Types | Key Pattern |
|----------|-----------|------------|-------------|
| code-review.yaml | Moderate | cmd, gate, map, chat | Parallel file review |
| fix-issue.yaml | Complex | cmd, agent, gate, repeat | Plan → implement → validate |
| refactor.yaml | Complex | cmd, chat, agent, gate, repeat | Chat plan + agent implement |
| security-audit.yaml | Moderate | cmd, gate, map, chat | Parallel security scan |
| flaky-test-fix.yaml | Complex | cmd, call, chat, agent, gate | Multi-run detection |
| generate-docs.yaml | Moderate | cmd, gate, map, chat | Parallel doc generation |
| weekly-report.yaml | Moderate | cmd, chat | Data collection + summary |
| hello-world.yaml | Simple | cmd, gate, repeat | Basic test |
| simple-test.yaml | Simple | cmd | Minimal test |

## Key Architectural Decisions

1. **Async-first** — All step execution is async via tokio
2. **Trait-based extensibility** — StepExecutor + SandboxAwareExecutor traits
3. **Hierarchical context** — Parent-child scope inheritance with Arc references
4. **Tera templates** — With custom preprocessing for ?, !, from() syntax
5. **4-layer config** — global → type → pattern → step inline resolution
6. **Event bus** — tokio::broadcast for non-blocking lifecycle events
7. **Plugin system** — Dynamic loading via libloading (C ABI)
8. **Docker sandbox** — Full workspace copy with smart copy-back (git status check)

## Key Dependencies

- **tokio** 1.x — Async runtime
- **clap** 4.x — CLI framework (derive macros)
- **tera** 1.x — Template engine (Jinja2-like)
- **serde/serde_yaml** — YAML parsing
- **reqwest** 0.12 — HTTP client (for chat API)
- **rhai** 1.x — Embedded scripting
- **libloading** 0.8 — Dynamic library loading
- **indicatif** — Progress bars
- **colored** — Terminal colors
- **chrono** — Timestamp formatting
