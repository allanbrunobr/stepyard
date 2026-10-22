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
| Crash recovery | No | Yes: a new process replays the log and continues |
| Sandbox | Copies the project into the container and copies results back | Container per session, but the project is not copied in yet |
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
between steps loses nothing, because the next process starts from the log.

- `executor.rs`, `agent_exec.rs`, `chat_exec.rs`, `script_exec.rs`,
  `template_exec.rs`: one executor per step type.
- `scope.rs`: `repeat` / `map` / `call` bodies, with scope context on every
  event so progress is tracked per iteration.
- `render.rs`: Tera templates (`{{ steps.x.stdout }}`, `{{ target }}`,
  `{{ vars.* }}`).
- `startup.rs`: on start, marks sessions left `running` by a dead process and
  removes orphaned `stepyard-session-<uuid>` containers.
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

## Direction

The main gap is the v2 sandbox. Today v2 creates a container per session and
runs `cmd` steps in it, but it does not copy the project into the container,
and `agent` / `chat` steps run on the host. The next step is to bring v1's
workspace handling to v2: copy the project (or clone the repo) into the
container, run agent steps there, and copy the results back. After that, v1
can be retired.

Other planned work:

- Lift the remaining v2 limitations: nested scopes, `{{ stack.* }}` /
  `{{ prompts.* }}`, `--dry-run`, `--resume <step>`, `--repo`, dashboard events.
- Release binaries, so users do not need to build from source.
