# Development Guide — Minion Engine

_Generated: 2026-03-13_

---

## Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust | 1.75+ | Core language (use `rustup`) |
| Claude CLI | Latest | Required for `agent` steps |
| gh CLI | Latest | GitHub integration (auto-authenticated) |
| Docker Desktop | 4.40+ | Sandbox isolation (on by default) |
| ANTHROPIC_API_KEY | - | Required for `chat` and `map` steps |

## Quick Start

```bash
# Clone
git clone https://github.com/allanbrunobr/minion-engine.git
cd minion-engine

# Build
cargo build --release

# Verify
./target/release/minion --version

# Run a workflow
minion execute workflows/hello-world.yaml
```

## Install Methods

### Via crates.io (recommended)
```bash
cargo install minion-engine
minion --version
```

### Build from source
```bash
cargo build --release
# Binary at: ./target/release/minion
```

## Development Commands

| Command | Purpose |
|---------|---------|
| `cargo build` | Debug build |
| `cargo build --release` | Release build |
| `cargo test` | Run all tests (unit + integration) |
| `cargo test -- --nocapture` | Run tests with output |
| `cargo clippy` | Lint check (default rules) |
| `cargo fmt` | Format code (default rules) |
| `cargo doc --open` | Generate and open docs |

## CLI Commands

```bash
# Execute a workflow
minion execute <workflow.yaml> [--target <dir>] [--sandbox|--no-sandbox] [--verbose] [--json]

# Validate a workflow (no execution)
minion validate <workflow.yaml>

# List available workflows
minion list

# Initialize a new workflow from template
minion init <name> [--template <template-name>]

# Inspect workflow structure
minion inspect <workflow.yaml>
```

## Testing

### Unit Tests
All unit tests are **inline** in `#[cfg(test)] mod tests` at the bottom of each source file. 25 of 41 files have tests.

```bash
# Run all tests
cargo test

# Run tests for a specific module
cargo test --lib steps::agent
cargo test --lib engine::context

# Run integration tests only
cargo test --test integration
```

### Test Helpers
- `tempfile::tempdir()` for filesystem tests
- `wiremock` for HTTP endpoint mocking
- Helper functions (`make_step()`, `make_context()`) defined in each test module

## Project Structure

```
src/
├── main.rs              # Entrypoint
├── lib.rs               # Module exports
├── engine/              # Core orchestrator
├── steps/               # 10 step executors
├── workflow/            # Schema, parser, validator
├── cli/                 # CLI interface
├── sandbox/             # Docker isolation
├── config/              # 4-layer config merge
├── events/              # Event bus
├── plugins/             # Dynamic plugin loading
├── claude/              # Session management
├── error.rs             # Error types
└── control_flow.rs      # Flow control
```

## Adding a New Step Type

1. Create `src/steps/new_type.rs` with executor struct
2. Implement `StepExecutor` trait (and optionally `SandboxAwareExecutor`)
3. Add variant to `StepType` enum in `workflow/schema.rs`
4. Register in `Engine::dispatch_step()` in `engine/mod.rs`
5. Export from `src/steps/mod.rs`
6. Add tests in `#[cfg(test)]` module
7. Update `workflow/validator.rs` if new validation rules needed

## Creating Workflows

Workflows are YAML files in `workflows/`. Key structure:

```yaml
name: my-workflow
version: 1
description: "What this workflow does"

config:
  global:
    timeout: 300s
  agent:
    command: claude
    permissions: skip
    model: claude-sonnet-4-20250514

steps:
  - name: step_name
    type: cmd|agent|chat|gate|repeat|map|parallel|call|template|script
    run: "command"          # for cmd
    prompt: "instruction"   # for agent/chat
    condition: "{{ expr }}" # for gate
```

## Environment Variables

| Variable | Required | Purpose |
|----------|----------|---------|
| `ANTHROPIC_API_KEY` | For chat/map steps | Anthropic API authentication |
| `GH_TOKEN` | Auto-detected | GitHub API access (from `gh auth`) |
| `MINION_LOG` | Optional | Set log level (e.g., `debug`, `trace`) |

## Release Process

1. Update version in `Cargo.toml`
2. Commit and push
3. Create git tag: `git tag v0.x.x`
4. Push tag: `git push origin v0.x.x`
5. GitHub Actions builds binaries for 5 targets and creates release
6. Publish to crates.io: `cargo publish`

## Code Conventions

- **Formatting**: Default `rustfmt` (no custom config)
- **Linting**: Default `clippy` (no custom config)
- **Naming**: snake_case files/functions, PascalCase types
- **Tests**: Inline `#[cfg(test)]` modules, `#[tokio::test]` for async
- **Errors**: `thiserror` for enums, `anyhow` for propagation
- **Visibility**: `pub(crate)` for internal, `pub` for API surface
