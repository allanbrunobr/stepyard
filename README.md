# Minion Engine

AI workflow engine that orchestrates Claude Code CLI through declarative YAML workflows.

## Overview

Minion Engine executes multi-step workflows that combine shell commands, Claude Code AI agent calls, conditional gates, and retry loops. It powers automated workflows like fixing GitHub issues end-to-end (fetch → plan → implement → lint → test → PR).

```
┌──────────────────────────────────────────────┐
│              minion execute                  │
│                                              │
│  YAML Workflow                               │
│  ┌─────────────────────────────────────┐    │
│  │  steps:                             │    │
│  │    cmd  → shell command             │    │
│  │    agent → Claude Code CLI          │    │
│  │    gate  → conditional branch       │    │
│  │    repeat → retry loop              │    │
│  │    map   → iterate over items       │    │
│  │    parallel → concurrent steps      │    │
│  │    call  → invoke a scope           │    │
│  │    template → render prompt file    │    │
│  └─────────────────────────────────────┘    │
│                                              │
│  Context Store: step outputs, variables      │
│  Template Engine: Tera ({{ expressions }})   │
│  Config: 4-layer merge (global/type/pattern/step) │
└──────────────────────────────────────────────┘
```

## Prerequisites

- Rust 1.75+ (`rustup` recommended)
- `claude` CLI installed and authenticated (for agent steps)
- `gh` CLI installed (for GitHub workflows)
- Docker Desktop 4.40+ (sandbox is ON by default — required unless you use `--no-sandbox`)

## Build

```bash
cargo build --release
# binary at: ./target/release/minion
```

## Install

### Via crates.io (recommended)

```bash
cargo install minion-engine
minion --version
```

### Build from source

```bash
git clone https://github.com/allanbrunobr/minion-engine.git
cd minion-engine
cargo install --path .
```

<!-- TODO: Pre-compiled binaries and Homebrew will be available after CI/CD is set up -->

## Quick Start

```bash
# Create a new workflow from a template
minion init my-workflow --template fix-issue

# Run it (sandbox is ON by default — your project stays safe)
minion execute my-workflow.yaml --verbose -- 247

# Run without sandbox (directly on your machine)
minion execute my-workflow.yaml --no-sandbox --verbose -- 247

# List available workflows
minion list

# Inspect a workflow (config, scopes, dependency graph)
minion inspect my-workflow.yaml

# Validate without running
minion validate my-workflow.yaml
```

## Usage

### `minion execute`

```bash
minion execute <workflow.yaml> [flags] -- [target]

Flags:
  --no-sandbox      Disable Docker sandbox (sandbox is ON by default)
  --verbose         Show all step outputs
  --quiet           Only show errors
  --json            Output final result as JSON
  --var KEY=VALUE   Set a workflow variable (repeatable)
  --timeout N       Override global timeout in seconds
```

### `minion validate`

```bash
minion validate <workflow.yaml>
```

Parses and validates the workflow without executing steps.

### `minion list`

```bash
minion list
```

Lists workflows found in:
- Current directory
- `./workflows/`
- `~/.minion/workflows/`

Shows name, description, and step count for each.

### `minion init`

```bash
minion init <name> [--template <template>] [--output <dir>]
```

Creates a new workflow YAML file from a built-in template.

Available templates: `blank`, `fix-issue`, `code-review`, `security-audit`

### `minion inspect`

```bash
minion inspect <workflow.yaml>
```

Shows:
- Validation status
- Resolved config layers
- Scopes with step counts
- Step dependency graph
- Dry-run summary (step type breakdown)

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
  patterns:
    "^lint.*":
      fail_on_error: false

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
    outputs: "{{ steps.run_cmd.stdout }}"

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
| `chat` | Direct Anthropic/OpenAI API call |
| `gate` | Evaluate condition, control flow |
| `repeat` | Run a scope repeatedly (retry loop) |
| `map` | Run a scope once per item in a list |
| `parallel` | Run nested steps concurrently |
| `call` | Invoke a scope once |
| `template` | Render a prompt template file |

See [docs/STEP-TYPES.md](docs/STEP-TYPES.md) for full documentation.

## Template Variables

| Variable | Description |
|----------|-------------|
| `{{ target }}` | Target argument passed to execute |
| `{{ steps.<name>.stdout }}` | stdout of a cmd step |
| `{{ steps.<name>.stderr }}` | stderr of a cmd step |
| `{{ steps.<name>.exit_code }}` | exit code of a cmd step |
| `{{ steps.<name>.response }}` | response text of an agent/chat step |
| `{{ scope.value }}` | current iteration value (in repeat/map scopes) |
| `{{ scope.index }}` | current iteration index (0-based) |
| `{{ vars.<key> }}` | variable set via `--var KEY=VALUE` |

## Example: Fix a GitHub Issue

```bash
# Sandbox is ON by default — AI runs isolated, your project stays safe
minion execute fix-issue --verbose -- 247

# Without sandbox (runs directly on your machine)
minion execute fix-issue --no-sandbox --verbose -- 247
```

This will:
1. Fetch the GitHub issue details
2. Find relevant source files
3. Plan the implementation (Claude agent)
4. Implement the fix (Claude agent)
5. Lint and auto-fix (repeat up to 3x)
6. Test and auto-fix (repeat up to 2x)
7. Create a branch, commit, push, and open a PR

All of this happens inside a Docker container **by default**. Your workspace is copied in, the AI works in isolation, and only the results are copied back. If anything goes wrong, the container is destroyed — zero impact on your project.

## Docker Sandbox

The sandbox is **ON by default** — workflows run inside an isolated Docker container with auto-forwarded credentials.

```bash
# Build the sandbox image (once)
docker build -f Dockerfile.sandbox -t minion-sandbox:latest .

# Sandbox is ON by default — just run!
minion execute code-review -- 142
minion execute security-audit
minion execute weekly-report

# To run locally without sandbox:
minion execute code-review --no-sandbox -- 142
```

Three sandbox modes:

| Mode | How to activate | What runs in Docker |
|------|-----------------|---------------------|
| **FullWorkflow** | Default (use `--no-sandbox` to disable) | Everything (all steps) |
| **AgentOnly** | `config.agent.sandbox: true` | Only AI agent steps |
| **Devbox** | `config.sandbox.mode: devbox` | Persistent dev container |

Credentials (`ANTHROPIC_API_KEY`, `GH_TOKEN`, `~/.config/gh`, `~/.ssh`) are auto-forwarded. See [docs/DOCKER-SANDBOX.md](docs/DOCKER-SANDBOX.md) for full configuration.

## Running Tests

```bash
cargo test
```

## Project Structure

```
src/
  main.rs                  # Entry point
  lib.rs                   # Module re-exports
  cli/
    mod.rs                 # CLI subcommands (execute, validate, list, init, inspect, version)
    commands.rs            # Command implementations
    init_templates.rs      # Built-in workflow templates
    display.rs             # Terminal output helpers
  engine/
    mod.rs                 # Engine core — runs workflow steps
    context.rs             # Context store (step outputs, variables)
    template.rs            # Tera template rendering
  workflow/
    schema.rs              # WorkflowDef, StepDef, StepType
    parser.rs              # YAML → WorkflowDef
    validator.rs           # Validation rules
  steps/
    cmd.rs                 # Shell command executor
    agent.rs               # Claude Code CLI executor
    chat.rs                # Direct API chat executor
    gate.rs                # Conditional gate executor
    repeat.rs              # Retry loop executor
    map.rs                 # Map-over-items executor
    parallel.rs            # Parallel step executor
    call.rs                # Scope call executor
    template_step.rs       # Template rendering executor
  config/
    manager.rs             # 4-layer config resolution
    merge.rs               # YAML/JSON merge helpers
  control_flow.rs          # ControlFlow enum (skip, fail, break, next)
  error.rs                 # StepError enum
workflows/                 # Example workflow YAML files
tests/
  integration.rs           # Integration test suite
  fixtures/                # YAML test fixtures and mock scripts
docs/
  YAML-SPEC.md             # Complete YAML format specification
  STEP-TYPES.md            # Per-step-type documentation
  CONFIG.md                # 4-layer config system
  DOCKER-SANDBOX.md        # Running steps in Docker
  EXAMPLES.md              # Example workflow catalog
```

## Documentation

- [YAML Specification](docs/YAML-SPEC.md)
- [Step Types](docs/STEP-TYPES.md)
- [Configuration System](docs/CONFIG.md)
- [Docker Sandbox](docs/DOCKER-SANDBOX.md)
- [Example Workflows](docs/EXAMPLES.md)

## API Docs

```bash
cargo doc --open
```

## License

MIT
