# Configuration

This document describes how **`config:` keys inside a workflow YAML file** merge
for each step. It is separate from the **config file stack** documented in the
README (embedded defaults → `~/.stepyard/defaults.yaml` → `.stepyard/config.yaml`
→ workflow `config:` → step-inline `config:`). File-level defaults supply base
values; the four layers below decide how keys inside the workflow override each
other at execution time.

## Four layers inside a workflow

Workflow step config resolves in four layers, from lowest to highest priority:

1. **Global** — `config.global.*` (applies to every step)
2. **Type** — `config.<step_type>.*` (applies to steps of that type)
3. **Pattern** — `config.patterns.<regex>.*` (applies when step name matches)
4. **Step-inline** — `step.config.*` (wins over everything)

For each key, the highest-priority layer that sets it takes effect.

Both engines resolve `timeout` (cmd and agent steps) and `fail_on_error` (cmd
steps) through these layers. With no layer setting `timeout`, cmd steps stop
after 60s and agent steps after 600s; the embedded defaults set
`config.global.timeout: 300s` and `config.cmd.timeout: 60s`. On `--engine v2`,
a malformed value (`timeout: "soon"`, a bare number such as `timeout: 30`, or
`fail_on_error: "no"`) is an error when the workflow loads instead of a silent
fallback to the default.

## Shape

```yaml
config:
  global:
    timeout: 600s

  agent:
    model: claude-opus-4-6
    permissions: skip

  cmd:
    timeout: 60s

  chat:
    model: claude-haiku-4-5

  gate:
    # gate-type defaults (rare)

  patterns:
    "lint.*":
      model: claude-haiku-4-5   # cheaper model for lint-prefixed steps
      timeout: 30s
    "^(test|build).*":
      timeout: 120s

# Step-inline override (Layer 4):
steps:
  - name: lint_check
    type: agent
    config:
      timeout: 10s              # overrides pattern (30s) and global (600s)
```

## Layer 2 — step-type keys

Currently dispatched in [`config/manager.rs`](../src/config/manager.rs):

| Step type   | Config key under `config:` |
|-------------|----------------------------|
| `cmd`       | `cmd`                      |
| `agent`     | `agent`                    |
| `chat`      | `chat`                     |
| `gate`      | `gate`                     |
| others      | *(no type-level defaults — fall through to `global` + inline)* |

## Layer 3 — patterns

Keys under `config.patterns` are treated as regexes (Rust `regex` crate syntax)
matched against the step `name`. Multiple patterns can match a single step —
they merge in the order encountered, with later patterns overriding earlier ones.
Patterns always rank below step-inline config.

```yaml
config:
  patterns:
    ".*":                 # applies to every step
      timeout: 300s
    "lint.*":             # more specific — overrides the above
      timeout: 30s
```

## Value types

`StepConfig` exposes typed accessors ([`config/mod.rs`](../src/config/mod.rs)):

- `get_str(key) -> Option<&str>`
- `get_bool(key) -> bool` (defaults to `false` if missing)
- `get_u64(key) -> Option<u64>`
- `get_duration(key) -> Option<Duration>` — parses `"100ms"`, `"30s"`, `"5m"`, or bare number (seconds)

Invalid values (wrong type, malformed duration) are treated as missing — the
step uses the executor's default.

## Common keys

These are the keys currently consumed by built-in executors. Plugins may add
more.

### Agent

| Key                     | Type   | Effect                                                           |
|-------------------------|--------|------------------------------------------------------------------|
| `model`                 | string | `--model <value>`                                                |
| `system_prompt_append`  | string | `--append-system-prompt <value>`                                 |
| `permissions`           | `"skip"` | `--dangerously-skip-permissions`                               |
| `resume`                | step name | `--resume <step's session_id>`                                |
| `fork_session`          | step name | Fork from a named step's session                              |
| `session`               | `"shared"` (default) \| `"isolated"` | Workflow-level session (see STEP-TYPES.md) |
| `timeout`               | duration | Hard timeout for the CLI process                               |

### Cmd

| Key             | Type     | Effect                         |
|-----------------|----------|--------------------------------|
| `timeout`       | duration | Kill the subprocess after this |
| `fail_on_error` | bool (default `true`) | `false`: a non-zero exit completes the step instead of failing the run; `{{ steps.X.exit_code }}` keeps the real code for a following gate |

### Chat

| Key     | Type   | Effect                           |
|---------|--------|----------------------------------|
| `model` | string | Anthropic model id               |
| `session` | string | Name of a multi-turn history bucket |

## Debugging resolved config

Use `stepyard inspect <workflow.yaml>` to print the config resolved for every
step (shows exactly which keys come from which layer).

`--dry-run` also prints resolved config per step without executing anything.
Requires `--engine v1` today — the v2 harness rejects `--dry-run`.

## See also

- `README.md` — config **file** priority (embedded → user → project → workflow → step)
- `YAML-SPEC.md` — overall workflow schema
- `STEP-TYPES.md` — which config keys each step type honors
- `DOCKER-SANDBOX.md` — sandbox and API proxy settings

## Host secrets and workflows

Workflow files, the project's `.stepyard/` files and prompts can come from a
repository you do not control, so two host-side controls are off by default
and can only be lifted from the environment of `stepyard` (a workflow cannot
set them):

| Variable | Default | Effect |
|----------|---------|--------|
| `STEPYARD_ENV_ALLOW` | empty | Comma-separated host variable names that `env:` values of the form `${NAME}` may read even though the name looks like a credential (`*TOKEN*`, `*SECRET*`, `*PASSWORD*`, `*_KEY`, `DATABASE_URL`, …). Other names (`${HOME}`, `${CI}`) expand as before. |
| `STEPYARD_CHAT_BASE_URL_ALLOW` | empty | Comma-separated URL prefixes a chat step may use as `base_url`. A prefix matches only up to a URL part boundary: `https://llm.example` allows `https://llm.example/v1`, not `https://llm.example.evil.net` nor `https://llm.example:9999`; a `base_url` with `user@` is never allowed. Loopback endpoints (`http://localhost:…`, `http://127.0.0.1:…`, `http://[::1]:…`) are always allowed; any other `base_url` is refused before the request, so a workflow cannot send the key read from `api_key_env` to its own server. |

Environment values passed to `cmd` steps in a Docker/Podman sandbox reach the
container through the CLI's environment (`docker exec --env NAME`), not on the
command line, so they do not show up in `ps`. Names the container CLI reads
itself (`PATH`, `HOME`, `TMPDIR`, `SHELL`, `USER`, `*_PROXY` and names
starting with `DOCKER_`, `CONTAINER_`, `CONTAINERS_`, `PODMAN_`, `BUILDKIT_`,
`BUILDX_`, `XDG_`, `SSH_`, `LD_`, `DYLD_`) go as `--env NAME=VALUE` instead,
so a step value meant for the container does not change which `docker` runs
or which daemon it talks to; those values are visible in `ps`.

## Step output kept in the session log

On `--engine v2` (the default), `cmd`, `agent` and `chat` output (stdout and
stderr, each, the error of a failed step and a chat reply) is kept in the
session log up to `STEPYARD_MAX_STEP_OUTPUT_BYTES` (default 1 MiB). The v1
engine (`--engine v1`, also used by remote dispatch and the Slack bot) does
not apply this cap: it records no step output, but a failed step's error,
which holds a `cmd`'s stderr, is kept whole. Longer
output is cut on a character boundary and ends with a
`…[stepyard: output truncated, N more bytes …]` marker; later steps that read
`{{ steps.X.stdout }}` see the same capped value. The engine reads the log back
several times per step, so a very large value here slows every later step.

## Diagnostic logs

Two kinds of logs exist:

- **The session log**: every event of a run (step started, completed,
  failed, outputs). It is stored in the database and, unless
  `--no-file-logs` or `STEPYARD_NO_FILE_LOGS=1` is set, mirrored to
  `.stepyard/logs/<session_id>.jsonl`. It is the record of what the workflow
  did.
- **Diagnostic logs** from the CLI itself (startup reconcile, sandbox and
  workspace warnings), written to stdout through `tracing`.

| Variable | Default | Effect |
|---|---|---|
| `RUST_LOG` | `info` | Filter for diagnostic logs, e.g. `warn`, `debug`, `stepyard_harness=debug`. |
| `STEPYARD_LOG_FORMAT` | text | `json` writes one JSON object per line with a `timestamp` and, for lines logged while a v2 step runs, `span.session_id`. Use it for v2 runs nobody watches (CI, cron) to match a line to `.stepyard/logs/<session_id>.jsonl`. Remote dispatch runs v1, whose lines carry no `session_id`; its log file is named after the run id instead. The default text format has no timestamps; a line logged while a v2 step runs starts with `session{session_id=…}:`. |
