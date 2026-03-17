# Minion Engine

![Minion Engine High-Level Architecture](docs/architecture-high-level.jpg)

**Automate code review, bug fixing, and PR creation with AI — defined in YAML, executed in Docker.**

Minion Engine is a CLI tool that runs multi-step AI workflows. You define what you want in a YAML file, and it orchestrates everything: shell commands, Claude AI calls, conditional logic, and parallel execution — all inside an isolated Docker sandbox.

```
You write YAML → Minion runs AI workflows → Results appear as PR comments, fixes, reports
```

## Why?

**Without Minion Engine**, reviewing a PR means:
- Open the PR, read each file manually
- Switch context between Python, TypeScript, Rust conventions
- Remember to check for security issues, type safety, error handling
- Write your findings as a comment

**With Minion Engine**, one command does it all:
```bash
minion execute code-review.yaml -- 42
```
Every changed file is reviewed with **language-specific criteria** (Python gets Python rules, TypeScript gets TypeScript rules), the project architecture is considered, and a structured report is posted as a PR comment.

**Typical execution times:**

| Workflow | Time | What affects it |
|----------|------|-----------------|
| **code-review** | 2–5 min | Number of changed files, size of diffs |
| **fix-issue** | 8–15 min | Complexity of the issue, number of lint/test retry loops |
| **security-audit** | 3–6 min | Number of source files, parallelism level |
| **generate-docs** | 4–8 min | Number of source files to document |

Execution time depends on several factors:
- **Docker sandbox setup** (1–5 min on first run per session): creating the container, copying your project files in, and initializing git. Larger projects with more files take longer to copy.
- **API response time** (10–60s per call): each `chat` or `agent` step makes a round-trip to the Claude API. Longer prompts and larger context windows increase latency.
- **Retry loops**: workflows like `fix-issue` run lint and test gates in a `repeat` scope — if tests fail, the agent fixes and retries (up to `max_iterations`). Each iteration adds a full agent + cmd cycle.
- **Parallelism vs rate limits**: `map` steps with `parallel: 5` can trigger API rate limits (429), which currently causes failures. Lower parallelism is more reliable.
- **First run**: the Docker image build (`minion-sandbox:latest`) adds ~2 minutes the very first time. Subsequent runs use the cached image.

## Quick Start

```bash
# 1. Install (Rust toolchain required)
cargo install minion-engine

# 2. Interactive setup — checks requirements and configures API keys
minion setup

# 3. Go to your project and run a workflow
cd /path/to/your-project
minion execute code-review.yaml -- 42   # Review PR #42
```

That's it. Docker image is **built automatically** on first run. No manual setup needed.

### Install with Slack Bot

```bash
# Install with Slack integration
cargo install minion-engine --features slack

# Run interactive setup (includes Slack configuration)
minion setup

# Start the bot
minion slack start
```

## What Can It Do?

| Workflow | What it does |
|----------|-------------|
| **code-review** | Review a PR — detects language per file, loads language-specific prompts, posts findings as PR comment |
| **fix-issue** | Fetch a GitHub issue → plan → implement → lint → test → create PR |
| **fix-test** | Detect failing tests → analyze → fix → verify — repeat until green |
| **security-audit** | Scan codebase for OWASP vulnerabilities with AI analysis |
| **generate-docs** | Generate documentation from source code |

All workflows are YAML files you can customize or create from scratch.

## Prerequisites

| Requirement | How to get it | Notes |
|-------------|---------------|-------|
| **Rust toolchain** | [rustup.rs](https://rustup.rs) | For `cargo install` |
| **ANTHROPIC_API_KEY** | [console.anthropic.com](https://console.anthropic.com) | `export ANTHROPIC_API_KEY="sk-ant-..."` |
| **Docker Desktop** | [docker.com](https://www.docker.com/products/docker-desktop/) | Sandbox runs workflows in isolation |
| **gh CLI** | [cli.github.com](https://cli.github.com) | `gh auth login` — GH_TOKEN is **auto-detected** |

> **Docker image auto-build:** The first time you run a workflow, Minion automatically builds the sandbox image (`minion-sandbox:latest`). This takes ~2 minutes once and never needs to be repeated.

## Features

### 🐳 Docker Sandbox (default)

![Secure Docker Sandbox Workflow](docs/architecture-docker-sandbox.jpg)

Every workflow runs inside an isolated Docker container. Your project is copied in, the AI works in isolation, and only the results come back. If anything goes wrong, the container is destroyed — zero impact on your project.

```bash
minion execute code-review.yaml -- 42        # Sandbox ON (default)
minion execute code-review.yaml --no-sandbox -- 42  # Run locally instead
```

### 🔍 Language-Aware Code Review

The code review workflow detects the language of each changed file and applies **language-specific review criteria**:

- **Python** → checks for bare `except:`, missing type annotations, mutable default arguments
- **TypeScript** → checks for `any` types, missing `await`, unhandled promise rejections
- **Rust** → checks for `unwrap()` in production, unnecessary clones, unsafe blocks
- **Java** → checks for resource leaks, null safety, checked exceptions
- Falls back to generic review for other languages

### 📐 Architecture Context

If your project has a `CLAUDE.md`, `ARCHITECTURE.md`, or `README.md`, the code review workflow reads it automatically and uses it to evaluate whether changes align with your project's design.

### 🎯 Stack Detection & Prompt Registry

Minion detects your project's tech stack (Rust, Python, TypeScript, React, Java, etc.) from file markers (`Cargo.toml`, `package.json`, `requirements.txt`) and uses it to select the right prompts and tools.

## CLI Reference

### `minion execute`

```bash
minion execute <workflow.yaml> [flags] -- [target]
```

| Flag | Description |
|------|-------------|
| `--no-sandbox` | Disable Docker sandbox (sandbox is ON by default) |
| `--verbose` | Show all step outputs |
| `--quiet` | Only show errors |
| `--json` | Output result as JSON |
| `--dry-run` | Show what steps would run without executing |
| `--var KEY=VALUE` | Set a workflow variable (repeatable) |
| `--timeout SECONDS` | Override global timeout |
| `--resume STEP` | Resume from a specific step |

```bash
# Examples
minion execute code-review.yaml -- 42              # Review PR #42
minion execute fix-issue.yaml --verbose -- 247     # Fix issue with verbose output
minion execute fix-test.yaml -- 7                  # Fix failing tests for PR #7
minion execute security-audit.yaml                 # Security audit (no target needed)
minion execute workflow.yaml --var mode=strict -- 5 # Pass variables
```

### `minion init`

```bash
minion init <name> [--template <template>]
```

Creates a new workflow from a built-in template.

Templates: `blank`, `fix-issue`, `code-review`, `security-audit`

### `minion validate`

```bash
minion validate <workflow.yaml>
```

Parses and validates a workflow without executing it.

### `minion list`

```bash
minion list
```

Lists workflows found in the current directory, `./workflows/`, and `~/.minion/workflows/`.

### `minion inspect`

```bash
minion inspect <workflow.yaml>
```

Shows config layers, scopes, step dependency graph, and dry-run summary.

### `minion config`

Manage default configuration (model, provider, timeouts).

```bash
minion config show          # Show current effective configuration (embedded + user + project merged)
minion config init          # Create or edit user-level defaults (~/.minion/defaults.yaml)
minion config set KEY VALUE # Set a config value (dot notation)
minion config path          # Show where config files are located
```

```bash
# Examples
minion config set chat.model claude-opus-4-20250514    # Change the default AI model
minion config set chat.temperature 0.5             # Adjust creativity
minion config set global.timeout 600s              # Increase timeout
minion config set agent.model claude-sonnet-4-20250514   # Change agent model
```

**Config priority** (lowest → highest):
1. **Embedded defaults** — compiled into the binary, always available
2. **User-level** — `~/.minion/defaults.yaml` (created with `minion config init`)
3. **Project-level** — `.minion/config.yaml` in your project root
4. **Workflow YAML** — `config:` section in each workflow file
5. **Step inline** — `config:` on individual steps

New users get sensible defaults automatically via `cargo install` — no config files needed.

### `minion setup`

```bash
minion setup
```

Interactive setup wizard — checks requirements, configures API keys, and optionally sets up Slack bot credentials. Saves config to `~/.minion/config.toml`.

### `minion slack start` (requires `--features slack`)

```bash
minion slack start [--port 9000]
```

Starts the Slack bot server. Reads config from `~/.minion/config.toml` or environment variables.

## Workflow YAML Format

```yaml
name: my-workflow
version: 1
description: "What this workflow does"

# Config is optional — sensible defaults are embedded in the binary.
# Only specify overrides for what's different from defaults.
config:
  global:
    timeout: 600s           # Override default 300s
  chat:
    temperature: 0.1        # Override default 0.2

steps:
  - name: get_info
    type: cmd
    run: "gh issue view {{ target }} --json title,body"

  - name: analyze
    type: chat
    prompt: |
      Analyze this issue and suggest a fix:
      {{ steps.get_info.stdout }}

  - name: report
    type: cmd
    run: "echo 'Analysis complete'"
```

## Step Types

| Type | Description |
|------|-------------|
| `cmd` | Execute a shell command |
| `agent` | Invoke Claude Code CLI |
| `chat` | Direct Anthropic API call |
| `gate` | Evaluate a condition, control flow |
| `repeat` | Run a scope repeatedly (retry loop) |
| `map` | Run a scope once per item in a list |
| `parallel` | Run nested steps concurrently |
| `call` | Invoke a scope once |

## Template Variables

| Variable | Description |
|----------|-------------|
| `{{ target }}` | Target argument passed after `--` |
| `{{ steps.<name>.stdout }}` | stdout of a cmd step |
| `{{ steps.<name>.stderr }}` | stderr of a cmd step |
| `{{ steps.<name>.exit_code }}` | Exit code of a cmd step |
| `{{ steps.<name>.response }}` | Response from a chat/agent step |
| `{{ scope.value }}` | Current item in a map/repeat scope |
| `{{ scope.index }}` | Current iteration index (0-based) |
| `{{ args.<key> }}` | Variable set via `--var KEY=VALUE` |
| `{{ prompts.<name> }}` | Load a prompt from the prompt registry |

## Example Output

```
▶ code-review
  🔒 Sandbox mode: FullWorkflow
  🐳 Creating Docker sandbox container…
  🔒 Sandbox ready — container 1.3s, copy 12.4s, git 98.7s (total 112.4s)
  ✓ get_diff (3.2s)
  ✓ changed_files (1.8s)
  ✓ check_files (0.0s)
  ✓ file_reviews (45.3s)    ← map scope: reviews each file with language-specific criteria
  ✓ summary (28.1s)          ← chat step: synthesizes all reviews into a report
  ✓ post_comment (2.1s)
  ✓ report (0.3s)
  📦 Copying results from sandbox…
  🗑️  Sandbox destroyed

✓ Done — 7 steps in 193.2s
```

## Slack Bot Integration

![Slack Bot Interaction Flow](docs/architecture-slack-bot.jpg)

Trigger Minion workflows from Slack by mentioning the bot:

```
@YourBot review pr #42
@YourBot fix issue https://github.com/org/repo/issues/10
@YourBot security audit myproject
@YourBot generate docs myproject
```

### Step-by-step Setup

#### 1. Create a Slack App

1. Go to **https://api.slack.com/apps** → **Create New App** → **From Scratch**
2. Name it (e.g., "Minion Engine") and select your workspace

#### 2. Configure Permissions

Go to **OAuth & Permissions** → **Bot Token Scopes** and add:

| Scope | Purpose |
|-------|---------|
| `app_mentions:read` | Detect `@YourBot` mentions |
| `chat:write` | Post replies in channels |
| `channels:history` | Read channel messages |
| `channels:read` | List channels |

Click **Install to Workspace** and copy the **Bot User OAuth Token** (`xoxb-...`).

#### 3. Configure Event Subscriptions

Go to **Event Subscriptions** → toggle **ON**.

Under **Subscribe to bot events**, add:
- `app_mention`

For the **Request URL**, you need a public endpoint. Use **ngrok** for development:

```bash
# Install ngrok (https://ngrok.com)
brew install ngrok   # macOS
# or download from https://ngrok.com/download

# Start a tunnel to port 9000
ngrok http 9000
```

Copy the ngrok URL (e.g., `https://abc123.ngrok-free.app`) and set the Request URL to:
```
https://abc123.ngrok-free.app/slack/events
```

Wait for the **"Verified"** checkmark, then click **Save Changes**.

> **Tip:** Use `ngrok http 9000 --domain your-name.ngrok-free.app` for a stable domain (free ngrok accounts get one static domain).

#### 4. Get the Signing Secret

Go to **Basic Information** → **App Credentials** → copy the **Signing Secret**.

#### 5. Install and Configure Minion

```bash
# Install with Slack support
cargo install minion-engine --features slack

# Run setup wizard — it will ask for your Slack tokens
minion setup
```

The setup wizard saves your config to `~/.minion/config.toml`:
```toml
[core]
anthropic_api_key = "sk-ant-..."
workflows_dir = "./workflows"

[slack]
bot_token = "xoxb-..."
signing_secret = "2d91c..."
port = 9000
```

Or set environment variables directly:
```bash
export SLACK_BOT_TOKEN="xoxb-..."
export SLACK_SIGNING_SECRET="2d91c..."
```

#### 6. Start the Bot

```bash
# Make sure ngrok is running: ngrok http 9000
minion slack start
```

#### 7. Invite the Bot

In your Slack channel:
```
/invite @YourBot
```

Then mention it:
```
@YourBot review pr #42
```

### Supported Commands

| Slack Message | Workflow |
|---------------|----------|
| `@bot fix issue #10` or `@bot fix issue <url>` | `fix-issue.yaml` |
| `@bot review pr #42` or `@bot review pr <url>` | `code-review.yaml` |
| `@bot security audit <target>` | `security-audit.yaml` |
| `@bot generate docs <target>` | `generate-docs.yaml` |
| `@bot fix ci <pr-url>` | `fix-ci.yaml` |

## Project Structure

```
src/
  cli/          # CLI commands (execute, validate, list, init, inspect, setup)
  engine/       # Core engine — step execution, context, templates
  workflow/     # YAML parsing, validation
  steps/        # Step executors (cmd, agent, chat, gate, repeat, map, parallel)
  sandbox/      # Docker sandbox management
  prompts/      # Stack detection and prompt registry
  config/       # 5-layer config resolution (embedded → user → project → workflow → step)
  slack/        # Slack bot integration (optional, --features slack)
  plugins/      # Dynamic plugin system
workflows/      # Example workflow YAML files
prompts/        # Language-specific prompt templates
```

## License

MIT
