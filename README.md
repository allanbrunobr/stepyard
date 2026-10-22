# Stepyard

[![Check](https://github.com/allanbrunobr/stepyard/actions/workflows/check.yml/badge.svg)](https://github.com/allanbrunobr/stepyard/actions/workflows/check.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Status: alpha](https://img.shields.io/badge/status-alpha-orange.svg)

**You define the steps. The agent executes them.**

Stepyard runs AI coding workflows that you write as YAML. Each step is explicit:
run a command, ask a model, check a condition, retry until the tests pass. The
agent does not decide what to do next. It follows the workflow.

![How Stepyard works: an issue starts a workflow, the project is copied into a separate Docker container, an agent fixes the code there until the tests pass, a pull request is opened, the changed files are copied back, and the container is removed](docs/assets/how-it-works.gif)

<sub>How a sandboxed run works. The API key stays on your machine; the container
only gets a dummy token that goes through a local proxy. With
`--sandbox-runtime docker` (or `podman`), the project is copied into the
container, `cmd` and `agent` steps run there, and the changed files are copied
back at the end. See [Sandbox and API key protection](#sandbox-and-api-key-protection).
[MP4 version](docs/assets/how-it-works.mp4).</sub>

**A real run in the terminal:**

![Stepyard runs a test, lets Claude Code fix the bug, and repeats until the tests pass](docs/assets/demo.gif)

<sub>A real run of [`scripts/demo/project/fix-tests.yaml`](scripts/demo/project/fix-tests.yaml):
a `repeat` loop runs the tests, a `gate` checks the result, and an `agent` step
(Claude Code) fixes the code until the tests pass. This run uses the `local`
runtime (no container). Only the agent's waiting time is shortened. See [scripts/demo](scripts/demo/README.md).</sub>

```mermaid
flowchart LR
    Y[workflow.yaml] --> CLI[stepyard CLI]
    CLI --> E[Engine v2 / v1]
    E --> L[(Session log<br/>PostgreSQL or SQLite)]
    E --> C[cmd steps<br/>host or container]
    E --> A[agent steps<br/>Claude Code CLI]
    E --> M[chat steps<br/>model API]
    C --> G[git / gh]
```

```bash
stepyard execute workflows/code-review.yaml -- 42
```

> Gets the diff of PR #42, reviews each changed file with rules for its
> language, and posts one summary comment on the PR.

## Project status

Stepyard is **alpha** software. The CLI and the workflow format can change
between versions. Prebuilt binaries for macOS and Linux are on the
[releases page](https://github.com/allanbrunobr/stepyard/releases); you can also
install from source.

I developed Stepyard in a private repository and moved it here as a fresh
history when I made it public, so the commit log here is short. The project
has been in development since March 2026 (about 300 commits before the move).

The CLI has two engines:

| Engine | Status | Use it when |
|--------|--------|-------------|
| `v2` (default) | Session log in PostgreSQL or SQLite, resumes after a crash | Most workflows, on the host or in a Docker/Podman container |
| `v1` (`--engine v1`) | Legacy | You need `{{ stack }}` / `{{ prompts.* }}`, `--dry-run`, `--repo`, or dashboard events |

Read [Known limitations of v2](#known-limitations-of-v2) before you depend on the default engine.

## Why?

AI coding tools can surprise you. They edit the wrong file, add a dependency
you did not ask for, or drift away from the task. This happens because the
agent chooses the next action.

Stepyard reverses that: the workflow chooses the next action, and the model only
does the work inside each step. A run is repeatable and you can read it in a
diff.

## Quick start

### 1. Requirements

| Requirement | Why | How to get it |
|-------------|-----|---------------|
| Rust toolchain (stable) | Only to build from source | [rustup.rs](https://rustup.rs) |
| git | The v2 engine must run inside a git repository with at least one commit | your package manager |
| A model API key | `chat` steps | for example `export ANTHROPIC_API_KEY=sk-ant-...` |
| [Claude Code CLI](https://docs.anthropic.com/en/docs/claude-code) (`claude`) | `agent` steps | `npm install -g @anthropic-ai/claude-code` |
| [GitHub CLI](https://cli.github.com) (`gh`) | Workflows that read PRs and issues | `gh auth login` |
| Docker or Podman | Optional: container sandbox | [docker.com](https://www.docker.com/products/docker-desktop/) / [podman.io](https://podman.io/) |
| PostgreSQL | Only for the default (`postgres`) build | see step 2 |

### 2. Install

**Prebuilt binary (SQLite build, macOS or Linux):** download it from the
[latest release](https://github.com/allanbrunobr/stepyard/releases/latest), for example:

```bash
curl -L https://github.com/allanbrunobr/stepyard/releases/latest/download/stepyard-macos-aarch64 -o stepyard
chmod +x stepyard && sudo mv stepyard /usr/local/bin/
```

**From source**, pick one session store:

```bash
# A) SQLite: no database server. Sessions are stored in .stepyard/sessions.db.
#    Steps run on the host by default.
cargo install --locked --git https://github.com/allanbrunobr/stepyard \
  --no-default-features --features sqlite

# B) PostgreSQL (default build): needed for `stepyard session` and the dashboard.
#    Steps run in Docker by default.
cargo install --locked --git https://github.com/allanbrunobr/stepyard
docker run -d --name stepyard-db -p 5432:5432 \
  -e POSTGRES_PASSWORD=stepyard -e POSTGRES_DB=stepyard postgres:16-alpine
export DATABASE_URL=postgres://postgres:stepyard@localhost:5432/stepyard
```

Then check your environment:

```bash
stepyard setup      # interactive: checks tools and stores API keys in ~/.stepyard/config.toml
stepyard --version
```

### 3. Run your first workflow

The bundled workflows and prompts are in this repository. They are not
installed with the binary yet. Copy the ones you need into your project:

```bash
git clone https://github.com/allanbrunobr/stepyard ~/stepyard
cd /path/to/your-project          # a git repository with at least one commit
cp -r ~/stepyard/workflows ~/stepyard/prompts .

stepyard execute workflows/hello-world-cmd.yaml
stepyard execute workflows/code-review.yaml --sandbox-runtime local -- 42
```

Output:

```
▶ hello-world-cmd
  ✓ greet (0.0s)
  ✓ show_pwd (0.0s)
  ✓ done (0.0s)

✓ Done — 3 steps in 0.0s
```

The v2 engine prints each step, including the steps inside `repeat`, `map` and
`call` loops and the decision of each `gate`. Add `--verbose` to also print the
output of `cmd` steps. The full output of every step is in
`.stepyard/logs/<session_id>.jsonl`.

To start a workflow of your own, run `stepyard init my-workflow --template blank`.

## Bundled workflows

| Workflow | What it does | Engine |
|----------|--------------|--------|
| `code-review` | Reviews each changed file of a PR with rules for its language and posts a summary comment | v1, v2 |
| `fix-issue` | Reads a GitHub issue, plans, implements, lints, tests, and opens a PR | v1, v2 |
| `security-audit` | Scans the codebase for OWASP-style problems | v1, v2 |
| `generate-docs` | Writes Markdown documentation for source files | v1, v2 |
| `refactor`, `flaky-test-fix`, `weekly-report` | Refactors from a description, finds flaky tests, summarizes a week of git activity | v1, v2 |
| `fix-ci`, `fix-test` | Detects the stack, runs lint or tests, and fixes failures in a loop | **v1 only** |
| `hello-world-cmd`, `hello-world`, `simple-test` | Smoke tests | v1, v2 |

Each file starts with a usage comment. You can change them or write your own.

## A minimal workflow

```yaml
name: explain-issue
description: "Summarize a GitHub issue"

steps:
  - name: issue
    type: cmd
    run: "gh issue view {{ target }} --json title,body"

  - name: explain
    type: chat
    prompt: |
      Explain this issue in three bullet points:
      {{ steps.issue.stdout }}

  - name: print
    type: cmd
    run: |
      cat <<'EOF'
      {{ steps.explain.response }}
      EOF
```

```bash
stepyard execute explain-issue.yaml -- 123
```

### Step types

| Type | What it does |
|------|--------------|
| `cmd` | Runs a shell command |
| `agent` | Runs the Claude Code CLI with a prompt |
| `chat` | Calls a model API directly (Anthropic by default; OpenAI, Gemini, Ollama, and other [rig-core](https://github.com/0xPlaygrounds/rig) providers through `provider:`) |
| `gate` | Evaluates a condition and continues, skips, breaks, or fails |
| `repeat` | Runs a scope again until a gate breaks the loop (retry) |
| `map` | Runs a scope once for each item of a list |
| `parallel` | Runs nested steps at the same time |
| `call` | Runs a scope one time |
| `template` | Renders a prompt template file |
| `script` | Runs an inline [Rhai](https://rhai.rs) script |

Full reference: [`docs/STEP-TYPES.md`](docs/STEP-TYPES.md) and [`docs/YAML-SPEC.md`](docs/YAML-SPEC.md).

### Template variables

| Variable | Meaning | v1 | v2 |
|----------|---------|----|----|
| `{{ target }}` | The argument after `--` | ✓ | ✓ |
| `{{ steps.<name>.stdout }}` / `.stderr` / `.exit_code` | Output of an earlier step | ✓ | ✓ |
| `{{ steps.<name>.response }}` | Reply of a `chat` or `agent` step (same as `.stdout` in v2) | ✓ | ✓ |
| `{{ scope.value }}` / `{{ scope.index }}` | Current item and index in `map` or `repeat` | ✓ | ✓ |
| `{{ args.KEY }}` | Value from `--var KEY=VALUE` (v2 also accepts `{{ vars.KEY }}`) | ✓ | ✓ |
| `{{KEY}}` (no spaces) | Replaced by `--var KEY=VALUE` when the file loads | ✓ | ✓ |
| `{{ stack.* }}`, `{{ prompts.<name> }}` | Detected project stack and prompt registry | ✓ | ✗ |

`--var` keys must be uppercase (`[A-Z_][A-Z0-9_]*`), for example `--var MODE=strict`.

## Sandbox and API key protection

With a container runtime, each run gets a fresh container (default image
`stepyard-sandbox:latest`, built on the first run):

1. Before the first `cmd` or `agent` step, the project (the current directory)
   is copied into `/workspace` in the container. `target/`, `node_modules/`,
   `.stepyard/`, `.venv/`, and `__pycache__/` are not copied.
2. `cmd` steps run in `/workspace`. `agent` steps run the Claude CLI in
   `/workspace` as the non-root user `stepyard`, with only the step's
   environment (not your shell environment).
3. When the workflow ends (also when a step fails), the files that changed are
   copied back to the project and the container is removed. Changes are found
   with `git status`, so files that your `.gitignore` excludes are not copied
   back. The CLI prints the list of changed files.

A cancelled run (Ctrl-C) removes the container without copying changes back.

The real `ANTHROPIC_API_KEY` does not go into the container:

- The container gets a per-session dummy token as `ANTHROPIC_API_KEY` and
  `ANTHROPIC_BASE_URL=http://host.docker.internal:<port>`.
- A proxy on the host checks that token, sends only `/v1/*` paths to
  `api.anthropic.com`, and adds the real key.
- If the proxy cannot start, the workflow stops. There is no fallback that
  gives the real key to the container.
- `agent` steps in the container use the same proxy. With no
  `ANTHROPIC_API_KEY` on the host, the Claude CLI in the container is not
  logged in (a Claude subscription login on the host is not shared).
- With a remote Docker daemon, set `STEPYARD_PROXY_PORT` and forward that port
  with `ssh -R` so the container can reach the proxy. See
  [Remote Docker host](docs/DOCKER-SANDBOX.md#remote-docker-host).
- `chat` steps always run on the host.

```mermaid
sequenceDiagram
    participant S as stepyard
    participant P as API proxy (host)
    participant C as Container
    participant A as api.anthropic.com
    S->>P: start proxy with the real API key
    S->>C: create container (dummy token, ANTHROPIC_BASE_URL=proxy)
    C->>P: /v1/* request + dummy token
    P->>A: same request + real API key
    A-->>C: response (through the proxy)
    S->>C: remove container after the run
```

Select the runtime with `--sandbox-runtime docker|podman|local`, the
`STEPYARD_SANDBOX` environment variable, or `--no-sandbox` (same as `local`).
The default is `docker` for the PostgreSQL build and `local` for the SQLite
build.

Details: [`docs/DOCKER-SANDBOX.md`](docs/DOCKER-SANDBOX.md).

## Known limitations of v2

| Feature | Status in v2 |
|---------|--------------|
| `{{ stack.* }}`, `{{ prompts.* }}` | Not rendered. Use `--engine v1` |
| `--dry-run` | Rejected. Use `--engine v1` or `stepyard inspect` |
| `--quiet`, `--resume`, `--repo` | Ignored (`--verbose` works: it prints cmd output) |
| `parallel:` on `map` | Ignored. Items run one after another (v1 also runs them one after another by default) |
| `config.events.dashboard` | Not sent. Dashboard events work only with v1 |
| `--timeout` | Has no effect in either engine. Set `config.global.timeout` or a step `timeout:` |

## CLI overview

| Command | Purpose |
|---------|---------|
| `stepyard execute <file> [flags] -- [target]` | Run a workflow |
| `stepyard validate <file>` | Check a workflow without running it |
| `stepyard inspect <file>` | Show config layers, scopes, and the step graph |
| `stepyard list` | List workflows in `.`, `./workflows/`, and `~/.stepyard/workflows/` |
| `stepyard init <name> [--template blank\|fix-issue\|code-review\|security-audit]` | Create a workflow from a template |
| `stepyard config show\|init\|set\|path` | Show or change default settings |
| `stepyard setup` | Interactive setup wizard |
| `stepyard session list` | List stored sessions (PostgreSQL build) |
| `stepyard exec [--interactive] <session-id> [-- cmd]` | Run a command in the sandbox of a live session |
| `stepyard remote exec\|status\|logs` | Send workflows to a remote engine. See [`docs/REMOTE.md`](docs/REMOTE.md) |
| `stepyard slack start` | Start the Slack bot (`--features slack`). See [`docs/SLACK.md`](docs/SLACK.md) |
| `stepyard version` | Show the version |

Main `execute` flags:

| Flag | Meaning |
|------|---------|
| `--engine v1\|v2` | Select the engine (default `v2`) |
| `--var KEY=VALUE` | Set a template variable (repeatable, uppercase key) |
| `--sandbox-runtime docker\|podman\|local` | Select where `cmd` steps run |
| `--no-sandbox` | Run on the host |
| `--branch-strategy head\|merge-to-head\|named-branch:<name>` | Git worktree strategy for the v2 session |
| `--no-file-logs` | Do not write `.stepyard/logs/<session_id>.jsonl` (or set `STEPYARD_NO_FILE_LOGS=1`) |
| `--json` | Print the result as JSON |
| `--verbose` | Also print the output of `cmd` steps |
| `--dry-run`, `--quiet`, `--resume <step>`, `--repo owner/repo` | v1 only |

Run `stepyard <command> --help` for every option.

## Configuration

Defaults are compiled into the binary, so a workflow does not need a `config:`
block. Settings are merged in this order (later wins):

1. Embedded defaults (for example `chat.model: claude-sonnet-4-20250514`, `chat.temperature: 0.2`)
2. `~/.stepyard/defaults.yaml` (`stepyard config init`)
3. `.stepyard/config.yaml` in the project
4. The `config:` block of the workflow
5. The `config:` block of a step

```bash
stepyard config set chat.model claude-opus-4-20250514
stepyard config set global.timeout 600s
```

See [`docs/CONFIG.md`](docs/CONFIG.md) for the layers inside one workflow file.

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `engine requires PostgreSQL backend: DATABASE_URL env var is not set` | Export `DATABASE_URL`, or install the SQLite build |
| `fatal: not a git repository` | Run the v2 engine from inside a git repository |
| `Workflow file not found` | Give the path to the YAML file. Stepyard also looks in `~/.stepyard/workflows/<name>`. The bundled workflows are not installed with the binary |
| `--var` is rejected | Use an uppercase key: `--var MODE=strict` |
| `render cmd ... Variable ... not found` | The step uses a variable that v2 does not provide (see the tables above), or a step name is wrong |
| `429 Too Many Requests` | Fewer concurrent model calls: reduce `parallel` steps or add retries |
| First Docker run is slow | The `stepyard-sandbox:latest` image builds once (about 2 minutes) and is cached |
| `gh` fails inside the sandbox | Run `gh auth login` on the host first. The token is passed as `GH_TOKEN` |

## Dashboard (optional)

The repository has a TypeScript dashboard for workflow runs, costs, and remote
dispatch (`packages/api`, Express + PostgreSQL; `packages/web`, React + Vite).

```bash
cp .env.example .env    # replace every change-me value
npm install
npm run dev             # Docker Compose: Postgres + API + web + LiteLLM
```

Only the v1 engine sends events to the dashboard. See [`docs/REMOTE.md`](docs/REMOTE.md).

## Documentation

- [`docs/YAML-SPEC.md`](docs/YAML-SPEC.md): workflow format
- [`docs/STEP-TYPES.md`](docs/STEP-TYPES.md): every step type, with examples
- [`docs/CONFIG.md`](docs/CONFIG.md): configuration layers
- [`docs/DOCKER-SANDBOX.md`](docs/DOCKER-SANDBOX.md): sandbox modes, security, proxy
- [`docs/EXAMPLES.md`](docs/EXAMPLES.md): example workflows and patterns
- [`docs/REMOTE.md`](docs/REMOTE.md): remote execution and dashboard dispatch
- [`docs/SLACK.md`](docs/SLACK.md): Slack bot setup
- [`ARCHITECTURE.md`](ARCHITECTURE.md): how the code is organized, invariants, and what comes next

## Project layout

```
src/                 CLI binary (stepyard) and the legacy v1 engine
crates/
  stepyard-core/     Shared contracts (Event, EngineError, templates)
  stepyard-session/  Append-only session log (PostgreSQL / SQLite)
  stepyard-sandbox-orchestrator/  Sandbox lifecycle (Docker, Podman, local shell)
  stepyard-harness/  v2 workflow engine
packages/api, web    Dashboard
workflows/           Bundled workflows
prompts/             Prompt templates per language and task
docs/                Reference documentation
```

## Contributing

Issues and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md)
for build, test, and security-test conventions.

## License

[MIT](LICENSE)
