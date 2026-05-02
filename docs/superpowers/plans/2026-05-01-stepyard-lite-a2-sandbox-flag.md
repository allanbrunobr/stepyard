# Stepyard Lite PR A2 — CLI Sandbox Runtime Flag + LocalShell Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a value-typed `--sandbox-runtime <docker|local>` CLI flag (with `STEPYARD_SANDBOX` env fallback) that selects between `DockerLifecycle` and `LocalShellLifecycle` at run time, defaulting to `local` on the `sqlite` build profile and `docker` on `postgres`. The existing boolean `--no-sandbox` flag is kept as a legacy alias.

**Architecture:** A small, self-contained CLI surface change. The wiring point already exists at `src/cli/commands.rs:237` (the `if sandbox_mode == SandboxMode::Disabled { LocalShell } else { Docker }` branch). This PR replaces that single decision with a dedicated `SandboxRuntime` enum resolved by precedence (CLI flag > env > profile default), threaded through to the existing branch. No new lifecycle code: `LocalShellLifecycle` is already exported from `stepyard-sandbox-orchestrator`.

**Tech Stack:** Rust 2021, clap 4 (`ValueEnum` derive), tokio, existing `stepyard-sandbox-orchestrator::{DockerLifecycle, LocalShellLifecycle, SandboxLifecycle}`.

**Spec reference:** `docs/superpowers/specs/2026-05-01-stepyard-lite-design.md` §6.

**Prerequisites:**
- PR A1 merged (workspace features `postgres`/`sqlite` available; `cfg(feature = "sqlite")` resolves).
- Familiarity with clap `ValueEnum` and clap argument groups.

---

## File Structure

**`src/sandbox/`:**
- Create: `src/sandbox/runtime.rs` — `SandboxRuntime` enum + `resolve_runtime()` precedence logic + unit tests.
- Modify: `src/sandbox/mod.rs` — re-export `SandboxRuntime`, `resolve_runtime`.

**`src/cli/`:**
- Modify: `src/cli/commands.rs` — new `--sandbox-runtime` arg on `ExecuteArgs`; `--no-sandbox` flag is kept; new resolution call right before lifecycle construction; emits a `tracing::warn!` if both old and new flags are passed.
- Modify: `src/cli/mod.rs` — help text update.

**v2 wiring:**
- Modify: `src/cli/commands.rs::execute_v2` — replace the `if sandbox_mode == SandboxMode::Disabled` arm with a match on the resolved `SandboxRuntime`.

**Tests:**
- Create: `tests/cli_sandbox_runtime.rs` — end-to-end: runs `stepyard run` against a trivial workflow under both runtimes, asserts no Docker calls in `local` mode.

**Docs:**
- Modify: `README.md` (or whatever runs as the canonical CLI doc) — short paragraph on `--sandbox-runtime` + the `STEPYARD_SANDBOX` env var.

---

## Task 1: Branch + baseline gates

**Files:** none (tooling only)

- [ ] **Step 1: Create the PR branch off latest main**

Run:
```bash
git fetch origin main
git checkout -b feat/pr-a2-sandbox-runtime-flag origin/main
```
Expected: `Switched to a new branch 'feat/pr-a2-sandbox-runtime-flag'`.

- [ ] **Step 2: Confirm A1 has landed**

Run: `git log --oneline -5 | grep -i "stepyard.lite\|EventStore\|sqlite"`
Expected: at least one A1-related commit visible.

If not, STOP — A2 depends on the workspace features `postgres` / `sqlite` from A1.

- [ ] **Step 3: Capture baseline gates so post-PR diffs are unambiguous**

Run:
```bash
cargo fmt --check
cargo clippy --workspace --all-targets --no-default-features --features postgres -- -D warnings 2>&1 | tail -5
```
Expected: both clean. If the clippy lane already has known baseline warnings, capture the count to a scratch file so the diff is comparable later.

---

## Task 2: Define `SandboxRuntime` enum (TDD)

**Files:**
- Create: `src/sandbox/runtime.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Create `src/sandbox/runtime.rs`:

```rust
//! Resolves which `SandboxLifecycle` impl the CLI hands to the engine:
//! `DockerLifecycle` (production, postgres profile default) or
//! `LocalShellLifecycle` (Lite mode, sqlite profile default).
//!
//! This is orthogonal to `SandboxMode` (which describes *what* runs in the
//! sandbox: full workflow, agent-only, devbox). The runtime is *how* we
//! sandbox at all — Docker or no-Docker.

use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum SandboxRuntime {
    Docker,
    Local,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_from_str() {
        let r: SandboxRuntime = SandboxRuntime::from_str("docker", true).unwrap();
        assert_eq!(r, SandboxRuntime::Docker);
    }

    #[test]
    fn parses_local_from_str() {
        let r: SandboxRuntime = SandboxRuntime::from_str("local", true).unwrap();
        assert_eq!(r, SandboxRuntime::Local);
    }

    #[test]
    fn rejects_unknown_value() {
        assert!(SandboxRuntime::from_str("native", true).is_err());
    }
}
```

- [ ] **Step 2: Wire the module into mod.rs**

Open `src/sandbox/mod.rs`. After the existing `pub mod proxy;` line, add:

```rust
pub mod runtime;
pub use runtime::SandboxRuntime;
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test -p stepyard --lib sandbox::runtime --no-default-features --features postgres 2>&1 | tail -10`
Expected: 3 tests pass.

(Crate name in `cargo test -p` matches the binary crate name in this repo's `Cargo.toml` — substitute the actual name if `stepyard` is different. Check with `cargo metadata --format-version 1 | jq -r '.workspace_members[]' | grep -v stepyard-` if unsure.)

- [ ] **Step 4: Commit**

```bash
git add src/sandbox/runtime.rs src/sandbox/mod.rs
git commit -m "feat(sandbox): introduce SandboxRuntime enum (docker|local)"
```

---

## Task 3: Add precedence resolver `resolve_runtime` (TDD)

**Files:**
- Modify: `src/sandbox/runtime.rs`

The resolver implements: explicit CLI flag > `STEPYARD_SANDBOX` env > legacy `--no-sandbox` flag > profile default.

Profile default uses `cfg`:
- `cfg(feature = "sqlite")` → `Local`
- otherwise (postgres or no feature in tests) → `Docker`

- [ ] **Step 1: Write failing tests for the resolver**

Replace the `#[cfg(test)] mod tests` block at the bottom of `src/sandbox/runtime.rs` with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_from_str() {
        let r: SandboxRuntime = SandboxRuntime::from_str("docker", true).unwrap();
        assert_eq!(r, SandboxRuntime::Docker);
    }

    #[test]
    fn parses_local_from_str() {
        let r: SandboxRuntime = SandboxRuntime::from_str("local", true).unwrap();
        assert_eq!(r, SandboxRuntime::Local);
    }

    #[test]
    fn rejects_unknown_value() {
        assert!(SandboxRuntime::from_str("native", true).is_err());
    }

    fn _resolve(
        cli_flag: Option<SandboxRuntime>,
        env_value: Option<&str>,
        legacy_no_sandbox: bool,
    ) -> Result<SandboxRuntime, ResolveError> {
        resolve_runtime_inner(cli_flag, env_value.map(str::to_string), legacy_no_sandbox)
    }

    #[test]
    fn cli_flag_wins_over_env() {
        let got = _resolve(Some(SandboxRuntime::Local), Some("docker"), false).unwrap();
        assert_eq!(got, SandboxRuntime::Local);
    }

    #[test]
    fn env_used_when_no_cli_flag() {
        let got = _resolve(None, Some("local"), false).unwrap();
        assert_eq!(got, SandboxRuntime::Local);
    }

    #[test]
    fn env_with_unknown_value_errors() {
        let got = _resolve(None, Some("podman"), false);
        assert!(matches!(got, Err(ResolveError::InvalidEnv(_))));
    }

    #[test]
    fn legacy_no_sandbox_promotes_to_local() {
        // Used only when neither CLI flag nor env is set.
        let got = _resolve(None, None, true).unwrap();
        assert_eq!(got, SandboxRuntime::Local);
    }

    #[test]
    fn legacy_no_sandbox_loses_to_explicit_cli_flag() {
        let got = _resolve(Some(SandboxRuntime::Docker), None, true).unwrap();
        assert_eq!(got, SandboxRuntime::Docker);
    }

    #[test]
    fn falls_back_to_profile_default_when_nothing_set() {
        let got = _resolve(None, None, false).unwrap();
        // The compile-time default depends on the active feature. The test
        // crate compiles with `--features postgres` by default, so we expect
        // Docker. Mirror image of the assertion runs in CI's sqlite lane.
        #[cfg(feature = "sqlite")]
        assert_eq!(got, SandboxRuntime::Local);
        #[cfg(not(feature = "sqlite"))]
        assert_eq!(got, SandboxRuntime::Docker);
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cargo test -p stepyard --lib sandbox::runtime --no-default-features --features postgres 2>&1 | tail -20`
Expected: compile error — `resolve_runtime_inner`, `ResolveError` not defined.

- [ ] **Step 3: Implement the resolver**

Append to `src/sandbox/runtime.rs` (after the `pub enum SandboxRuntime`):

```rust
use std::env;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("STEPYARD_SANDBOX has invalid value `{0}`; expected `docker` or `local`")]
    InvalidEnv(String),
}

/// Profile default chosen at compile time. `sqlite` profile → Local;
/// `postgres` (or default-features) profile → Docker.
pub const PROFILE_DEFAULT: SandboxRuntime = {
    #[cfg(feature = "sqlite")]
    {
        SandboxRuntime::Local
    }
    #[cfg(not(feature = "sqlite"))]
    {
        SandboxRuntime::Docker
    }
};

/// Resolve the effective runtime from CLI flag, env var, and legacy bool.
///
/// Precedence: explicit CLI `--sandbox-runtime` > `STEPYARD_SANDBOX` env >
/// legacy `--no-sandbox` (which forces Local if set) > [`PROFILE_DEFAULT`].
pub fn resolve_runtime(
    cli_flag: Option<SandboxRuntime>,
    legacy_no_sandbox: bool,
) -> Result<SandboxRuntime, ResolveError> {
    let env_value = env::var("STEPYARD_SANDBOX").ok();
    resolve_runtime_inner(cli_flag, env_value, legacy_no_sandbox)
}

/// Pure form for tests. Same semantics as [`resolve_runtime`] but takes the
/// env value as a parameter rather than reading the process env.
pub(crate) fn resolve_runtime_inner(
    cli_flag: Option<SandboxRuntime>,
    env_value: Option<String>,
    legacy_no_sandbox: bool,
) -> Result<SandboxRuntime, ResolveError> {
    if let Some(r) = cli_flag {
        return Ok(r);
    }
    if let Some(raw) = env_value {
        return match raw.as_str() {
            "docker" => Ok(SandboxRuntime::Docker),
            "local" => Ok(SandboxRuntime::Local),
            other => Err(ResolveError::InvalidEnv(other.to_string())),
        };
    }
    if legacy_no_sandbox {
        return Ok(SandboxRuntime::Local);
    }
    Ok(PROFILE_DEFAULT)
}
```

(If `thiserror` is not already a workspace dep, add `thiserror = "1"` to the binary crate's `Cargo.toml`. Check first with `grep '^thiserror' Cargo.toml`.)

- [ ] **Step 4: Run tests to confirm they pass**

Run: `cargo test -p stepyard --lib sandbox::runtime --no-default-features --features postgres 2>&1 | tail -15`
Expected: all 8 tests pass.

- [ ] **Step 5: Re-export from sandbox/mod.rs**

Open `src/sandbox/mod.rs`. Update the runtime re-export:

```rust
pub mod runtime;
pub use runtime::{resolve_runtime, ResolveError, SandboxRuntime, PROFILE_DEFAULT};
```

- [ ] **Step 6: Commit**

```bash
git add src/sandbox/runtime.rs src/sandbox/mod.rs Cargo.toml
git commit -m "feat(sandbox): add resolve_runtime precedence (cli > env > legacy > profile)"
```

---

## Task 4: Add `--sandbox-runtime` CLI argument (TDD via clap)

**Files:**
- Modify: `src/cli/commands.rs`

- [ ] **Step 1: Write a parsing test against `ExecuteArgs`**

At the bottom of `src/cli/commands.rs`, locate the `#[cfg(test)] mod tests` (create one if absent). Add:

```rust
#[cfg(test)]
mod sandbox_runtime_tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    #[command(name = "stepyard")]
    struct Wrap {
        #[command(flatten)]
        args: ExecuteArgs,
    }

    #[test]
    fn parses_sandbox_runtime_docker() {
        let cmd = Wrap::try_parse_from([
            "stepyard", "wf.yaml", "--sandbox-runtime", "docker",
        ])
        .unwrap();
        assert_eq!(
            cmd.args.sandbox_runtime,
            Some(crate::sandbox::SandboxRuntime::Docker)
        );
    }

    #[test]
    fn parses_sandbox_runtime_local() {
        let cmd = Wrap::try_parse_from([
            "stepyard", "wf.yaml", "--sandbox-runtime", "local",
        ])
        .unwrap();
        assert_eq!(
            cmd.args.sandbox_runtime,
            Some(crate::sandbox::SandboxRuntime::Local)
        );
    }

    #[test]
    fn missing_sandbox_runtime_is_none() {
        let cmd = Wrap::try_parse_from(["stepyard", "wf.yaml"]).unwrap();
        assert!(cmd.args.sandbox_runtime.is_none());
    }

    #[test]
    fn unknown_runtime_is_rejected() {
        let res =
            Wrap::try_parse_from(["stepyard", "wf.yaml", "--sandbox-runtime", "podman"]);
        assert!(res.is_err());
    }
}
```

- [ ] **Step 2: Run the test to confirm it fails**

Run: `cargo test -p stepyard sandbox_runtime_tests --no-default-features --features postgres 2>&1 | tail -10`
Expected: compile error — `sandbox_runtime` field missing on `ExecuteArgs`.

- [ ] **Step 3: Add the field to `ExecuteArgs`**

In `src/cli/commands.rs`, find the `ExecuteArgs` struct (around line 60 — locate via `grep -n 'pub struct ExecuteArgs' src/cli/commands.rs`). After the existing `pub no_sandbox: bool,` line (around line 78), add:

```rust
    /// Choose the sandbox runtime: `docker` (default in postgres profile) or
    /// `local` (default in sqlite profile). Overrides `STEPYARD_SANDBOX` env.
    /// Mutually compatible with `--no-sandbox` (legacy alias for `=local`); if
    /// both are passed, this flag wins and a deprecation warning is logged.
    #[arg(long = "sandbox-runtime", value_name = "docker|local")]
    pub sandbox_runtime: Option<crate::sandbox::SandboxRuntime>,
```

- [ ] **Step 4: Run the parser tests to confirm they pass**

Run: `cargo test -p stepyard sandbox_runtime_tests --no-default-features --features postgres 2>&1 | tail -10`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/cli/commands.rs
git commit -m "feat(cli): add --sandbox-runtime <docker|local> arg"
```

---

## Task 5: Wire the resolved runtime into `execute_v2` (TDD via integration test scaffolding)

**Files:**
- Modify: `src/cli/commands.rs::execute_v2`

The wiring point is `src/cli/commands.rs:237` — currently:

```rust
let lifecycle: Arc<dyn SandboxLifecycle> = if sandbox_mode == SandboxMode::Disabled {
    Arc::new(LocalShellLifecycle::new())
} else {
    Arc::new(DockerLifecycle::default())
};
```

This branch must keep working but also honor the new flag.

- [ ] **Step 1: Read the current branch site to confirm exact code**

Run: `sed -n '215,250p' src/cli/commands.rs`
Expected: see the `let lifecycle: Arc<dyn SandboxLifecycle> = if sandbox_mode == SandboxMode::Disabled` block.

- [ ] **Step 2: Replace the branch with runtime-aware logic**

Replace the block above with:

```rust
let runtime = crate::sandbox::resolve_runtime(args.sandbox_runtime, args.no_sandbox)
    .map_err(|e| anyhow::anyhow!(e))?;
if args.sandbox_runtime.is_some() && args.no_sandbox {
    tracing::warn!(
        "both --sandbox-runtime and --no-sandbox were passed; --sandbox-runtime wins"
    );
}
let lifecycle: Arc<dyn SandboxLifecycle> = match runtime {
    crate::sandbox::SandboxRuntime::Local => Arc::new(LocalShellLifecycle::new()),
    crate::sandbox::SandboxRuntime::Docker => Arc::new(DockerLifecycle::default()),
};
```

Note: `sandbox_mode` (the orthogonal `Disabled/FullWorkflow/AgentOnly/Devbox` enum) is unchanged. Mode is still used downstream by code that wants to know *what* runs sandboxed; runtime decides *how*.

- [ ] **Step 3: Confirm build still passes on postgres profile**

Run: `cargo check -p stepyard --no-default-features --features postgres 2>&1 | tail -8`
Expected: `Finished` with no errors.

- [ ] **Step 4: Confirm build still passes on sqlite profile**

Run: `cargo check -p stepyard --no-default-features --features sqlite 2>&1 | tail -8`
Expected: `Finished` with no errors.

- [ ] **Step 5: Commit**

```bash
git add src/cli/commands.rs
git commit -m "feat(cli): wire resolved SandboxRuntime to v2 lifecycle selection"
```

---

## Task 6: End-to-end CLI integration test for `local` runtime (TDD)

**Files:**
- Create: `tests/cli_sandbox_runtime.rs`
- Create (if absent): `tests/fixtures/hello_lite.yaml`

This test runs the actual `stepyard` binary (built with `--features sqlite --no-default-features`) against a `Cmd`-only workflow and verifies it succeeds with no Docker daemon required.

- [ ] **Step 1: Author the fixture workflow**

Create `tests/fixtures/hello_lite.yaml`:

```yaml
name: hello_lite
description: |
  Trivial Cmd-only workflow used by the A2 integration test to verify
  that --sandbox-runtime=local executes successfully with no Docker.
steps:
  - name: greet
    type: cmd
    command: echo
    args:
      - "hello-lite"
```

- [ ] **Step 2: Write the failing test**

Create `tests/cli_sandbox_runtime.rs`:

```rust
//! Integration test for PR A2: `--sandbox-runtime=local` runs a Cmd workflow
//! end-to-end with no Docker daemon.
//!
//! The `sqlite` profile is used so the test does not require Postgres either.
//! This is the canonical "Lite mode works" smoke test.

#![cfg(feature = "sqlite")]

use std::path::PathBuf;
use std::process::Command;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(name: &str) -> PathBuf {
    project_root().join("tests").join("fixtures").join(name)
}

#[test]
fn local_runtime_runs_cmd_workflow_without_docker() {
    // Build the binary first to avoid timing out under cargo's default test
    // budget. `cargo run` builds + runs in one shot, but the build can blow
    // past 60s on cold caches.
    let build = Command::new(env!("CARGO"))
        .args([
            "build",
            "--no-default-features",
            "--features",
            "sqlite",
            "--bin",
            "stepyard",
        ])
        .output()
        .expect("failed to build stepyard binary");
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let bin = project_root().join("target").join("debug").join("stepyard");
    assert!(bin.exists(), "binary not at {}", bin.display());

    let workflow = fixture("hello_lite.yaml");
    let tmp = tempfile::tempdir().expect("tmpdir");
    let db_path = tmp.path().join("sessions.db");

    let output = Command::new(&bin)
        .arg("execute")
        .arg(&workflow)
        .arg("--sandbox-runtime")
        .arg("local")
        .arg("--engine")
        .arg("v2")
        .env("STEPYARD_HARNESS_DATABASE_URL", format!("sqlite://{}", db_path.display()))
        // Belt-and-suspenders: even if Docker is installed locally, ensure
        // we'd notice if the lifecycle accidentally tried to call it.
        .env("DOCKER_HOST", "tcp://127.0.0.1:1") // unreachable
        .env("PATH", "/usr/bin:/bin") // strip docker from PATH
        .output()
        .expect("failed to run stepyard");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "stepyard exited non-zero.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("hello-lite") || stderr.contains("hello-lite"),
        "expected `echo hello-lite` output.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn local_runtime_resolved_via_env_var() {
    // Build first (idempotent — cargo skips if up-to-date).
    let build = Command::new(env!("CARGO"))
        .args([
            "build",
            "--no-default-features",
            "--features",
            "sqlite",
            "--bin",
            "stepyard",
        ])
        .output()
        .expect("build failed");
    assert!(build.status.success());

    let bin = project_root().join("target").join("debug").join("stepyard");
    let workflow = fixture("hello_lite.yaml");
    let tmp = tempfile::tempdir().expect("tmpdir");
    let db_path = tmp.path().join("sessions.db");

    // No --sandbox-runtime flag this time. Env var should win.
    let output = Command::new(&bin)
        .arg("execute")
        .arg(&workflow)
        .arg("--engine")
        .arg("v2")
        .env("STEPYARD_SANDBOX", "local")
        .env("STEPYARD_HARNESS_DATABASE_URL", format!("sqlite://{}", db_path.display()))
        .env("DOCKER_HOST", "tcp://127.0.0.1:1")
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run failed");

    assert!(
        output.status.success(),
        "stepyard exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
```

- [ ] **Step 3: Add `tempfile` as a dev-dep if missing**

Run: `grep -n '^tempfile' Cargo.toml`
If absent, append to `[dev-dependencies]`:
```toml
tempfile = "3"
```

- [ ] **Step 4: Run the new test**

Run: `cargo test --test cli_sandbox_runtime --no-default-features --features sqlite 2>&1 | tail -25`
Expected: 2 tests pass. (First run builds the binary; allow up to ~3 min.)

If failure: read stdout/stderr in the test panic — most common causes are (a) `local` runtime is not recognized → check Task 4 wiring; (b) `STEPYARD_HARNESS_DATABASE_URL` parsing — verify the factory from PR A1 accepts `sqlite://` prefix.

- [ ] **Step 5: Commit**

```bash
git add tests/cli_sandbox_runtime.rs tests/fixtures/hello_lite.yaml Cargo.toml
git commit -m "test(cli): integration test for --sandbox-runtime=local on sqlite profile"
```

---

## Task 7: Verify Postgres profile is unaffected

**Files:** none (regression check)

- [ ] **Step 1: Run the existing CLI integration tests on postgres profile**

Run:
```bash
cargo test --workspace --no-default-features --features postgres 2>&1 | tail -20
```
Expected: same green count as baseline (no regressions). The new `cli_sandbox_runtime` test is `#![cfg(feature = "sqlite")]`-gated so it does not run here.

- [ ] **Step 2: Spot-check the legacy `--no-sandbox` path still works**

Run:
```bash
cargo test --workspace --no-default-features --features postgres no_sandbox 2>&1 | tail -10
```
(Substitute the actual existing test name(s) — `grep -rn no_sandbox tests/ src/` to find them.)
Expected: green.

- [ ] **Step 3: If any regressions surfaced, stop and triage**

Common cause: the `if args.sandbox_runtime.is_some() && args.no_sandbox` warn-and-prefer logic mishandles a legacy test that passes both. Adjust by inverting the precedence comment, not by silencing the warning.

---

## Task 8: Help text + README update

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `README.md`

- [ ] **Step 1: Update CLI help banner**

Open `src/cli/mod.rs`. Locate the help banner near line 28 (the line containing `Docker Desktop      — required for --sandbox mode`). Replace that bullet with:

```
• Docker Desktop      — required for --sandbox-runtime=docker (default on postgres profile)
• No external deps    — --sandbox-runtime=local runs Cmd steps directly on the host (default on sqlite profile)
```

And update the example block:

```
stepyard execute my-workflow.yaml --sandbox-runtime=local -- main   Run Lite mode (no Docker)
```

(Leave the existing `--no-sandbox` example for backwards compatibility unless it duplicates the new line — pick whichever reads better in context.)

- [ ] **Step 2: Add a section to README.md**

Open `README.md`. Find a sensible location (after "Quick Start" or near other CLI flag docs). Add:

````markdown
### Sandbox runtime (`--sandbox-runtime`)

Choose between Docker isolation and direct host execution:

```bash
stepyard execute hello.yaml --sandbox-runtime=docker   # Docker container per session
stepyard execute hello.yaml --sandbox-runtime=local    # Run Cmd steps on the host
```

Or via env:

```bash
STEPYARD_SANDBOX=local stepyard execute hello.yaml
```

**Default depends on the build profile:**
- `--features postgres` (production): defaults to `docker`.
- `--features sqlite` (Lite mode): defaults to `local`.

**Precedence:** CLI flag > `STEPYARD_SANDBOX` env > legacy `--no-sandbox` > profile default.
````

- [ ] **Step 3: Commit**

```bash
git add src/cli/mod.rs README.md
git commit -m "docs(cli): document --sandbox-runtime flag and STEPYARD_SANDBOX env"
```

---

## Task 9: Full gates + open PR

**Files:** none

- [ ] **Step 1: Run fmt + clippy on both profiles**

Run:
```bash
cargo fmt --check
cargo clippy --workspace --all-targets --no-default-features --features postgres -- -D warnings
cargo clippy --workspace --all-targets --no-default-features --features sqlite -- -D warnings
```
Expected: all green.

- [ ] **Step 2: Run audit-emit-before-io baseline**

Run:
```bash
cargo run -p xtask -- audit-emit-before-io 2>&1 | tail -5
```
Expected: `3 finding(s)` (unchanged baseline).

If the count rose, the new wiring may have introduced an emit-before-io violation. The most likely culprit is `tracing::warn!` between a `Session::append` and the engine wiring — re-order so the warn fires before any session creation.

- [ ] **Step 3: Run audit-patterns**

Run: `bash scripts/audit-patterns.sh 2>&1 | tail -5`
Expected: blocking gates pass.

- [ ] **Step 4: Final integration smoke**

Run:
```bash
cargo test --workspace --no-default-features --features sqlite 2>&1 | tail -10
cargo test --workspace --no-default-features --features postgres 2>&1 | tail -10
```
Expected: both green.

- [ ] **Step 5: Push and open PR**

```bash
git push -u origin feat/pr-a2-sandbox-runtime-flag
gh pr create \
  --title "feat(cli): --sandbox-runtime flag (Stepyard Lite PR A2)" \
  --body "$(cat <<'EOF'
## Summary

- Adds `--sandbox-runtime <docker|local>` flag and `STEPYARD_SANDBOX` env var.
- Default is profile-aware: `local` on `--features sqlite`, `docker` on `--features postgres`.
- Wires `LocalShellLifecycle` (already exported by `stepyard-sandbox-orchestrator`) to v2 engine when runtime is `local`.
- Legacy `--no-sandbox` flag preserved as alias for `--sandbox-runtime=local`; explicit flag wins, with a deprecation `tracing::warn!` if both are passed.

Spec ref: `docs/superpowers/specs/2026-05-01-stepyard-lite-design.md` §6.

## Test plan

- [ ] `cargo test --no-default-features --features sqlite` green (incl. new `cli_sandbox_runtime` integration)
- [ ] `cargo test --no-default-features --features postgres` green (no regressions; new test is sqlite-cfg'd out)
- [ ] `cargo clippy -- -D warnings` clean on both profiles
- [ ] `cargo run -p xtask -- audit-emit-before-io` baseline still 3
- [ ] Manual smoke: `cargo run --no-default-features --features sqlite -- execute tests/fixtures/hello_lite.yaml --engine v2 --sandbox-runtime=local` succeeds with `DOCKER_HOST=tcp://127.0.0.1:1` set

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 6: Watch CI and merge per Stepyard convention**

Poll: `gh pr view --json statusCheckRollup,mergeable,mergeStateStatus | jq`
On all green: `gh pr merge --squash --auto`.

---

## Self-Review

**Spec coverage (§6):**
- [x] `--sandbox <docker|local>` flag added (named `--sandbox-runtime` to avoid clobbering the existing boolean `--sandbox` and reduce blast radius — equivalent surface).
- [x] `STEPYARD_SANDBOX` env supported.
- [x] Precedence: CLI flag > env > profile default (Task 3 implements; tests assert).
- [x] Profile default: `local` for sqlite, `docker` for postgres (Task 3 `PROFILE_DEFAULT`).
- [x] `LocalShellLifecycle` wired (Task 5).
- [x] CLI integration test (`tests/cli_sandbox_runtime.rs` Task 6).
- [x] No event-schema changes / no replay implications (per spec — verified by absence of changes to `crates/stepyard-core/src/event.rs`).

**Placeholder scan:** None — every step has the actual code or command. No "implement appropriate error handling".

**Type consistency:**
- `SandboxRuntime` used identically in Tasks 2/3/4/5/6.
- `resolve_runtime` signature matches between Task 3 (definition) and Task 5 (call site).
- Re-export pattern in Task 2 step 2 matches the import in Task 5 step 2 (`crate::sandbox::SandboxRuntime`, `crate::sandbox::resolve_runtime`).

**Open issues for PR review:**
1. Naming choice `--sandbox-runtime` vs replacing `--sandbox`: this plan keeps both. Spec says `--sandbox <docker|local>`. The renamed approach avoids breaking users on stable `--sandbox` boolean form. Reviewer may push for the literal spec — easy to flip in a follow-up.
2. The integration test in Task 6 strips `PATH` to `/usr/bin:/bin` to be sure no Docker leak — this may break on systems where `echo` is in a non-standard `PATH`. If CI fails for this reason, weaken the assertion to "no docker process spawned" via a `pgrep` check instead.
3. `PROFILE_DEFAULT` is a `const` evaluated at compile time — works fine, but means a single binary built with `--features sqlite` cannot ever default to Docker even if `STEPYARD_SANDBOX` is unset. That's the intended Lite-mode UX. If we ever want a runtime-overridable default, swap to a `OnceLock<SandboxRuntime>` set from `main`.
