# Configuration

Workflow config resolves in four layers, from lowest to highest priority:

1. **Global** — `config.global.*` (applies to every step)
2. **Type** — `config.<step_type>.*` (applies to steps of that type)
3. **Pattern** — `config.patterns.<regex>.*` (applies when step name matches)
4. **Step-inline** — `step.config.*` (wins over everything)

For each key, the highest-priority layer that sets it takes effect.

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

| Key       | Type     | Effect                         |
|-----------|----------|--------------------------------|
| `timeout` | duration | Kill the subprocess after this |

### Chat

| Key     | Type   | Effect                           |
|---------|--------|----------------------------------|
| `model` | string | Anthropic model id               |
| `session` | string | Name of a multi-turn history bucket |

## Debugging resolved config

Use `minion inspect <workflow.yaml>` to print the config resolved for every
step (shows exactly which keys come from which layer).

`--dry-run` also prints resolved config per step without executing anything.

## See also

- `YAML-SPEC.md` — overall workflow schema
- `STEP-TYPES.md` — which config keys each step type honors
