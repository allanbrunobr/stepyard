# Worktree wt3 — COMPLETE

All 6 assigned stories have been implemented, tested, and committed.

## Stories Completed

### Story 3.1 — Async flag and pending futures (Status: review)
- `async_exec: Option<bool>` added to `StepDef` in `src/workflow/schema.rs`
- `pending_futures: HashMap<String, JoinHandle>` added to `Engine` struct
- Steps with `async_exec: true` spawned as `tokio::spawn` tasks in `run()` loop
- `dry_run()` shows ⚡ indicator for async steps

### Story 3.2 — Automatic await on output reference (Status: review)
- `await_pending_deps()` scans template fields via regex for `steps.<name>.` references
- Before each `execute_step()`, automatically awaits referenced pending async steps
- `await_pending_step(name)` helper removes handle, awaits, stores result in context

### Story 3.3 — Await all remaining async futures at workflow end (Status: review)
- After step loop in `run()`, drains `pending_futures` HashMap
- Awaits each remaining handle, stores output in context + step_records
- Failed async steps logged to stderr; workflow continues

### Story 4.1 — Rhai scripting engine dependency and ScriptExecutor skeleton (Status: review)
- `rhai = "1"` added to `Cargo.toml`
- `Script` variant added to `StepType` enum with Display + serde
- `src/steps/script.rs` created with `ScriptExecutor` implementing `StepExecutor`
- Registered as `pub mod script` in `src/steps/mod.rs`

### Story 4.2 — Script execution with context access (Status: review)
- `ctx_get(key)` reads from flattened context snapshot (e.g. "scan.stdout")
- `ctx_set(key, value)` writes to thread_local storage during script execution
- `Dynamic` <-> `serde_json::Value` bidirectional conversion
- Script return value -> `StepOutput::Cmd { stdout: return_value }`
- Timeout via `RhaiEngine::set_max_operations(1_000_000)`
- Runtime errors -> `StepError::Fail` with message

### Story 4.3 — Script step engine dispatch and sandbox support (Status: review)
- `StepType::Script => ScriptExecutor.execute()` in `execute_step()` dispatch
- `should_sandbox_step()` returns false for Script (embedded, no external process)
- `StepType::Script` arm added to `print_step_details()` for dry_run display

## Test Results

- 97 unit tests: ALL PASS
- 17 integration tests: ALL PASS
- New tests added: 5 script tests, 4 async engine tests

## Files Changed

- src/workflow/schema.rs — async_exec field, Script variant
- src/engine/mod.rs — async spawning, await_pending_deps, join_all, Script dispatch
- src/steps/script.rs — NEW FILE, full Rhai ScriptExecutor
- src/steps/mod.rs — pub mod script added
- src/workflow/validator.rs — Script validation arm
- Cargo.toml / Cargo.lock — rhai = "1" added
- src/steps/{cmd,parallel,gate,chat,map,call,agent,template_step}.rs — async_exec: None in test helpers
