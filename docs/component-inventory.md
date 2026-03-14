# Component Inventory — Minion Engine

_Generated: 2026-03-13_

---

## Step Executors

| Component | File | LOC | Complexity | Sandboxable | Description |
|-----------|------|-----|------------|-------------|-------------|
| CmdExecutor | `steps/cmd.rs` | 228 | 4 | Yes | Shell command execution |
| AgentExecutor | `steps/agent.rs` | 442 | 5 | Yes | Claude Code CLI orchestration with streaming JSON |
| ChatExecutor | `steps/chat.rs` | 595 | 5 | No | Anthropic/OpenAI API with truncation strategies |
| GateExecutor | `steps/gate.rs` | 159 | 2 | No | Conditional branching (pass/fail) |
| MapExecutor | `steps/map.rs` | 747 | 5 | No | Parallel iteration with collect/reduce |
| ParallelExecutor | `steps/parallel.rs` | 220 | 3 | No | Concurrent step execution |
| RepeatExecutor | `steps/repeat.rs` | 316 | 4 | No | Retry loops with break conditions |
| CallExecutor | `steps/call.rs` | 234 | 3 | No | Scope invocation |
| TemplateStepExecutor | `steps/template_step.rs` | 118 | 2 | No | Tera .md.tera rendering |
| ScriptExecutor | `steps/script.rs` | 285 | 4 | No | Rhai scripting engine |

## Core Types

| Type | Kind | File | Variants/Fields |
|------|------|------|-----------------|
| Engine | struct | `engine/mod.rs` | with_options, dry_run, run, dispatch_step |
| EngineOptions | struct | `engine/mod.rs` | verbose, quiet, json, dry_run, resume_from, sandbox_mode |
| Context | struct | `engine/context.rs` | steps, variables, parent, scope_value, session_id |
| StepOutput | enum | `steps/mod.rs` | Cmd, Agent, Chat, Gate, Scope, Empty |
| StepError | enum | `error.rs` | Fail, ControlFlow, Timeout, Template, Sandbox, Config, Other |
| ControlFlow | enum | `control_flow.rs` | Skip, Fail, Break, Next |
| StepType | enum | `workflow/schema.rs` | Cmd, Agent, Chat, Gate, Repeat, Map, Parallel, Call, Template, Script |
| ParsedValue | enum | `steps/mod.rs` | Text, Json, Integer, Lines, Boolean |
| SandboxMode | enum | `sandbox/mod.rs` | Disabled, FullWorkflow, AgentOnly, Devbox |
| TruncationStrategy | enum | `steps/chat.rs` | None, Last, First, FirstLast, SlidingWindow |

## Traits

| Trait | File | Methods | Implementors |
|-------|------|---------|-------------|
| StepExecutor | `steps/mod.rs` | `execute()` | All 10 executors |
| SandboxAwareExecutor | `steps/mod.rs` | `execute_sandboxed()` | CmdExecutor, AgentExecutor |
| PluginStep | `plugins/mod.rs` | `name()`, `execute()`, `validate()`, `config_schema()` | External plugins |
| EventSubscriber | `events/mod.rs` | `on_event()` | WebhookSubscriber, FileSubscriber |

## CLI Commands

| Command | Function | File |
|---------|----------|------|
| Execute | `commands::execute()` | `cli/commands.rs` |
| Validate | `commands::validate()` | `cli/commands.rs` |
| List | `commands::list()` | `cli/commands.rs` |
| Init | `commands::init()` | `cli/commands.rs` |
| Inspect | `commands::inspect()` | `cli/commands.rs` |

## Built-in Workflows

| Workflow | Complexity | Step Types | Pattern |
|----------|-----------|------------|---------|
| fix-issue.yaml | Complex | cmd, agent, gate, repeat | Plan → implement → validate |
| code-review.yaml | Moderate | cmd, gate, map, chat | Parallel file review |
| refactor.yaml | Complex | cmd, chat, agent, gate, repeat | Chat plan + agent implement |
| security-audit.yaml | Moderate | cmd, gate, map, chat | Parallel security scan |
| flaky-test-fix.yaml | Complex | cmd, call, chat, agent, gate | Multi-run detection |
| generate-docs.yaml | Moderate | cmd, gate, map, chat | Parallel doc generation |
| weekly-report.yaml | Moderate | cmd, chat | Data collection + summary |

## Event Types

| Event | Emitted When |
|-------|-------------|
| StepStarted | Before step execution |
| StepCompleted | After successful step |
| StepFailed | After step failure |
| WorkflowStarted | Workflow begins |
| WorkflowCompleted | Workflow finishes successfully |
| SandboxCreated | Docker container created |
| SandboxDestroyed | Docker container removed |
