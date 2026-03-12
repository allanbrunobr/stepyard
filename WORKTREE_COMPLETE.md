# Worktree wt2 — Complete

## Stories Implemented

| Story | Title | Status |
|-------|-------|--------|
| 2.1 | Step — chat (Direct LLM API) | done |
| 2.4 | Step — call (Scope Invocation) | done |
| 2.2 | Step — map (Collection Processing) | done |
| 2.3 | Step — parallel (Independent Concurrent Steps) | done |
| 2.8 | Step — template (Tera File Rendering) | done |

## Files Created

- `src/steps/chat.rs` — ChatExecutor: Anthropic + OpenAI HTTP APIs via reqwest
- `src/steps/call.rs` — CallExecutor: named scope invocation, includes shared `dispatch_scope_step` helper
- `src/steps/map.rs` — MapExecutor: serial and semaphore-bounded parallel iteration over items
- `src/steps/parallel.rs` — ParallelExecutor: concurrent nested steps with JoinSet + abort on failure
- `src/steps/template_step.rs` — TemplateStepExecutor: .md.tera file rendering with Tera

## Test Results

25 tests pass (0 failed):
- `steps::chat` — 3 tests (missing key, missing prompt, mock HTTP response via wiremock)
- `steps::call` — 3 tests (2-step scope, explicit outputs, missing scope error)
- `steps::map` — 3 tests (3 items serial, 3 items parallel, order preserved)
- `steps::parallel` — 2 tests (2 cmd steps, 1 failure cancels others)
- `steps::template_step` — 2 tests (renders with context, file not found error)

## Dependencies Added

- `futures = "0.3"` (regular)
- `wiremock = "0.6"` (dev)
- `tempfile = "3"` (dev)
