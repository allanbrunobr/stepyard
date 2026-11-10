# Architecture

This document explains how Stepyard is built today and where it is going. It
is for people who want to read or change the code. For usage, see the
[README](README.md) and [`docs/`](docs/).

## Bird's eye view

Stepyard reads a workflow written in YAML and runs its steps in order. A step
can run a shell command (`cmd`), call the Claude Code CLI (`agent`), call a
model API (`chat`), check a condition (`gate`), or group other steps
(`repeat`, `map`, `call`, `parallel`). The workflow decides what happens next,
not the model.

```
workflow.yaml ─► parse + validate ─► engine ─► step executors ─► host or container
                                       │
                                       └─► session log (PostgreSQL or SQLite)
```

There are two engines in the same binary:

| | v1 (`--engine v1`) | v2 (default) |
|---|---|---|
| Code | `src/engine/`, `src/steps/` | `crates/stepyard-harness/` |
| State | In memory | Append-only session log, replayed on every step |
| Crash recovery | No | The log survives a crash and can be replayed (`Engine::resume_existing` in the library), but the CLI has no resume command: a later `stepyard execute` marks a session whose owner is gone as `failed` and starts a new one |
| Sandbox | Copies the project into the container and copies results back | Copies the project into the session container. `cmd` and `agent` run there; changes are copied back. `chat` stays on the host |
| Features | All | Same YAML; a few features are still v1-only (README, "Known limitations of v2") |

v1 is the original engine. v2 is the rewrite around a durable session log; it
is the default and new work goes there. The CLI translates the shared YAML
schema into the v2 model in `src/cli/harness_adapter.rs`, so both engines read
the same files.

## Code map

### Binary crate (`src/`)

- `main.rs`, `cli/`: argument parsing (`clap`) and subcommands. `cli/commands.rs`
  holds `execute`, including `execute_v2`, which wires the store, the sandbox
  runtime, the API proxy, signal handling and the progress view.
- `cli/harness_adapter.rs`: converts the YAML schema (`workflow/schema.rs`)
  into `stepyard_harness::Workflow`. Shape errors are reported here, so the
  harness never sees legacy YAML.
- `workflow/`: YAML parser, schema and validator, shared by both engines.
- `config/`: layered defaults (embedded → `~/.stepyard/defaults.yaml` →
  project → workflow → step).
- `engine/`, `steps/`: the v1 engine and its step executors.
- `sandbox/`: v1 Docker sandbox, sandbox configuration, and the API key proxy
  (`sandbox/proxy.rs`) that both engines use.
- `events/`, `slack/`, `plugins/`: v1 event subscribers (webhook, dashboard),
  Slack integration, and the optional dynamic plugin loader.

### `crates/stepyard-core`

Shared types with no I/O: the `Event` enum written to the session log, the
workflow model, duration and env helpers, the `Subscriber` trait. Everything
else depends on it.

### `crates/stepyard-session`

The session log. `Session::append` writes an event with a monotonic `seq`;
`Session::replay` returns events ordered by `seq`. Events are never edited.
Backends: PostgreSQL (`pg_store.rs`, default build) and SQLite
(`sqlite_store.rs`, `--features sqlite`). `file_log_mirror.rs` also writes
each session to `.stepyard/logs/<session_id>.jsonl` unless disabled.

### `crates/stepyard-harness`

The v2 engine. `Engine::step()` runs exactly one top-level step: it replays
the log, works out the next step, runs it, and appends the result. A crash
between steps loses nothing that was recorded: the log holds every completed
step, and a library caller can continue from it (`Engine::resume_existing`).
The CLI does not do that today; it always starts a new session.

- `executor.rs`, `agent_exec.rs`, `chat_exec.rs`, `script_exec.rs`,
  `template_exec.rs`: one executor per step type.
- `scope.rs`: `repeat` / `map` / `call` bodies, with scope context on every
  event so progress is tracked per iteration.
- `render.rs`: Tera templates (`{{ steps.x.stdout }}`, `{{ target }}`,
  `{{ vars.* }}`).
- `startup.rs`: on start, marks sessions left `running` by a dead process as
  `failed` (the owner holds a PostgreSQL advisory lock while it runs, so live
  sessions are skipped) and removes the `stepyard-session-<uuid>` containers
  of sessions this database knows are over.
- `process_group.rs`: child processes run in their own process group so a
  timeout or Ctrl-C kills the whole tree.

### `crates/stepyard-sandbox-orchestrator`

The `SandboxLifecycle` trait (`reuse_or_create`, `exec`, `destroy`) with Docker,
Podman, local (run on the host) and mock implementations. Containers are named
`stepyard-session-<uuid>` and are disposable: any state that matters is in the
session log.

### `packages/` (optional dashboard)

`packages/api` (Node/Express, reads the session tables) and `packages/web`
(React). Started with `npm run dev`. The CLI works without them.

## Invariants

These hold today. Breaking one is a bug.

1. **The session log is append-only.** State is rebuilt by replaying it.
2. **Every top-level step emits one `StepStarted` and one `StepCompleted` or
   `StepFailed`.** Progress counting and resume rely on this pairing.
3. **The real `ANTHROPIC_API_KEY` never enters a container.** The container
   gets a per-session dummy token and a proxy URL. If the proxy cannot start,
   the run stops; there is no fallback that passes the real key.
4. **Subprocesses get argv, not a shell string.** Commands go through
   `tokio::process::Command`. A shell is only used when the workflow asks for
   one (`sh -c`).
5. **Containers are disposable.** Destroying one at any time loses no state
   that the engine needs.
6. **Library crates return typed errors** (`thiserror`). `anyhow` stays in the
   binary.
7. **The v2 engine keeps no state between steps.** Whatever a step needs from
   earlier steps (outputs, scope progress, chat history, agent session ids) is
   read back from the session log, not held in memory. Code comments cite
   this as "invariant 7".
8. **Concurrent sessions do not affect each other.** The harness shares one
   `SandboxLifecycle` (`Send + Sync`) across sessions. Cancelling or failing
   one session does not change the others
   (`crates/stepyard-harness/tests/concurrent_sessions.rs`).

## Direction

A container run on the default engine already copies the project into the
session container (`HarnessConfig::container_workspace`), runs `cmd` and
`agent` steps there, and copies the changes back. `chat` steps still run on
the host. The same lifecycle is described in
[`docs/DOCKER-SANDBOX.md`](docs/DOCKER-SANDBOX.md).

Still missing on v2:

- Dashboard events (`config.events.dashboard` is sent only by v1).
- Nested scopes.
- `{{ stack.* }}` and `{{ prompts.* }}`.
- `--repo`.
- `--dry-run`.

Release binaries already exist. The prebuilt binary is the SQLite build; see
the README install section.
