# Minion Engine — Step Types Reference

---

## `cmd` — Shell Command

Executes a shell command via `/bin/sh -c`.

```yaml
- name: fetch
  type: cmd
  run: "gh issue view {{ target }} --json title,body"
  config:
    fail_on_error: true    # abort workflow on non-zero exit (default: true)
    timeout: 60s           # override global timeout
```

**Output stored as:**
- `steps.<name>.stdout` — captured stdout (trimmed)
- `steps.<name>.stderr` — captured stderr
- `steps.<name>.exit_code` — integer exit code

---

## `agent` — Claude Code CLI

Invokes the `claude` CLI, sends a prompt via stdin, and parses streaming JSON output.

```yaml
- name: implement
  type: agent
  prompt: |
    Fix the bug described in:
    {{ steps.analyze.stdout }}
  config:
    command: claude                          # CLI binary name/path
    model: claude-sonnet-4-20250514         # --model flag
    permissions: skip                       # use --dangerously-skip-permissions
    system_prompt_append: "Be concise."    # --append-system-prompt
    timeout: 600s
```

### Retry Configuration

Agent steps automatically retry on Claude CLI rate limit errors (HTTP 429):

```yaml
- name: agent_with_retry
  type: agent
  prompt: "Your prompt"
  config:
    max_retries: 3                    # Max retry attempts (default: 3)
    retry_base_delay_ms: 1000         # Base delay in milliseconds (default: 1000)
    retry_max_delay_ms: 8000          # Maximum delay cap in milliseconds (default: 8000)
```

**Retry behavior:**
- Detects rate limit errors from Claude CLI stderr output
- Uses exponential backoff: 1s, 2s, 4s progression
- Respects `Retry-After` headers when present in error messages
- Logs retry attempts for monitoring
- Returns `RateLimitExhausted` error after max retries

**Output stored as:**
- `steps.<name>.response` — final assistant response text
- `steps.<name>.session_id` — Claude session ID

---

## `chat` — Direct API Chat

Calls the Anthropic API (or OpenAI-compatible endpoint) directly without spawning a subprocess.

```yaml
- name: summarize
  type: chat
  prompt: "Summarize: {{ steps.fetch.stdout }}"
  config:
    api: anthropic          # or: openai
    model: claude-haiku-4-5-20251001
    max_tokens: 1024
    system: "You are a concise summarizer."
    timeout: 60s
```

### Retry Configuration

Chat steps automatically retry on API rate limit errors (HTTP 429):

```yaml
- name: chat_with_retry
  type: chat
  prompt: "Analyze this code"
  config:
    max_retries: 3                    # Max retry attempts (default: 3)
    retry_base_delay_ms: 1000         # Base delay in ms (default: 1000)
    retry_max_delay_ms: 8000          # Max delay cap in ms (default: 8000)
```

**Retry Behavior:**
- Exponential backoff: 1s, 2s, 4s (based on base_delay)
- Respects `Retry-After` header when provided
- Only retries on 429 rate limit errors
- Logs each retry attempt with delay duration

**Output stored as:**
- `steps.<name>.response` — assistant response text

---

## `gate` — Conditional Control Flow

Evaluates a Tera boolean expression and branches the workflow.

```yaml
- name: check_tests
  type: gate
  condition: "{{ steps.test.exit_code == 0 }}"
  on_pass: continue   # continue (default) | break | skip
  on_fail: fail       # fail (default) | skip | break
  message: "Tests must pass before opening PR"
```

**`on_pass` / `on_fail` actions:**

| Value | Effect |
|-------|--------|
| `continue` | Normal execution continues |
| `fail` | Abort workflow with error |
| `skip` | Skip this step, continue workflow |
| `break` | Break out of enclosing scope loop |

---

## `repeat` — Retry / Loop

Runs a named scope repeatedly until a `break` control flow is triggered or `max_iterations` is reached.

```yaml
scopes:
  lint_loop:
    steps:
      - name: lint
        type: cmd
        run: "cargo clippy --fix --allow-dirty"
      - name: check
        type: gate
        condition: "{{ steps.lint.exit_code == 0 }}"
        on_pass: break
    outputs: "{{ steps.lint.stdout }}"

steps:
  - name: fix_lint
    type: repeat
    scope: lint_loop
    max_iterations: 3
    initial_value: null    # optional initial scope.value
```

**Template variables inside scope:**
- `{{ scope.index }}` — 0-based iteration count
- `{{ scope.value }}` — current accumulated value

---

## `map` — Iterate Over Items

Runs a scope once per item in a comma-separated list.

```yaml
- name: process_files
  type: map
  scope: process_single_file
  items: "{{ steps.list_files.stdout }}"   # comma-separated items
  parallel: 1    # 1 = serial (default), N > 1 = N concurrent workers
```

**Inside the scope:**
- `{{ scope.value }}` — current item string
- `{{ scope.index }}` — 0-based index

---

## `parallel` — Concurrent Steps

Runs nested steps concurrently and waits for all to complete.

```yaml
- name: run_checks
  type: parallel
  steps:
    - name: lint
      type: cmd
      run: "cargo clippy 2>&1"
    - name: test
      type: cmd
      run: "cargo test 2>&1"
    - name: fmt
      type: cmd
      run: "cargo fmt --check 2>&1"
```

---

## `call` — Invoke a Scope

Executes a named scope once (no looping).

```yaml
scopes:
  setup:
    steps:
      - name: install
        type: cmd
        run: "npm install"

steps:
  - name: do_setup
    type: call
    scope: setup
```

---

## `template` — Render a Prompt File

Renders a Markdown/text prompt template file and makes it available for the next step.

```yaml
- name: load_prompt
  type: template
  prompt: "prompts/analyze.md"   # relative to prompts_dir or cwd
```

The rendered content is stored as `steps.<name>.response`.
