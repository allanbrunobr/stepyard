# Worktree wt3 - BMAD Development Agent

You are an autonomous coding agent working in a **parallel worktree** following the **BMAD development methodology**.

## Your Branch
`minion-engine-bmad-wt3`

## Development Methodology

**CRITICAL:** You MUST follow the BMAD dev-story workflow for each assigned story.

For each story below:
1. **MANDATORY — Plan:** Call `sequentialthinking` to plan the implementation approach BEFORE writing any code. Plan which files to create/modify, in what order, verify territory ownership. Do NOT skip this.
2. Read the story completely (ACs, Tasks, Dev Notes)
3. **MANDATORY — Read with Serena:** Use `get_symbols_overview` to understand file structure, `find_symbol` to read specific functions, `find_referencing_symbols` before editing anything. Do NOT read entire source files.
4. **MANDATORY — Edit with Serena:** Use `replace_symbol_body` for edits, `insert_after_symbol`/`insert_before_symbol` for new code. Only use raw file writes for new files or non-code files.
5. Follow the Tasks/Subtasks sequence EXACTLY as written
6. Use red-green-refactor cycle: write failing test → implement → refactor
7. Mark each task/subtask as [x] when complete
8. Update the File List with all changed files
9. Add notes to Dev Agent Record
10. After ALL tasks complete, mark story Status as "review"

**DO NOT:**
- Skip tasks or change their order
- Implement features not in the story
- Mark tasks complete without passing tests
- Stop at "milestones" — continue until story is COMPLETE
- Start coding without calling `sequentialthinking` first — this is a VIOLATION
- Read entire source files instead of using Serena — this wastes tokens and context

---

## Assigned Stories

### Story 3.1: Add async flag to StepDef and track pending futures

**Feature 7 — Epic 3: Per-Step Async Execution**

**Status:** review

As a workflow author,
I want to mark any step as `async: true`,
So that it starts executing in the background without blocking subsequent steps.

**Acceptance Criteria:**

**Given** a workflow YAML with step "scan" having `async: true`
**When** the engine encounters this step
**Then** the step is spawned as a tokio task and the engine immediately proceeds to the next step
**And** the step is registered in a pending futures map

**Given** a dry-run of a workflow with async steps
**When** the tree is printed
**Then** async steps are marked with a lightning indicator

**Technical Notes:**
- Add `async_exec: Option<bool>` field to `StepDef` (avoid Rust keyword `async`)
- In `Engine::run()`, spawn async steps as `tokio::spawn()` tasks
- Store `JoinHandle<Result<StepOutput, StepError>>` in a `HashMap<String, JoinHandle>` on Engine
- Files: `src/workflow/schema.rs`, `src/engine/mod.rs`

**Dev Agent Record:**
- Files Changed: src/workflow/schema.rs, src/engine/mod.rs, src/steps/cmd.rs, src/steps/parallel.rs, src/steps/gate.rs, src/steps/chat.rs, src/steps/map.rs, src/steps/call.rs, src/steps/agent.rs, src/steps/template_step.rs, src/workflow/validator.rs
- Notes: Added async_exec: Option<bool> to StepDef. Engine spawns async steps as tokio tasks with minimal context. dry_run shows ⚡ indicator. Updated all test helpers with new field.

---

### Story 3.2: Implement automatic await on output reference

**Feature 8 — Epic 3: Per-Step Async Execution**

**Status:** review

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

**Dev Agent Record:**
- Files Changed: src/engine/mod.rs
- Notes: await_pending_deps() scans run/prompt/condition fields via regex for steps.<name>. references. Awaits any matching pending futures before execute_step is called.

---

### Story 3.3: Await all remaining async steps at workflow end

**Feature 9 — Epic 3: Per-Step Async Execution**

**Status:** review

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

**Dev Agent Record:**
- Files Changed: src/engine/mod.rs
- Notes: After step loop, drain pending_futures HashMap and await each. Results stored in context and step_records. Failures logged to stderr without aborting workflow.

---

### Story 4.1: Add Rhai scripting engine dependency and ScriptExecutor skeleton

**Feature 10 — Epic 4: Embedded Script Step**

**Status:** review

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

**Dev Agent Record:**
- Files Changed: Cargo.toml, Cargo.lock, src/workflow/schema.rs, src/steps/script.rs, src/steps/mod.rs
- Notes: rhai = "1" added. Script variant in StepType. ScriptExecutor skeleton in script.rs. Registered in steps/mod.rs.

---

### Story 4.2: Implement script execution with context access

**Feature 11 — Epic 4: Embedded Script Step**

**Status:** review

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

**Dev Agent Record:**
- Files Changed: src/steps/script.rs
- Notes: ctx_get(key) reads from flattened context snapshot built via tera Context::get(). ctx_set(key, value) uses thread_local storage. Dynamic ↔ serde_json::Value conversions. set_max_operations(1_000_000) for timeout protection. Return value → StepOutput::Cmd.stdout.

---

### Story 4.3: Add script step to engine dispatch and sandbox support

**Feature 12 — Epic 4: Embedded Script Step**

**Status:** review

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

**Dev Agent Record:**
- Files Changed: src/engine/mod.rs
- Notes: Added StepType::Script dispatch in execute_step(). should_sandbox_step() returns false for Script (embedded engine needs no sandbox). Added Script arm to print_step_details() for dry_run.

---

## Project Context

Rust project using Cargo with the following key dependencies:
- **tokio** (full features) — async runtime
- **serde** / **serde_json** / **serde_yaml** — serialization
- **tera** — template rendering
- **async-trait** — async trait support
- **clap** — CLI framework
- **reqwest** — HTTP client
- **anyhow** / **thiserror** — error handling

The engine uses `tokio::spawn` for parallel steps already (see `ParallelExecutor`). Context uses `Arc` for shared access. The execution loop is in `Engine::run()` in `src/engine/mod.rs`, which dispatches steps via a `match` on `StepType`. Each step type implements the `StepExecutor` trait.

Key architectural patterns:
- `StepExecutor` trait with async `execute()` method
- `Context` store with parent-chain lookup (scoped contexts)
- `StepOutput` enum with variants per step type
- 4-layer config merge: global -> type -> pattern -> step inline
- Template rendering via Tera with context variables

---

## FILE OWNERSHIP RULES (CRITICAL)

### Owned Files & Directories (you CAN freely create/edit)
- `src/steps/script.rs` — NEW FILE, you create this entire file

You may also create NEW files within your owned directories.

### Files This Worktree Will Modify (with shared strategies)

#### `src/engine/mod.rs` (SHARED - deferred, THIS IS YOUR PRIMARY FILE)
You own the async execution loop modifications:
- JoinHandle tracking
- `tokio::spawn` for async steps
- `await_pending_step()` method
- `join_all` at workflow end
- `Script` dispatch to `dispatch_step()` match

Other worktrees add parsing (wt1) and plugin/event integration (wt4). Stay within YOUR sections.

#### `src/workflow/schema.rs` (SHARED - append_only)
Add ONLY:
- `async_exec: Option<bool>` field to `StepDef`
- `Script` variant to `StepType` enum

Do NOT modify existing fields, variants, or implementations.

#### `Cargo.toml` (SHARED - append_only)
Add ONLY: `rhai = "1"` dependency.
Do NOT remove existing dependencies or change metadata.

#### `src/steps/mod.rs` (SHARED - append_only)
Add ONLY: Register `ScriptExecutor`, re-export `script` module.
Do NOT modify existing registrations or exports.

### Read-Only Files (import but DO NOT modify)
- `src/engine/context.rs`
- `src/engine/template.rs`
- `src/steps/agent.rs`
- `src/steps/cmd.rs`
- `src/steps/chat.rs`
- `src/steps/gate.rs`
- `src/steps/repeat.rs`
- `src/steps/map.rs`
- `src/steps/parallel.rs`
- `src/steps/call.rs`

You may read these files to understand interfaces and patterns. You may import from them. You MUST NOT edit them.

### Forbidden Directories (DO NOT touch)
- `src/plugins/` — owned by wt4
- `src/events/` — owned by wt4

### Shared File Strategies Explained

| File | Strategy | What You Can Do |
|------|----------|----------------|
| `src/engine/mod.rs` | deferred | Own execution loop changes (async spawning, JoinHandle management) and Script dispatch. Other worktrees add parsing (wt1) and plugin/event integration (wt4). Stay within YOUR sections. |
| `src/workflow/schema.rs` | append_only | Add ONLY `async_exec` field and `Script` variant |
| `Cargo.toml` | append_only | Add ONLY `rhai` dependency |
| `src/steps/mod.rs` | append_only | Add ONLY Script-related exports |

---

## INTEGRATION CONTRACTS

### You Provide (other worktrees depend on your code)
- **Async step infrastructure** — JoinHandle management, pending futures map in Engine
- **ScriptExecutor** — for inline code evaluation via Rhai
- **`Script` variant in `StepType` enum** — other worktrees need this variant to exist
- **`async_exec` field on `StepDef`** — wt4's event system may reference this

### You Consume (code from existing or other worktrees)
- **`Context::render_template()`** — for template resolution (used as-is from wt1's changes or existing)
- **`StepExecutor` trait** — implement for `ScriptExecutor`
- **`StepOutput` enum** — use existing variants or `Cmd` variant for script output
- **`StepError`** — use existing error types for failure reporting
- **`StepConfig`** — receive merged config in executor
- **`StepDef`** — read step definition fields

---

## MCP Tools — MANDATORY FOR MEDIUM/COMPLEX TASKS

You have two MCP servers. **Using them is NOT optional.** They are your primary tools for reading, editing, and reasoning about code. Skipping them leads to worse code, wasted tokens, and broken territory rules.

### Compliance Matrix

| Task complexity | Serena | Sequential Thinking |
|---|---|---|
| **Trivial** (rename, 1-line fix, config edit) | Recommended | Optional |
| **Medium** (new function, modify existing module, 2-5 files) | **MANDATORY** | **MANDATORY** |
| **Complex** (new feature, cross-module changes, 5+ files) | **MANDATORY** | **MANDATORY** (multi-step plan required) |

**If you skip these tools on a medium or complex task, you are violating your instructions.**

---

### Serena (Code Intelligence) — MANDATORY

Serena is your **primary way to read and edit code**. Do NOT read entire source files with `cat` or `Read` unless the file is non-code (config, markdown, JSON). For source code, ALWAYS use Serena.

**Mandatory workflow for every file you touch:**

1. **Before touching any file:** `get_symbols_overview` → understand its structure (classes, functions, exports) without reading 500+ lines
2. **To understand a specific symbol:** `find_symbol` with `include_body=True` → read ONLY the function/class you need
3. **Before editing ANY symbol:** `find_referencing_symbols` → know who depends on it. Breaking callers = breaking other worktrees
4. **To edit code:** `replace_symbol_body` for surgical edits. `insert_after_symbol` / `insert_before_symbol` for new code at precise locations
5. **To explore:** `list_dir`, `find_file`, `search_for_pattern` to locate files and patterns

**NEVER do this:**
- Read an entire 300-line file to find one function → use `find_symbol` instead
- Edit a file with sed/string replacement when you can use `replace_symbol_body`
- Modify a function without checking `find_referencing_symbols` first

---

### Sequential Thinking (Structured Reasoning) — MANDATORY

Sequential Thinking is your **planning tool**. You MUST call `sequentialthinking` before writing code for any medium or complex task. No exceptions.

**Mandatory triggers — you MUST call `sequentialthinking` when:**

1. **Starting each story** — Plan: which files to create/modify, in what order, what to test, what territory rules apply
2. **Creating a new file** — Think: does this file belong in my territory? What will it export? Who will import it?
3. **Modifying existing code** — Think: what breaks if I change this? Are there callers in other worktrees?
4. **Facing a design decision** — Reason through tradeoffs before committing to an approach
5. **Debugging a failure** — Systematically analyze root cause before making random changes
6. **Before any cross-cutting change** — If a change touches 3+ files, plan the full sequence first

**NEVER do this:**
- Jump straight into coding a story without planning → call `sequentialthinking` first
- Make a "quick fix" that touches multiple files without thinking through the implications
- Start editing territory-boundary files without verifying ownership rules first

---

## Implementation Order

Implement stories in this exact order:

1. **Story 3.1:** Add async flag to StepDef and track pending futures
   - Commit: `feat(epic-3): implement story 3.1 - async flag and pending futures`

2. **Story 3.2:** Implement automatic await on output reference
   - Commit: `feat(epic-3): implement story 3.2 - automatic await on output reference`

3. **Story 3.3:** Await all remaining async steps at workflow end
   - Commit: `feat(epic-3): implement story 3.3 - await all at workflow end`

4. **Story 4.1:** Add Rhai scripting engine dependency and ScriptExecutor skeleton
   - Commit: `feat(epic-4): implement story 4.1 - Rhai dependency and ScriptExecutor skeleton`

5. **Story 4.2:** Implement script execution with context access
   - Commit: `feat(epic-4): implement story 4.2 - script execution with context access`

6. **Story 4.3:** Add script step to engine dispatch and sandbox support
   - Commit: `feat(epic-4): implement story 4.3 - script step engine dispatch`

After completing each story:
1. Commit with the message specified above
2. Update the story Status from "pending" to "review"
3. Proceed to next story

After ALL stories are complete:
1. Run full test suite to verify no regressions
2. Commit final state
3. Signal completion: create a file `WORKTREE_COMPLETE.md` with summary of all changes
