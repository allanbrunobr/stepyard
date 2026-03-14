# Source Tree Analysis — Minion Engine

_Generated: 2026-03-13 | Scan: Quick | Mode: Initial Scan_

---

## Directory Structure

```
minion-engine/
├── Cargo.toml                    # Package manifest (v0.2.1, MIT)
├── Cargo.lock                    # Dependency lock file
├── README.md                     # User-facing documentation
├── ARCHITECTURE-MINION-ENGINE.md # Detailed architecture doc
├── EPICS-AND-STORIES.md          # Feature tracking (legacy)
├── features.md                   # Feature tracker (40 features)
├── PROMPT.md                     # Prompt engineering reference
├── Dockerfile.sandbox            # Docker sandbox image definition
├── .gitignore
│
├── src/                          # ★ Main source tree (41 files, 9,504 LOC)
│   ├── main.rs                   # CLI entrypoint (33 LOC)
│   ├── lib.rs                    # Module re-exports
│   │
│   ├── engine/                   # ★ Core orchestration engine
│   │   ├── mod.rs                # Engine struct — step dispatch, sandbox lifecycle (1,655 LOC) [HUB]
│   │   ├── context.rs            # Hierarchical context tree + Tera rendering (490 LOC) [HUB]
│   │   ├── template.rs           # Tera preprocessing (?, !, from() syntax) (272 LOC)
│   │   └── state.rs              # Workflow state persistence/resume (143 LOC)
│   │
│   ├── steps/                    # ★ Step executors (10 types, 3,533 LOC)
│   │   ├── mod.rs                # StepExecutor/SandboxAwareExecutor traits, StepOutput enum [HUB]
│   │   ├── cmd.rs                # Shell command execution (228 LOC)
│   │   ├── agent.rs              # Claude Code CLI orchestration (442 LOC)
│   │   ├── chat.rs               # Anthropic/OpenAI API with truncation (595 LOC)
│   │   ├── gate.rs               # Conditional branching (159 LOC)
│   │   ├── map.rs                # Parallel iteration + collect/reduce (747 LOC)
│   │   ├── parallel.rs           # Concurrent step execution (220 LOC)
│   │   ├── repeat.rs             # Retry loops with break conditions (316 LOC)
│   │   ├── call.rs               # Scope invocation (234 LOC)
│   │   ├── template_step.rs      # .md.tera file rendering (118 LOC)
│   │   └── script.rs             # Rhai scripting with context access (285 LOC)
│   │
│   ├── workflow/                  # Workflow schema and parsing
│   │   ├── mod.rs                # Module root
│   │   ├── schema.rs             # WorkflowDef, StepDef, StepType enum [HUB]
│   │   ├── parser.rs             # YAML file/string parsing (91 LOC)
│   │   └── validator.rs          # Step validation, cycle detection (275 LOC)
│   │
│   ├── cli/                      # CLI interface
│   │   ├── mod.rs                # Cli struct + Command enum (65 LOC)
│   │   ├── commands.rs           # execute, validate, list, init, inspect (612 LOC)
│   │   ├── display.rs            # Colored terminal output + progress bars (184 LOC)
│   │   └── init_templates.rs     # Built-in workflow templates (292 LOC)
│   │
│   ├── sandbox/                   # Docker sandbox isolation
│   │   ├── mod.rs                # SandboxMode enum + resolution (127 LOC)
│   │   ├── docker.rs             # Container lifecycle management (428 LOC)
│   │   └── config.rs             # SandboxConfig, network/resource limits (332 LOC)
│   │
│   ├── config/                    # Configuration management
│   │   ├── mod.rs                # StepConfig struct [HUB]
│   │   ├── manager.rs            # 4-layer config merge (139 LOC)
│   │   └── merge.rs              # YAML→JSON conversion (6 LOC)
│   │
│   ├── events/                    # Event system
│   │   ├── mod.rs                # EventBus + broadcast (64 LOC)
│   │   ├── types.rs              # Event enum (7 lifecycle events) (41 LOC)
│   │   └── subscribers.rs        # Webhook + file subscribers (86 LOC)
│   │
│   ├── plugins/                   # Plugin system
│   │   ├── mod.rs                # PluginStep trait (42 LOC)
│   │   ├── registry.rs           # Plugin registry (43 LOC)
│   │   └── loader.rs             # Dynamic .dylib/.so loading (62 LOC)
│   │
│   ├── claude/                    # Claude session management
│   │   └── session.rs            # Session capture/resume (109 LOC)
│   │
│   ├── error.rs                   # StepError enum (7 variants) [HUB]
│   └── control_flow.rs            # ControlFlow enum (Skip/Fail/Break/Next) [HUB]
│
├── workflows/                     # ★ Built-in YAML workflows (9 files)
│   ├── fix-issue.yaml             # Fetch issue → plan → implement → validate → PR
│   ├── code-review.yaml           # Parallel file review
│   ├── refactor.yaml              # Chat plan + agent implement
│   ├── security-audit.yaml        # Parallel security scanning
│   ├── flaky-test-fix.yaml        # Multi-run flaky test detection
│   ├── generate-docs.yaml         # Parallel documentation generation
│   ├── weekly-report.yaml         # Data collection + summary
│   ├── hello-world.yaml           # Basic workflow test
│   └── simple-test.yaml           # Minimal test workflow
│
├── prompts/                       # ★ Tera prompt templates
│   └── (used by template steps)
│
├── tests/                         # Integration tests
│   └── integration.rs             # Cross-module integration tests
│
├── Formula/                       # Homebrew formula
│
├── .github/
│   └── workflows/
│       └── release.yml            # CI: Build + GitHub Release (5 targets)
│
├── .hive/                         # Hive feature tracking data
│   ├── codebase-map.json
│   ├── summary.md
│   └── file-tree.txt
│
└── _bmad-output/                  # BMAD workflow outputs
    └── project-context.md
```

## Hub Files (imported by 3+ modules)

| File | Role | Importers |
|------|------|-----------|
| `engine/context.rs` | Context tree + rendering | All step executors, template, engine |
| `engine/mod.rs` | Main orchestrator | CLI commands |
| `steps/mod.rs` | Traits + output types | Engine, context, state, all executors |
| `workflow/schema.rs` | Schema definitions | Parser, validator, config, steps |
| `error.rs` | Error types | Engine, all step executors |
| `config/mod.rs` | StepConfig | All step executors, manager, plugins |
| `control_flow.rs` | Flow control | Error, gate, map, repeat |

## Entry Points

- **Binary**: `src/main.rs` → `cli::Cli::run()` → `cli/commands.rs`
- **Library**: `src/lib.rs` (re-exports all modules)
- **Sandbox**: `Dockerfile.sandbox` (Docker image for isolated execution)

## Critical Paths

1. **Workflow execution**: `main.rs` → `commands::execute()` → `Engine::run()` → `dispatch_step()` → `{Type}Executor::execute()`
2. **Template rendering**: `context.rs::render_template()` → `template.rs::preprocess_template()` → Tera
3. **Config resolution**: `commands.rs` → `ConfigManager::resolve()` → `StepConfig`
4. **Sandbox lifecycle**: `Engine` → `DockerSandbox::create()` → `copy_workspace()` → `run_command()` → `copy_results()`
