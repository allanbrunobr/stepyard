# Step Types

Each step has `type:` — one of the ten values below. Shared fields (`name`,
`config`, `output_type`, `async_exec`) are documented in `YAML-SPEC.md`;
`output_type` and `async_exec` only take effect with `--engine v1`.

## `cmd` — shell command

Runs `/bin/sh -c "<run>"` and captures stdout, stderr, exit code.

```yaml
- name: count_files
  type: cmd
  run: "find {{ target | shell_quote }} -name '*.rs' | wc -l"
  output_type: integer
```

**Outputs:** `stdout`, `stderr`, `exit_code`; `duration` is v1 only.
Non-zero exit fails the step unless wrapped in a `gate` with `on_fail: continue`.

## `agent` — Claude Code CLI

Pipes a rendered prompt to `claude -p --output-format stream-json` and parses
the streaming response.

```yaml
- name: plan
  type: agent
  prompt: "Plan the implementation of issue #{{ target }}"
  config:
    model: claude-opus-4-6
    session: shared            # shared | isolated — see "Session management" below
    permissions: skip          # --dangerously-skip-permissions
    system_prompt_append: "Reply in JSON."
```

**Outputs:** `response` (final text); `session_id` and `stats` (tokens, cost, duration) are v1 only.

### Session management

- `session: shared` (default) — reuses the first captured `session_id` across all agent steps via `--fork-session --resume <id>`. Each fork branches from the shared base.
- `session: isolated` — fresh session, no resume flags.
- `resume: <step_name>` — explicit: resume the session produced by a prior named step.
- `fork_session: <step_name>` — explicit: fork from a named step's session.

Explicit `resume`/`fork_session` override `session:`.

## `chat` — direct LLM API

Calls the Anthropic (or OpenAI-compatible) API directly. No CLI, no session.

```yaml
- name: summarize
  type: chat
  prompt: "Summarize in 3 bullets:\n{{ steps.fetch.stdout }}"
  config:
    model: claude-haiku-4-5
    session: review           # optional — grouped multi-turn history
```

**Outputs:** `response`; `model` and `stats` are v1 only. Requires `ANTHROPIC_API_KEY`.

## `gate` — conditional branch

Evaluates a Tera boolean expression and controls flow.

```yaml
- name: check_tests
  type: gate
  condition: "{{ steps.test.exit_code == 0 }}"
  on_pass: continue
  on_fail: break
  message: "tests failed"
```

**`on_pass` / `on_fail`:** one of `continue`, `break`, `skip`, `next`, `fail`.
Defaults: `on_pass: continue`, `on_fail: break`.

## `repeat` — bounded retry loop

Runs a scope until `break` (gate) or `max_iterations` is reached. `initial_value`
is available as `{{ scope.value }}` in the first iteration.

```yaml
- name: fix_until_green
  type: repeat
  scope: attempt_fix
  max_iterations: 5
  initial_value: "first attempt"
```

**Outputs:** v1 only: `iterations` (list of per-iteration outputs), `final_value`. What v2 puts in the step's `stdout` is a known gap: after a gate `break` in a later iteration, `outputs:` is rendered with the previous iteration's steps (see `docs/p3-progresso.md`).

## `map` — collection processing

Runs a scope once per item. Items come from a comma-separated list (or a Tera
expression producing one). `parallel:` controls concurrency.

```yaml
- name: review_files
  type: map
  items: "{{ steps.list.output | join(sep=',') }}"
  scope: review_file
  parallel: 4
```

Each scope invocation gets `{{ scope.value }}` (the item) and `{{ scope.index }}`.

**Outputs:** v1 only: `iterations` (one per item), `final_value` (last). On v2 the step's `stdout` is a JSON array with each item's output and `outputs:` is ignored, a known gap (see `docs/p3-progresso.md`).

## `parallel` — concurrent nested steps

Runs a fixed set of nested steps concurrently and waits for all.

```yaml
- name: checks
  type: parallel
  steps:
    - name: lint
      type: cmd
      run: "cargo clippy"
    - name: test
      type: cmd
      run: "cargo test"
    - name: fmt
      type: cmd
      run: "cargo fmt --check"
```

A failure in any branch fails the parent step. Reading a branch's output from a
later step is a known gap: neither engine exposes `steps.checks.<name>` (see
`docs/p3-progresso.md`).

## `call` — single scope invocation

Runs a scope once. Useful for grouping reusable logic without looping.

```yaml
- name: setup
  type: call
  scope: prepare_workspace
  initial_value: "{{ target }}"
```

**Outputs:** v2: `stdout` is the scope's `outputs:` rendered after the run, or the last scope step's `stdout` when there is no `outputs:`. v1 only: `iterations: [single]`, `final_value`.

## `template` — Tera file rendering

Reads `<prompts_dir>/<step_name>.md.tera` (or the path derived from `prompt:`)
and renders it against the current context.

```yaml
- name: prompt_file
  type: template
```

With a prompt template used as a dynamic path:

```yaml
- name: stack_prompt
  type: template
  prompt: "fix-lint/{{ stack.name }}"   # → prompts/fix-lint/react.md.tera
```

**Outputs:** the rendered text, as `stdout` on v2 and as `rendered` on v1 (v1 only). Typically fed to a subsequent agent step:

```yaml
- name: prompt_file
  type: template
- name: run_agent
  type: agent
  prompt: "{{ steps.prompt_file.stdout }}"   # v1: .rendered
```

## `script` — inline Rhai

Evaluates a Rhai script (see [rhai.rs](https://rhai.rs)) and stores the return
value. Useful for light data shaping between steps without spawning a shell.

```yaml
- name: total
  type: script
  run: |
    let count = ctx_get("count.stdout");
    "files changed: " + count
```

On v2 (the default) earlier step outputs are read with
`ctx_get("<step>.stdout")` (also `.stderr` and `.exit_code`), and the target
with `ctx_get("target")`; there is no `steps.*` variable in the script, and
`ctx_get` returns `()` for a missing key. A `gate` records no output, so it
has no keys. On `--engine v1` the keys are the fields of each step's own v1
output (see each type's **Outputs**): for example `agent` and `chat` have
`.response`, not `.stdout`.

**Outputs:** the evaluated Rhai value (coerced to JSON), as `stdout` on v2.

## Async execution (`async_exec`)

**v1 only.** The v2 engine (the default) ignores `async_exec` and runs the step
in order; use `--engine v1` for this.

Any step can declare `async_exec: true` — it launches in the background and its
output is awaited when first referenced (lazily) or at workflow completion.

```yaml
- name: slow_audit
  type: agent
  prompt: "Audit the whole repo"
  async_exec: true

- name: quick_task
  type: cmd
  run: "echo running while audit is in flight"

- name: use_audit
  type: cmd
  run: "echo {{ steps.slow_audit.response | shell_quote }}"   # awaits slow_audit here
```

Dry-run marks async steps with `⚡`.

## See also

- `YAML-SPEC.md` — top-level workflow schema
- `CONFIG.md` — resolving `config:` across 4 layers
- `EXAMPLES.md` — complete reference workflows
