# Minion Engine — Configuration System

Minion Engine uses a **4-layer configuration system** where each layer overrides the previous one.

---

## The 4 Layers (lowest → highest priority)

```
Layer 1: global      — applies to every step
Layer 2: by type     — applies to all steps of a specific type (cmd, agent, chat, gate)
Layer 3: by pattern  — applies to steps whose name matches a regex
Layer 4: step inline — specified directly on the step definition
```

The final resolved config for any step is the merge of all four layers, with higher layers winning on key conflicts.

---

## Full Example

```yaml
config:
  # Layer 1: global — applies to all steps
  global:
    timeout: 300s
    fail_on_error: true

  # Layer 2: by step type
  agent:
    command: claude
    model: claude-sonnet-4-20250514
    permissions: skip
    timeout: 600s         # overrides global for agent steps only

  cmd:
    fail_on_error: true
    timeout: 60s

  chat:
    api: anthropic
    model: claude-haiku-4-5-20251001
    max_tokens: 2048

  gate:
    default_on_fail: fail

  # Layer 3: regex patterns on step name
  patterns:
    "^lint.*":
      fail_on_error: false    # lint steps never abort the workflow
    "^test.*":
      timeout: 120s

steps:
  - name: build
    type: cmd
    run: "cargo build"
    # Layer 4: step inline — overrides everything above
    config:
      timeout: 120s
      fail_on_error: false
```

---

## Config Keys by Step Type

### Global keys (apply to all steps)

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `timeout` | duration string | `300s` | Max execution time. Formats: `60s`, `5m`, `1500ms` |
| `fail_on_error` | bool | `true` | Abort workflow on step failure |

### `agent` keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `command` | string | `claude` | Claude CLI binary name or full path |
| `model` | string | — | `--model` flag value |
| `permissions` | `skip` \| — | — | If `skip`, adds `--dangerously-skip-permissions` |
| `system_prompt_append` | string | — | `--append-system-prompt` value |
| `timeout` | duration | `600s` | Longer default for AI steps |

### `cmd` keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `fail_on_error` | bool | `true` | Abort on non-zero exit code |
| `timeout` | duration | `300s` | Max execution time |

### `chat` keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `api` | `anthropic` \| `openai` | `anthropic` | API to use |
| `model` | string | — | Model identifier |
| `max_tokens` | integer | `4096` | Max response tokens |
| `system` | string | — | System prompt |
| `timeout` | duration | `60s` | API call timeout |

---

## Runtime Variable Overrides

Pass variables at execution time with `--var`:

```bash
minion execute workflow.yaml -- --var MODEL=claude-opus-4-6
```

Access in templates as `{{ vars.MODEL }}`.

---

## Duration Format

| String | Meaning |
|--------|---------|
| `300s` | 300 seconds |
| `5m` | 5 minutes |
| `1500ms` | 1500 milliseconds |
| `300` | 300 seconds (bare integer) |
