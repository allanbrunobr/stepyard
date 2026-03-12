# Minion Engine — YAML Workflow Specification

Complete reference for the workflow YAML format.

---

## Top-level Structure

```yaml
name: <string>          # required — unique workflow identifier
version: <integer>      # optional — schema version (default: 0)
description: <string>   # optional — human-readable description
prompts_dir: <path>     # optional — directory for prompt template files
config: <ConfigBlock>   # optional — 4-layer config (see docs/CONFIG.md)
scopes: <ScopeMap>      # optional — named sub-workflow scopes
steps: <StepList>       # required — ordered list of steps
```

---

## Config Block

```yaml
config:
  global:              # applied to ALL steps
    timeout: 300s
  agent:               # applied to agent steps only
    model: claude-sonnet-4-20250514
    permissions: skip
  cmd:                 # applied to cmd steps only
    fail_on_error: true
  chat:                # applied to chat steps only
    api: anthropic
  gate:                # applied to gate steps only
    default_on_fail: fail
  patterns:            # regex-matched step name → overrides
    "^lint.*":
      fail_on_error: false
```

See [CONFIG.md](CONFIG.md) for full 4-layer merge rules.

---

## Scopes

A scope is a named sub-workflow used by `repeat`, `map`, and `call` steps.

```yaml
scopes:
  <scope_name>:
    steps:             # list of StepDef (same format as top-level steps)
      - name: ...
        type: ...
    outputs: <template>  # optional — expression evaluated as scope result
```

---

## Step Types

Every step has at minimum:

```yaml
- name: <string>       # unique within its context
  type: <StepType>     # one of: cmd, agent, chat, gate, repeat, map, parallel, call, template
  config: <map>        # optional — step-level config overrides (layer 4)
```

See [STEP-TYPES.md](STEP-TYPES.md) for full per-type documentation.

---

## Template Syntax

Field values support [Tera](https://keats.github.io/tera/) templates:

| Variable | Description |
|----------|-------------|
| `{{ target }}` | Target argument passed to `minion execute -- <target>` |
| `{{ steps.<name>.stdout }}` | stdout of a cmd step |
| `{{ steps.<name>.stderr }}` | stderr of a cmd step |
| `{{ steps.<name>.exit_code }}` | exit code of a cmd step |
| `{{ steps.<name>.response }}` | response text of an agent/chat step |
| `{{ scope.value }}` | current item value (inside repeat/map scope) |
| `{{ scope.index }}` | current iteration index, 0-based |
| `{{ vars.<key> }}` | workflow variable set via `--var KEY=VALUE` |

---

## Validation Rules

- Step names must be unique within their context (top-level or scope).
- `cmd` steps require `run`.
- `agent` and `chat` steps require `prompt`.
- `gate` steps require `condition`.
- `repeat`, `map`, and `call` steps require `scope` (must reference a declared scope).
- `map` steps additionally require `items`.
- `parallel` steps require nested `steps`.
- Circular scope references are detected and rejected.
