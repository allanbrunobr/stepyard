# Contributing to Stepyard

Thank you for helping. This guide covers how to build, test, and submit changes.

## Repository layout

| Path | Contents |
|------|----------|
| `src/` | CLI binary (`stepyard`) and the legacy v1 engine |
| `crates/stepyard-core` | Shared contracts: `Event`, `EngineError`, template helpers |
| `crates/stepyard-session` | Append-only session log (PostgreSQL / SQLite) |
| `crates/stepyard-sandbox-orchestrator` | Sandbox lifecycle (Docker, Podman, local shell) |
| `crates/stepyard-harness` | v2 workflow engine |
| `packages/api`, `packages/web` | Dashboard (Express + React) |
| `workflows/`, `prompts/` | Bundled workflows and prompt templates |
| `xtask/` | Internal tooling (audits) |

## Build and test

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# SQLite build (no PostgreSQL)
cargo check -p stepyard-engine --no-default-features --features sqlite
cargo test  -p stepyard-engine --test cli_lite_smoke --no-default-features --features sqlite

# Dashboard
npm ci
npm run build --workspace @stepyard/api
npm run build --workspace @stepyard/web
npm run test  --workspace @stepyard/api
```

CI (`.github/workflows/check.yml`) runs these commands. Run them before you
open a pull request.

### Tests with external services

Some tests need a service. They skip when the service is not configured:

| Variable | Enables |
|----------|---------|
| `DATABASE_URL`, `STEPYARD_HARNESS_DATABASE_URL`, `STEPYARD_SESSION_DATABASE_URL` | PostgreSQL integration tests for the CLI, the harness, and the session crate (CI sets all three) |
| `STEPYARD_TEST_DOCKER=1` | Docker sandbox and injection tests |

Example:

```bash
docker run -d --name stepyard-test-db -p 5432:5432 \
  -e POSTGRES_PASSWORD=stepyard -e POSTGRES_DB=stepyard postgres:16-alpine
export DATABASE_URL=postgres://postgres:stepyard@localhost:5432/stepyard
export STEPYARD_HARNESS_DATABASE_URL=$DATABASE_URL STEPYARD_SESSION_DATABASE_URL=$DATABASE_URL
STEPYARD_TEST_DOCKER=1 cargo test --workspace
```

## Coding conventions

- Rust 2021, `cargo fmt` formatting, no clippy warnings.
- Library crates use typed errors (`thiserror`). Keep `anyhow` at the binary boundary.
- Start subprocesses with `tokio::process::Command` and argv arguments. Do
  not join strings into a shell command unless the workflow explicitly runs
  `sh -c`.
- Never log secrets or environment variable values.
- Tests: name them by behavior, keep them near the crate they test, and use
  deterministic time instead of real sleeps.

### Audit gates

CI also runs blocking audits:

```bash
scripts/audit-patterns.sh          # secrets in logs, shell joining, real sleeps in tests, ...
cargo xtask audit-emit-before-io   # every side effect is recorded in the session log first
```

If a low-level call site must be exempt from the emit-before-IO audit, add an
`audit: emit-before-io exempt - reason: ...` comment that explains why.

## Commits and pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org/) with a
  scope: `feat(harness): ...`, `fix(core): ...`, `docs(readme): ...`.
- A pull request should include a short behavior summary, the linked issue,
  migration or configuration notes, and the validation commands you ran.
- Add screenshots only for dashboard changes.

## Reporting security problems

Do not open a public issue for a vulnerability. Contact the maintainer
privately through GitHub first.
