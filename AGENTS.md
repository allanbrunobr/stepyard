# Repository Guidelines

## Project Structure & Module Organization

This workspace is a Rust CLI and library monorepo with a TypeScript dashboard. Rust source lives in `src/`; reusable crates are under `crates/` (`stepyard-core`, `stepyard-session`, `stepyard-sandbox-orchestrator`, `stepyard-harness`). Rust integration tests live in `tests/` and crate-specific `*/tests/` directories. Dashboard/API code lives in `packages/web` and `packages/api`. Workflow examples are in `workflows/`; docs and planning artifacts are in `docs/`, `_bmad-output/`, `.hive/`, and root Markdown files. Internal tooling lives in `xtask/`.

## Build, Test, and Development Commands

Run commands from the repository root unless noted.

- `cargo build --workspace`: build the Rust CLI and crates.
- `cargo test --workspace`: run all Rust unit and integration tests. Some DB/Docker tests skip unless required env vars are set.
- `cargo clippy --workspace --all-targets -- -D warnings`: enforce Rust lint cleanliness before merge.
- `cargo run -- execute workflows/hello-world-cmd.yaml --engine v2`: run a sample workflow through the harness path.
- `npm run dev`: start the Docker Compose development stack.
- `npm run dev:api` / `npm run dev:web`: run API or web dashboard separately.
- `npm run build --workspace @stepyard/api` and `npm run build --workspace @stepyard/web`: type-check/build TypeScript packages.

## Coding Style & Naming Conventions

Rust uses edition 2021, four-space indentation, `snake_case` modules/functions, `PascalCase` types, and `thiserror` for public library errors. Prefer typed errors in crates; keep `anyhow` at binary/application boundaries. Use `tokio::process::Command` with argv arguments for subprocesses; do not introduce shell interpolation unless the workflow explicitly chooses `sh -c`. TypeScript uses standard `tsc` checks, React components in `PascalCase`, and hooks named `use*`.

## Testing Guidelines

Place focused Rust tests near the crate they exercise, e.g. `crates/stepyard-harness/tests/step_timeout.rs`. Name tests by behavior, not implementation. For async Rust tests, prefer deterministic time; avoid unbounded sleeps. Out-of-process tests using `assert_cmd` should set explicit timeouts. Tests needing PostgreSQL or Docker should skip cleanly when env or daemon prerequisites are missing.

## Commit & Pull Request Guidelines

Use Conventional Commits with scopes, matching project history: `feat(harness): ...`, `fix(core): Story 1.1 ...`, `chore(bmad): ...`. Reference BMAD story/feature IDs when applicable. PRs should include a concise behavior summary, linked issue/story, migration or configuration notes, and exact validation commands run. Include screenshots only for dashboard/UI changes.

## Security & Configuration Tips

Do not log secrets or environment variable values. Keep `.env` files local; use `.env.example` for placeholders. Docker, PostgreSQL, GitHub CLI tokens, and Anthropic/OpenAI keys should be treated as optional runtime dependencies and validated before execution.