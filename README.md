# Minion Engine

AI workflow engine that orchestrates Claude Code CLI through declarative YAML workflows.

## Overview

Minion Engine executes multi-step workflows that combine shell commands, Claude Code AI agent calls, conditional gates, and retry loops. It powers automated workflows like fixing GitHub issues end-to-end (fetch → plan → implement → lint → test → PR).

## Prerequisites

- Rust 1.75+ (`rustup` recommended)
- `claude` CLI installed and authenticated (for agent steps)
- `gh` CLI installed (for GitHub workflows)

## Build

```bash
# Build release binary
cargo build --release

# The binary is at:
./target/release/minion
```

## Install

```bash
cargo install --path .
```

## Usage

### Execute a workflow

```bash
# Basic execution
minion execute workflows/fix-issue.yaml -- 247

# With verbose output (shows all step outputs)
minion execute workflows/fix-issue.yaml -- 247 --verbose

# With quiet output (errors only)
minion execute workflows/fix-issue.yaml -- 247 --quiet

# Set workflow variables
minion execute workflows/my-workflow.yaml -- --var KEY=VALUE

# Output results as JSON
minion execute workflows/my-workflow.yaml -- --json
```

### Validate a workflow

```bash
# Check YAML is valid without running
minion validate workflows/fix-issue.yaml
```

### List available workflows

```bash
minion list
```

### Show version

```bash
minion version
```

## Workflow YAML Format

```yaml
name: my-workflow
version: 1
description: "What this workflow does"

config:
  global:
    timeout: 300s
  agent:
    model: claude-sonnet-4-20250514
    permissions: skip
  cmd:
    fail_on_error: true

scopes:
  retry_loop:
    steps:
      - name: run_cmd
        type: cmd
        run: "npm test"
        config:
          fail_on_error: false
      - name: check
        type: gate
        condition: "{{ steps.run_cmd.exit_code == 0 }}"
        on_pass: break

steps:
  - name: fetch_data
    type: cmd
    run: "gh issue view {{ target }} --json title,body"

  - name: analyze
    type: agent
    prompt: |
      Analyze this issue and suggest a fix:
      {{ steps.fetch_data.stdout }}

  - name: validate
    type: repeat
    scope: retry_loop
    max_iterations: 3
```

## Step Types

| Type | Description |
|------|-------------|
| `cmd` | Execute shell command |
| `agent` | Invoke Claude Code CLI |
| `gate` | Evaluate condition, control flow |
| `repeat` | Run a scope repeatedly (retry loop) |

## Template Variables

- `{{ target }}` — the target argument passed to execute
- `{{ steps.<name>.stdout }}` — stdout of a cmd step
- `{{ steps.<name>.exit_code }}` — exit code of a cmd step
- `{{ steps.<name>.response }}` — response text of an agent step
- `{{ scope.value }}` — current iteration value (in repeat scopes)
- `{{ scope.index }}` — current iteration index (0-based)

## Example: Fix a GitHub Issue

```bash
# Run the full fix-issue workflow on issue #247
minion execute workflows/fix-issue.yaml -- 247 --verbose
```

This will:
1. Fetch the GitHub issue details
2. Find relevant source files
3. Plan the implementation (Claude agent)
4. Implement the fix (Claude agent)
5. Lint and auto-fix (repeat up to 3x)
6. Test and auto-fix (repeat up to 2x)
7. Create a branch, commit, push, and open a PR

## Running Tests

```bash
cargo test
```

## Project Structure

```
src/
  main.rs          # Entry point
  lib.rs           # Module re-exports
  cli/             # CLI commands (execute, validate, list, version)
  engine/          # Engine core and context store
  workflow/        # YAML schema, parser, validator
  steps/           # Step executors (cmd, agent, gate, repeat)
  config/          # 4-layer config resolution
  control_flow.rs  # ControlFlow enum (skip, fail, break, next)
  error.rs         # StepError enum
workflows/         # Example workflow YAML files
tests/             # Integration tests
```

## License

MIT
