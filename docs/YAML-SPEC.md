# YAML Specification

Reference for the `workflow.yaml` format accepted by `minion execute`.

## Top-level schema

```yaml
name: string                 # required — workflow identifier (used for state files, events)
version: integer             # optional — defaults to 0
description: string          # optional — free-form human description
prompts_dir: string          # optional — directory for prompt templates (default: "prompts")
config: { ... }              # optional — see CONFIG.md
scopes: { <name>: ScopeDef } # optional — named sub-workflows (see below)
steps: [ StepDef, ... ]      # required — ordered list of top-level steps
```

## `steps`

A step is one of ten types. The `type:` field is required; other fields vary by type.

```yaml
steps:
  - name: <step_name>        # required — unique within scope, used in templates as {{ steps.<name> }}
    type: cmd | agent | chat | gate | repeat | map | parallel | call | template | script
    # ... type-specific fields below
    config:                  # optional — step-inline overrides (Layer 4, see CONFIG.md)
      key: value
    output_type: text | json | integer | lines | boolean  # optional — parses output for template access
    async_exec: true | false # optional — run concurrently, await later (see STEP-TYPES.md#async)
```

### Type-specific fields

| Field           | Used by                   | Purpose                                                      |
|-----------------|---------------------------|--------------------------------------------------------------|
| `run`           | `cmd`                     | Shell command; rendered with Tera before execution           |
| `prompt`        | `agent`, `chat`, `template` | Inline prompt text or template filename (rendered with Tera) |
| `condition`     | `gate`                    | Tera boolean expression                                       |
| `on_pass`       | `gate`                    | Action when `condition` is true: `continue` or `break`        |
| `on_fail`       | `gate`                    | Action when `condition` is false: `continue`, `break`, `skip` |
| `message`       | `gate`                    | Human message attached to the gate decision                   |
| `scope`         | `repeat`, `map`, `call`   | Name of a scope to invoke                                     |
| `max_iterations`| `repeat`                  | Upper bound on loop iterations                                |
| `initial_value` | `repeat`, `map`, `call`   | Value injected as `{{ scope.value }}` in the first iteration  |
| `items`         | `map`                     | Comma-separated list (or Tera expression returning a list)    |
| `parallel`      | `map`                     | Concurrency degree (1 = serial)                               |
| `steps`         | `parallel`                | Nested step list (executed concurrently)                      |

See `STEP-TYPES.md` for per-type semantics and full examples.

## `scopes`

Named sub-workflows invoked by `repeat`, `map`, or `call`.

```yaml
scopes:
  per_file:
    steps:
      - name: lint
        type: cmd
        run: "eslint {{ scope.value }}"
    outputs: "{{ steps.lint.stdout }}"   # optional — overrides last-step output
```

## Template variables

Available in any rendered string (`run`, `prompt`, `condition`, `message`, `items`,
`initial_value`, `config.*`):

| Variable                              | Origin                                                       |
|---------------------------------------|--------------------------------------------------------------|
| `{{ target }}`                        | Positional arg after `--` (e.g., `minion execute wf.yaml -- 42`) |
| `{{ args.<key> }}`                    | Values set via `--var KEY=VALUE`                             |
| `{{ steps.<name>.stdout / stderr / exit_code }}` | Step outputs (cmd)                                |
| `{{ steps.<name>.response }}`         | Agent/chat response text                                     |
| `{{ steps.<name>.session_id }}`       | Captured Claude CLI session id (agent steps)                 |
| `{{ steps.<name>.output }}`           | Parsed output (see `output_type:`)                           |
| `{{ <name>.output }}`                 | Shorthand — top-level access to any step                     |
| `{{ from("<name>").output }}`         | Alternate accessor (useful for names with dashes)            |
| `{{ session_id }}`                    | First captured Claude session for the workflow run           |
| `{{ scope.value }} / scope.index`     | Inside repeat/map/call scopes                                |
| `{{ stack.* }}`                       | Detected stack info when `prompts/registry.yaml` exists      |

### Template operators

- `?` — safe accessor: `{{ foo.bar? }}` returns `""` if missing instead of failing.
- `!` — strict accessor: `{{ foo.bar! }}` fails the step if missing (default for bare paths).
- `from("name")` — fetch a step by literal name (supports dashes, no shadowing).

## Minimal example

```yaml
name: hello
steps:
  - name: greet
    type: cmd
    run: "echo Hello {{ target }}"
```

```bash
minion execute hello.yaml -- world
# → Hello world
```

## Full example with all top-level fields

```yaml
name: review-pr
version: 1
description: "Review a PR file-by-file and post a summary comment."
prompts_dir: prompts

config:
  global:
    timeout: 600s
  agent:
    model: claude-opus-4-6
  patterns:
    "lint.*":
      model: claude-haiku-4-5

scopes:
  review_file:
    steps:
      - name: read
        type: cmd
        run: "cat {{ scope.value }}"
      - name: review
        type: agent
        prompt: "Review this file for bugs:\n{{ steps.read.stdout }}"
    outputs: "{{ steps.review.response }}"

steps:
  - name: list_files
    type: cmd
    run: "gh pr diff {{ target }} --name-only"
    output_type: lines
  - name: per_file
    type: map
    items: "{{ steps.list_files.output | join(sep=',') }}"
    scope: review_file
    parallel: 4
  - name: post
    type: cmd
    run: "gh pr comment {{ target }} --body-file -"
```

## Validation

Run `minion validate workflow.yaml` to check syntax before execution. Validation catches:

- Missing required fields (`name`, `steps`, step-type–specific fields)
- Unknown `type:` values
- Scope references to undefined scopes
- Circular scope references
- Cycles in prompt registry parent chains

## See also

- `STEP-TYPES.md` — per-type semantics with examples
- `CONFIG.md` — 4-layer configuration merge
- `DOCKER-SANDBOX.md` — running steps in a container
- `EXAMPLES.md` — catalog of reference workflows
