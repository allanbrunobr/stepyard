---
stepsCompleted: ['step-01-validate-prerequisites', 'step-02-design-epics', 'step-03-create-stories', 'step-04-final-validation']
inputDocuments:
  - ARCHITECTURE-MINION-ENGINE.md
  - src/steps/mod.rs
  - src/engine/mod.rs
  - src/engine/context.rs
  - src/workflow/schema.rs
---

# Minion Engine — Epic Breakdown

## Overview

This document provides the complete epic and story breakdown for Minion Engine, decomposing the requirements from the Roast gap analysis into implementable stories. These 10 features were identified by comparing our engine with Shopify's Roast and prioritized by impact and effort.

## Requirements Inventory

### Functional Requirements

FR1: Steps must support typed output parsing (.json, .integer, .lines, .boolean) so subsequent steps receive structured data instead of raw strings
FR2: Output type annotations must be declarable per-step in YAML workflow definitions
FR3: Agent steps must support --resume flag to continue an existing Claude session
FR4: Agent steps must support --fork-session flag to clone a session for parallel investigation
FR5: Session IDs must be stored in context and accessible by subsequent steps
FR6: Any step must support an `async: true` flag to run in background without blocking
FR7: Async steps must be automatically awaited when their output is referenced by a later step
FR8: A new `script` step type must evaluate inline code using an embedded scripting engine
FR9: Script steps must have read/write access to the Context store
FR10: Chat steps must support conversation history management with truncation strategies
FR11: Chat history must support .first(n), .last(n), and sliding window truncation
FR12: The engine must support loading custom step types from external plugin modules
FR13: Plugins must implement a defined trait/interface and be discoverable at runtime
FR14: Map steps must support built-in `collect` and `reduce` operations on iteration results
FR15: Reduce operations must support common aggregators (concat, sum, filter, summarize)
FR16: Template expressions must support `?` suffix for safe access returning null on missing data
FR17: Template expressions must support `!` suffix for strict access that fails loudly on missing data
FR18: The engine must emit structured events for step lifecycle (started, completed, failed)
FR19: Events must be subscribable by external systems via configurable hooks
FR20: Template expressions must support `from(step_name)` function for cross-scope output access
FR21: from() must traverse the full context hierarchy to find the named step output

### Non-Functional Requirements

NFR1: All new features must maintain backward compatibility with existing YAML workflows
NFR2: Output parsing must add negligible overhead (<1ms per step)
NFR3: Async step execution must not introduce race conditions in the context store
NFR4: The embedded scripting engine must be sandboxed and not access filesystem directly
NFR5: Plugin loading must validate plugin signatures before execution
NFR6: Event emission must be non-blocking and not slow down step execution
NFR7: All features must have unit tests with >80% code coverage
NFR8: All features must work in both host and Docker sandbox execution modes

### Additional Requirements

- All new step types and features must integrate with the existing 4-layer config system (global → type → pattern → step)
- New step types must implement SandboxAwareExecutor trait for Docker sandbox compatibility
- All new features must produce meaningful JSON output in WorkflowJsonOutput
- Context store changes must maintain parent-chain lookup semantics
- Template rendering changes must be backward-compatible with existing Tera syntax
- CLI must expose new options where applicable (e.g., --async flag visibility in dry-run)

### FR Coverage Map

FR1: Epic 1 - Output type annotations on steps
FR2: Epic 1 - YAML schema for output type declarations
FR3: Epic 2 - Resume existing Claude session
FR4: Epic 2 - Fork Claude session for parallel paths
FR5: Epic 2 - Session ID context storage
FR6: Epic 3 - Async step flag and background execution
FR7: Epic 3 - Automatic await on output reference
FR8: Epic 4 - New script step type with embedded engine
FR9: Epic 4 - Script context read/write access
FR10: Epic 5 - Chat conversation history management
FR11: Epic 5 - Truncation strategies for chat history
FR12: Epic 6 - Plugin loading and registration
FR13: Epic 6 - Plugin trait interface and discovery
FR14: Epic 7 - Map collect/reduce operations
FR15: Epic 7 - Built-in aggregator functions
FR16: Epic 8 - Safe accessor (?) template syntax
FR17: Epic 8 - Strict accessor (!) template syntax
FR18: Epic 9 - Structured event emission system
FR19: Epic 9 - External subscriber hooks
FR20: Epic 10 - from() cross-scope template function
FR21: Epic 10 - Context hierarchy traversal for from()

## Epic List

### Epic 1: Typed Output Parsing
Enable steps to produce and consume typed data (JSON objects, integers, line arrays, booleans) instead of raw strings, creating reliable contracts between workflow steps.
**FRs covered:** FR1, FR2
**Impact:** High | **Effort:** Low

### Epic 2: Agent Session Continuity
Allow Claude agent steps to resume or fork existing sessions, maintaining conversation context across multiple steps without re-reading the entire codebase each time.
**FRs covered:** FR3, FR4, FR5
**Impact:** High | **Effort:** Low

### Epic 3: Per-Step Async Execution
Allow any individual step to run asynchronously in the background, enabling natural parallelism without restructuring workflows into parallel blocks.
**FRs covered:** FR6, FR7
**Impact:** High | **Effort:** Medium

### Epic 4: Embedded Script Step
Add a lightweight script step type that evaluates inline code using an embedded interpreter, enabling data transformations without spawning external processes.
**FRs covered:** FR8, FR9
**Impact:** High | **Effort:** Medium

### Epic 5: Chat Session Management
Add conversation history management with truncation strategies to chat steps, preventing context overflow in long iterative workflows.
**FRs covered:** FR10, FR11
**Impact:** Medium | **Effort:** Medium

### Epic 6: Plugin System
Enable users to create and load custom step types as external modules without modifying the engine source code.
**FRs covered:** FR12, FR13
**Impact:** Medium | **Effort:** High

### Epic 7: Map Collect/Reduce Helpers
Add built-in collect and reduce operations to the map step so iteration results are automatically aggregated without extra manual steps.
**FRs covered:** FR14, FR15
**Impact:** Medium | **Effort:** Low

### Epic 8: Three-Accessor Pattern for Templates
Support safe (?), strict (!), and normal access patterns in template expressions for explicit control over missing data handling.
**FRs covered:** FR16, FR17
**Impact:** Low | **Effort:** Medium

### Epic 9: Event & Instrumentation System
Add structured event emission for the step lifecycle with configurable subscriber hooks for external observability tools.
**FRs covered:** FR18, FR19
**Impact:** Medium | **Effort:** Medium

### Epic 10: Cross-Scope Output Access
Add a from() function to template expressions that allows any step to access outputs from any other step regardless of scope hierarchy.
**FRs covered:** FR20, FR21
**Impact:** Low | **Effort:** Low

---

## Epic 1: Typed Output Parsing

Enable steps to produce and consume typed data (JSON objects, integers, line arrays, booleans) instead of raw strings, creating reliable contracts between workflow steps.

### Story 1.1: Define OutputType enum and extend StepDef schema

As a workflow author,
I want to declare an output type on any step definition,
So that the engine knows how to parse the step's raw output into structured data.

**Acceptance Criteria:**

**Given** a workflow YAML with a step containing `output_type: json`
**When** the YAML is parsed by the workflow parser
**Then** the StepDef struct contains the output_type field set to OutputType::Json
**And** valid values are: `json`, `integer`, `lines`, `boolean`, `text` (default)

**Given** a workflow YAML with a step that has no output_type field
**When** the YAML is parsed
**Then** the output_type defaults to OutputType::Text (current behavior preserved)

**Technical Notes:**
- Add `OutputType` enum to `workflow/schema.rs`: `Text | Json | Integer | Lines | Boolean`
- Add `output_type: Option<String>` field to `StepDef`
- Extend `StepOutput` with a `parsed` field: `Option<ParsedValue>`
- Define `ParsedValue` enum in `steps/mod.rs`: `Text(String) | Json(serde_json::Value) | Integer(i64) | Lines(Vec<String>) | Boolean(bool)`
- Files: `src/workflow/schema.rs`, `src/steps/mod.rs`

### Story 1.2: Implement output parsing logic in the engine

As a workflow engine,
I want to automatically parse step outputs based on their declared output_type,
So that downstream steps receive correctly typed data.

**Acceptance Criteria:**

**Given** a cmd step with `output_type: integer` that outputs "42\n"
**When** the step completes execution
**Then** the stored output's parsed value is `ParsedValue::Integer(42)`
**And** accessing `{{step.output}}` in templates returns "42"

**Given** a cmd step with `output_type: json` that outputs `{"count": 5}`
**When** the step completes execution
**Then** the stored output's parsed value is `ParsedValue::Json({"count": 5})`
**And** accessing `{{step.output.count}}` in templates returns "5"

**Given** a step with `output_type: integer` that outputs "not a number"
**When** the step completes execution
**Then** the engine returns a `StepError::Fail` with a clear parsing error message

**Technical Notes:**
- Add parsing function in `engine/mod.rs` after step execution completes
- Integrate with `Context::store()` to store parsed values
- Update `Context::render_template()` to handle JSON navigation (dot-path access)
- Files: `src/engine/mod.rs`, `src/engine/context.rs`

### Story 1.3: Add output type support to template rendering

As a workflow author,
I want to access parsed output fields in templates using dot notation,
So that I can reference specific values from JSON outputs.

**Acceptance Criteria:**

**Given** a step "scan" with `output_type: json` that produced `{"issues": [{"name": "XSS"}], "count": 1}`
**When** a subsequent step template references `{{scan.output.count}}`
**Then** the template renders "1"

**Given** a step "files" with `output_type: lines` that produced "a.rs\nb.rs\nc.rs"
**When** a subsequent step template references `{{files.output}}`
**Then** the template renders a comma-separated list: "a.rs, b.rs, c.rs"
**And** `{{files.output | length}}` renders "3"

**Technical Notes:**
- Extend `Context::to_tera_context()` to serialize ParsedValue appropriately
- JSON values become nested Tera objects for dot-path access
- Lines values become Tera arrays
- Files: `src/engine/context.rs`, `src/engine/template.rs`

---

## Epic 2: Agent Session Continuity

Allow Claude agent steps to resume or fork existing sessions, maintaining conversation context across multiple steps without re-reading the entire codebase each time.

### Story 2.1: Add session resume support to agent executor

As a workflow author,
I want an agent step to resume a previous Claude session,
So that the Claude agent retains full context from earlier steps without re-reading everything.

**Acceptance Criteria:**

**Given** a workflow where step "analyze" is an agent step that produces a session_id
**When** a subsequent agent step "fix" has `resume: "analyze"` in its config
**Then** the AgentExecutor passes `--resume <session_id>` to the Claude CLI
**And** the Claude agent continues with the full context of the "analyze" session

**Given** a step references `resume: "nonexistent_step"`
**When** the engine resolves the session_id
**Then** the engine returns `StepError::Fail` with "session not found for step 'nonexistent_step'"

**Technical Notes:**
- Add `resume` field to StepDef or step config
- In `AgentExecutor::build_args()`, look up session_id from context and add `--resume <id>`
- Session IDs are already stored in `AgentOutput.session_id` and `Context.session_id`
- Files: `src/steps/agent.rs`, `src/workflow/schema.rs`

### Story 2.2: Add session fork support to agent executor

As a workflow author,
I want to fork an existing Claude session into parallel investigation paths,
So that multiple agents can explore different approaches with the same baseline context.

**Acceptance Criteria:**

**Given** a workflow where step "analyze" produced session_id "sess-123"
**When** two parallel agent steps both have `fork_session: "analyze"`
**Then** each agent step passes `--resume sess-123` to Claude CLI
**And** each agent creates its own new session_id from the fork
**And** the original session "sess-123" remains unmodified

**Given** a map step iterating over 3 items with an agent sub-step using `fork_session: "analyze"`
**When** each iteration runs the agent step
**Then** each iteration forks from the same base session independently

**Technical Notes:**
- Add `fork_session` field to StepDef config
- In `AgentExecutor::build_args()`, use `--resume <id>` for both resume and fork (Claude CLI handles fork semantics)
- Each forked session gets its own new session_id in the response
- Files: `src/steps/agent.rs`, `src/workflow/schema.rs`

### Story 2.3: Expose session_id in context for template access

As a workflow author,
I want to access a step's session_id in templates,
So that I can dynamically reference sessions in subsequent steps.

**Acceptance Criteria:**

**Given** an agent step "scan" that completed with session_id "sess-abc"
**When** a template references `{{scan.session_id}}`
**Then** the template renders "sess-abc"

**Given** a cmd step "build" that has no session_id
**When** a template references `{{build.session_id}}`
**Then** the template renders an empty string (not an error)

**Technical Notes:**
- Extend `Context::to_tera_context()` to include session_id per step
- Add session_id as a nested field under each step's context data
- Files: `src/engine/context.rs`

---

## Epic 3: Per-Step Async Execution

Allow any individual step to run asynchronously in the background, enabling natural parallelism without restructuring workflows into parallel blocks.

### Story 3.1: Add async flag to StepDef and track pending futures

As a workflow author,
I want to mark any step as `async: true`,
So that it starts executing in the background without blocking subsequent steps.

**Acceptance Criteria:**

**Given** a workflow with step "scan" having `async: true`
**When** the engine encounters this step
**Then** the step is spawned as a tokio task and the engine immediately proceeds to the next step
**And** the step is registered in a pending futures map

**Given** a dry-run of a workflow with async steps
**When** the tree is printed
**Then** async steps are marked with ⚡ indicator

**Technical Notes:**
- Add `async_exec: Option<bool>` field to `StepDef` (avoid Rust keyword `async`)
- In `Engine::run()`, spawn async steps as `tokio::spawn()` tasks
- Store `JoinHandle<Result<StepOutput, StepError>>` in a `HashMap<String, JoinHandle>` on Engine
- Files: `src/workflow/schema.rs`, `src/engine/mod.rs`

### Story 3.2: Implement automatic await on output reference

As a workflow engine,
I want to automatically await an async step when its output is needed,
So that data dependencies are resolved transparently without manual synchronization.

**Acceptance Criteria:**

**Given** step "scan" is async and step "report" references `{{scan.output}}`
**When** the engine begins executing step "report" and needs to render the template
**Then** the engine checks if "scan" is in the pending futures map
**And** if so, awaits the future, stores the output in context, and removes it from pending
**And** then renders the template with the resolved output

**Given** step "scan" is async and completes before "report" starts
**When** step "report" references `{{scan.output}}`
**Then** the output is already in context and no await is needed

**Technical Notes:**
- Before template rendering in `execute_step()`, scan template for step references
- If any referenced step is in pending futures, await it first
- Add `await_pending_step(name)` method to Engine
- Files: `src/engine/mod.rs`, `src/engine/context.rs`

### Story 3.3: Await all remaining async steps at workflow end

As a workflow engine,
I want to await all pending async steps before the workflow completes,
So that no background work is silently lost.

**Acceptance Criteria:**

**Given** a workflow with 2 async steps still pending at the end of the step loop
**When** the engine finishes the last synchronous step
**Then** the engine awaits all remaining pending futures
**And** stores their outputs in context
**And** reports any failures in the workflow summary

**Technical Notes:**
- After the step loop in `run()`, call `futures::future::join_all()` on remaining handles
- Store results and include in JSON output
- Files: `src/engine/mod.rs`

---

## Epic 4: Embedded Script Step

Add a lightweight script step type that evaluates inline code using an embedded interpreter, enabling data transformations without spawning external processes.

### Story 4.1: Add Rhai scripting engine dependency and ScriptExecutor skeleton

As a developer,
I want the Rhai scripting engine integrated into the project,
So that we have a sandboxed, Rust-native scripting runtime available for inline code evaluation.

**Acceptance Criteria:**

**Given** the Cargo.toml is updated with the `rhai` dependency
**When** the project builds with `cargo build`
**Then** the build succeeds without errors
**And** a new `src/steps/script.rs` file exists with a `ScriptExecutor` struct implementing `StepExecutor`

**Given** a StepType::Script variant is added to the schema
**When** a workflow YAML contains `type: script`
**Then** the parser correctly identifies it as StepType::Script

**Technical Notes:**
- Add `rhai = "1"` to Cargo.toml
- Add `Script` variant to `StepType` enum in `workflow/schema.rs`
- Create `src/steps/script.rs` with skeleton `ScriptExecutor`
- Register in `src/steps/mod.rs`
- Files: `Cargo.toml`, `src/workflow/schema.rs`, `src/steps/script.rs`, `src/steps/mod.rs`

### Story 4.2: Implement script execution with context access

As a workflow author,
I want to write inline Rhai scripts that can read and write to the workflow context,
So that I can perform data transformations without spawning external processes.

**Acceptance Criteria:**

**Given** a script step with `run: |` containing Rhai code `let x = ctx_get("scan.output"); ctx_set("count", x.len());`
**When** the step executes
**Then** the Rhai engine reads "scan.output" from the Context store
**And** sets "count" in the Context store
**And** the step's output text is the return value of the last expression

**Given** a script step with a runtime error in the Rhai code
**When** the step executes
**Then** the engine returns `StepError::Fail` with the Rhai error message and line number

**Technical Notes:**
- Register `ctx_get(key)` and `ctx_set(key, value)` as Rhai native functions
- Map Rhai `Dynamic` types to/from `serde_json::Value`
- The script's return value becomes the StepOutput text
- Add timeout support via Rhai's `Engine::set_max_operations()`
- Files: `src/steps/script.rs`, `src/engine/mod.rs`

### Story 4.3: Add script step to engine dispatch and sandbox support

As a workflow engine,
I want script steps to be dispatched correctly and work in both host and sandbox modes,
So that scripts integrate seamlessly with the existing execution pipeline.

**Acceptance Criteria:**

**Given** a workflow with a `type: script` step
**When** the engine dispatches this step
**Then** the `ScriptExecutor` is invoked correctly

**Given** a script step running in Docker sandbox mode
**When** the engine evaluates sandbox requirements
**Then** script steps run on the host (embedded engine, no external process needed)
**And** `should_sandbox_step()` returns false for Script type

**Technical Notes:**
- Add `StepType::Script => ScriptExecutor.execute()` in engine dispatch
- Script runs embedded, so no sandbox needed (it accesses context directly)
- Files: `src/engine/mod.rs`

---

## Epic 5: Chat Session Management

Add conversation history management with truncation strategies to chat steps, preventing context overflow in long iterative workflows.

### Story 5.1: Add ChatHistory struct with message storage

As a workflow engine,
I want to maintain a conversation history per chat session,
So that multi-turn chat workflows can build on previous messages.

**Acceptance Criteria:**

**Given** a chat step with `session: "review"` that sends a message
**When** the step completes
**Then** both the sent message and response are stored in a ChatHistory keyed by "review"

**Given** a subsequent chat step with the same `session: "review"`
**When** it sends a new message
**Then** the full history (previous messages + new message) is sent to the API

**Technical Notes:**
- Create `ChatHistory` struct in `src/steps/chat.rs` or new `src/chat/history.rs`
- Store `Vec<ChatMessage>` with role (user/assistant) and content
- Add `chat_sessions: HashMap<String, ChatHistory>` to Engine or Context
- Files: `src/steps/chat.rs`, `src/engine/context.rs`

### Story 5.2: Implement truncation strategies

As a workflow author,
I want to configure truncation strategies for chat history,
So that long conversations don't exceed the model's context window.

**Acceptance Criteria:**

**Given** a chat step with config `truncation: { strategy: "last", count: 10 }`
**When** the history has 50 messages
**Then** only the last 10 messages are sent to the API

**Given** a chat step with config `truncation: { strategy: "first_last", first: 2, last: 5 }`
**When** the history has 50 messages
**Then** the first 2 messages and last 5 messages are sent (total 7)

**Given** a chat step with config `truncation: { strategy: "sliding_window", max_tokens: 50000 }`
**When** the history exceeds 50000 tokens
**Then** the oldest messages are dropped until the history fits within the token budget

**Technical Notes:**
- Add `TruncationStrategy` enum: `None | Last(n) | First(n) | FirstLast(first, last) | SlidingWindow(max_tokens)`
- Implement `ChatHistory::truncate(&self, strategy) -> Vec<ChatMessage>`
- Token counting can use simple word-based estimation (words * 1.3)
- Files: `src/steps/chat.rs`

---

## Epic 6: Plugin System

Enable users to create and load custom step types as external modules without modifying the engine source code.

### Story 6.1: Define plugin trait interface and registry

As a plugin developer,
I want a clear trait interface to implement custom step types,
So that I can create plugins that integrate with the engine's execution pipeline.

**Acceptance Criteria:**

**Given** a plugin developer implements the `PluginStep` trait
**When** the trait has methods for `name()`, `execute()`, and `validate()`
**Then** the plugin can receive step config, context access, and return StepOutput

**Given** a `PluginRegistry` struct
**When** plugins are registered at startup
**Then** the registry maps plugin names to their trait implementations

**Technical Notes:**
- Define `PluginStep` trait in new `src/plugins/mod.rs`
- `PluginRegistry` holds `HashMap<String, Box<dyn PluginStep>>`
- Trait must be object-safe for dynamic dispatch
- Files: `src/plugins/mod.rs`, `src/plugins/registry.rs`

### Story 6.2: Implement dynamic plugin loading from shared libraries

As a workflow author,
I want to load plugins from .dylib/.so files specified in the workflow config,
So that custom step types are available without recompiling the engine.

**Acceptance Criteria:**

**Given** a workflow config with `plugins: [{name: "slack", path: "./plugins/libslack.dylib"}]`
**When** the engine initializes
**Then** the plugin is loaded via `libloading` and registered in the PluginRegistry

**Given** a workflow step with `type: slack` (a plugin-provided type)
**When** the engine dispatches this step
**Then** the PluginRegistry resolves it and calls the plugin's execute method

**Technical Notes:**
- Add `libloading` dependency to Cargo.toml
- Define C ABI entry point: `extern "C" fn create_plugin() -> Box<dyn PluginStep>`
- Add plugin loading to Engine initialization
- Extend step dispatch to check PluginRegistry for unknown StepType values
- Files: `Cargo.toml`, `src/plugins/loader.rs`, `src/engine/mod.rs`

### Story 6.3: Add plugin configuration and validation

As a workflow author,
I want plugins to declare their configuration schema,
So that the engine can validate plugin step configs before execution.

**Acceptance Criteria:**

**Given** a plugin declares required config fields `["channel", "message"]`
**When** a workflow step using this plugin is missing "channel"
**Then** the workflow validator reports an error before execution begins

**Given** a plugin declares optional config fields with defaults
**When** a step omits those fields
**Then** the defaults are applied automatically

**Technical Notes:**
- Add `config_schema()` method to `PluginStep` trait
- Integrate with existing `workflow/validator.rs`
- Files: `src/plugins/mod.rs`, `src/workflow/validator.rs`

---

## Epic 7: Map Collect/Reduce Helpers

Add built-in collect and reduce operations to the map step so iteration results are automatically aggregated without extra manual steps.

### Story 7.1: Add collect operation to map step

As a workflow author,
I want map results to be automatically collected into a single output,
So that I don't need a separate step to aggregate iteration results.

**Acceptance Criteria:**

**Given** a map step with `collect: all` iterating over 5 items
**When** all iterations complete
**Then** the map step's output contains all 5 iteration outputs as a JSON array

**Given** a map step with `collect: text` iterating over 3 items
**When** all iterations complete
**Then** the map step's output is all texts concatenated with newlines

**Given** a map step without any collect config
**When** all iterations complete
**Then** behavior is unchanged (current ScopeOutput with iterations vec)

**Technical Notes:**
- Add `collect: Option<String>` to StepDef (values: "all", "text", "json")
- After map loop in `map.rs`, apply collect transformation to ScopeOutput
- "all" → serialize all outputs as JSON array
- "text" → join all output.text() with newlines
- Files: `src/steps/map.rs`, `src/workflow/schema.rs`

### Story 7.2: Add reduce operation to map step

As a workflow author,
I want to apply a reduce operation on collected map results,
So that iteration outputs are consolidated into a single meaningful value.

**Acceptance Criteria:**

**Given** a map step with `reduce: "concat"` over items producing text outputs
**When** all iterations complete
**Then** the map output is all texts concatenated with separator

**Given** a map step with `reduce: "sum"` over items producing integer outputs
**When** iterations produce values [10, 20, 30]
**Then** the map output is "60"

**Given** a map step with `reduce: "filter"` and `reduce_condition: "{{item.output | length > 0}}"`
**When** some iterations produce empty output
**Then** only non-empty outputs are included in the final result

**Technical Notes:**
- Add `reduce: Option<String>` and `reduce_condition: Option<String>` to StepDef
- Built-in reducers: concat, sum, count, filter, min, max
- Apply reduce after collect
- Files: `src/steps/map.rs`, `src/workflow/schema.rs`

---

## Epic 8: Three-Accessor Pattern for Templates

Support safe (?), strict (!), and normal access patterns in template expressions for explicit control over missing data handling.

### Story 8.1: Implement safe accessor (?) for template expressions

As a workflow author,
I want to use `{{step.output?}}` to safely access potentially missing data,
So that my workflow doesn't fail when a conditional step was skipped.

**Acceptance Criteria:**

**Given** a template with `{{scan.output?}}`
**When** the step "scan" was skipped by a gate and has no output
**Then** the template renders an empty string instead of failing

**Given** a template with `{{scan.output?}}` and "scan" has output "hello"
**When** the template is rendered
**Then** it renders "hello" (normal behavior)

**Technical Notes:**
- Pre-process template strings in `Context::render_template()` before Tera rendering
- Replace `{{name?}}` with `{{name | default(value="")}}` Tera syntax
- Or implement custom template pre-processor that catches missing variable errors
- Files: `src/engine/context.rs`, `src/engine/template.rs`

### Story 8.2: Implement strict accessor (!) for template expressions

As a workflow author,
I want to use `{{step.output!}}` to explicitly require data to exist,
So that I catch configuration errors early with clear error messages.

**Acceptance Criteria:**

**Given** a template with `{{scan.output!}}`
**When** the step "scan" has no output in context
**Then** the engine returns `StepError::Fail` with message "Required output 'scan.output' is missing (strict access)"

**Given** a template with `{{scan.output!}}` and "scan" has output
**When** the template is rendered
**Then** it renders normally

**Technical Notes:**
- Pre-process `{{name!}}` in template rendering
- Check existence before rendering; fail with descriptive error if missing
- Normal `{{name}}` behavior remains unchanged (current Tera default behavior)
- Files: `src/engine/context.rs`, `src/engine/template.rs`

---

## Epic 9: Event & Instrumentation System

Add structured event emission for the step lifecycle with configurable subscriber hooks for external observability tools.

### Story 9.1: Define event types and create EventBus

As a developer,
I want a structured event system with defined event types,
So that the engine can emit lifecycle events for each step execution.

**Acceptance Criteria:**

**Given** the EventBus is initialized with the engine
**When** a step starts, completes, or fails
**Then** the corresponding event is emitted with metadata (step name, type, timestamp, duration)

**Given** no subscribers are registered
**When** events are emitted
**Then** they are silently dropped with zero overhead

**Technical Notes:**
- Create `src/events/mod.rs` with `Event` enum: `StepStarted`, `StepCompleted`, `StepFailed`, `SandboxCreated`, `SandboxDestroyed`, `WorkflowStarted`, `WorkflowCompleted`
- `EventBus` holds `Vec<Box<dyn EventSubscriber>>`
- `EventSubscriber` trait with `on_event(&self, event: &Event)`
- Use `tokio::sync::broadcast` channel for async emission
- Files: `src/events/mod.rs`, `src/events/types.rs`

### Story 9.2: Integrate event emission into engine execution loop

As a workflow engine,
I want to emit events at key points in the execution lifecycle,
So that subscribers receive real-time updates about workflow progress.

**Acceptance Criteria:**

**Given** an EventBus with a test subscriber
**When** a workflow runs with 3 steps
**Then** the subscriber receives: WorkflowStarted, 3x StepStarted, 3x StepCompleted, WorkflowCompleted

**Given** a step fails during execution
**When** the failure is caught
**Then** a StepFailed event is emitted with the error message

**Technical Notes:**
- Add `event_bus: EventBus` field to Engine
- Emit events in `run()` and `execute_step()` methods
- Events include: step name, step type, duration, token count, cost, error message
- Files: `src/engine/mod.rs`, `src/events/mod.rs`

### Story 9.3: Add configurable webhook and file subscriber

As a workflow author,
I want to configure event subscribers in the workflow YAML,
So that events are sent to external systems like webhooks or log files.

**Acceptance Criteria:**

**Given** a workflow config with `events: { webhook: "https://hooks.example.com/minion" }`
**When** a step completes
**Then** the event is POSTed as JSON to the webhook URL

**Given** a workflow config with `events: { file: "./events.jsonl" }`
**When** events are emitted
**Then** each event is appended as a JSON line to the file

**Technical Notes:**
- Add `events` section to `WorkflowConfig`
- Implement `WebhookSubscriber` (async HTTP POST) and `FileSubscriber` (append JSONL)
- Webhook calls must be non-blocking (fire-and-forget with timeout)
- Files: `src/events/subscribers.rs`, `src/workflow/schema.rs`

---

## Epic 10: Cross-Scope Output Access

Add a from() function to template expressions that allows any step to access outputs from any other step regardless of scope hierarchy.

### Story 10.1: Implement from() template function

As a workflow author,
I want to use `{{from("step_name").output}}` in templates,
So that I can access outputs from steps outside my current scope.

**Acceptance Criteria:**

**Given** a step "global-config" at the root scope that produced output "prod"
**When** a step inside a map iteration references `{{from("global-config").output}}`
**Then** the template renders "prod" regardless of scope depth

**Given** a step references `{{from("nonexistent").output}}`
**When** the template is rendered
**Then** the engine returns `StepError::Fail` with "Step 'nonexistent' not found in any scope"

**Technical Notes:**
- Register custom Tera function `from(step_name)` that traverses the full context parent chain
- The function returns the step's output data as a Tera value
- Must search current scope first, then parent, then grandparent, etc.
- Files: `src/engine/context.rs`, `src/engine/template.rs`

### Story 10.2: Add from() support for deep field access

As a workflow author,
I want to combine from() with dot notation for deep field access,
So that I can reference specific fields from cross-scope outputs.

**Acceptance Criteria:**

**Given** a step "scan" at root scope with JSON output `{"issues": [{"name": "XSS"}]}`
**When** a nested step references `{{from("scan").output.issues | length}}`
**Then** the template renders "1"

**Given** from() is combined with the ? accessor: `{{from("scan").output?}}`
**When** "scan" doesn't exist
**Then** the template renders empty string (safe access)

**Technical Notes:**
- Ensure `from()` returns a full Tera object that supports dot-path access
- Integrate with output parsing (Epic 1) for typed field access
- Integrate with accessor pattern (Epic 8) for safe/strict access
- Files: `src/engine/context.rs`, `src/engine/template.rs`
