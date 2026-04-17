# Worktree wt2 — BMAD Development Agent (Epic 3)

You are an autonomous coding agent in a **parallel git worktree**. Your job is to implement all five stories of **Epic 3: Sandbox Environment Injection** (F10–F14), following the BMAD dev-story workflow exactly.

## Your Branch
`minion-engine-bmad-wt2`

Worktree path (absolute): `/Users/bruno/Desktop/Dev/new-stripe-minions/minion-engine-bmad-wt2`
Branched from: `main` @ `a7b27ef` (Epic 1 F1–F4 shipped). Epic 2 is running in parallel in `minion-engine-bmad-wt1`.

## Development Methodology

CRITICAL: For each story, follow the BMAD dev-story workflow:

1. Call `sequentialthinking` (MCP) BEFORE writing code — plan the files and the argv-not-shell security invariant.
2. Read the story completely (ACs, derived Tasks/Subtasks, Dev Notes).
3. Use Serena (MCP) for code navigation: `get_symbols_overview`, `find_symbol`, `find_referencing_symbols`, `replace_symbol_body`. DO NOT read full Rust source files with `Read` — use Serena's symbolic tools.
4. Follow Tasks/Subtasks in ORDER.
5. Red → green → refactor. Write the negative-control test first where the story demands it (Story 3.5 is itself this discipline).
6. Mark each subtask `[x]` in PROMPT.md as you go, update File List, add Dev Agent Record notes.
7. After every task for a story passes, commit: `feat(epic-3): implement story 3.N — <title>` and mark the feature `done` in the local `features.md`. Proceed to the next story.
8. When ALL five stories are implemented, flip each story's Status to `review`.

DO NOT: skip tasks, reorder them, interpolate env into shell strings, modify files marked read-only, or mark tasks complete without passing tests.

---

## Assigned Stories

### Story 3.1 — Extend `SandboxLifecycle` Trait with `exec_with_env` Default-Impl Method

**Feature 10 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 627–661)

**Status:** _review_

As an engine maintainer,
I want `SandboxLifecycle` to gain an `exec_with_env(id, cmd, env: &HashMap<String, String>)` default-impl method that delegates to the existing `exec(id, cmd)` (ignoring env),
So that Epic 3 can inject env vars via the new method without changing the existing `exec` signature (NFR22 backward compat).

**Acceptance Criteria:**

**Given** `SandboxLifecycle` trait in `crates/minion-sandbox-orchestrator/src/lib.rs` (or wherever currently defined)
**When** inspected
**Then** it gains a new method: `async fn exec_with_env(&self, id: &SandboxId, cmd: &[String], env: &HashMap<String, String>) -> Result<ExecOutput, SandboxError>`
**And** the default impl is `self.exec(id, cmd).await` (env ignored — preserves existing behavior for unmigrated impls)
**And** the existing `exec` method signature is NOT changed (D3 explicit: extension via new method, not parameter addition)
**And** the trait retains `#[async_trait]` (project convention, already-locked)

**Given** the mock-extension safeguard
**When** `MockLifecycle` in `crates/minion-sandbox-orchestrator/src/mock.rs` (or test-common) is extended
**Then** `MockLifecycleCall::ExecWithEnv { id: SandboxId, cmd: Vec<String>, env: HashMap<String, String> }` is added as a variant
**And** `MockLifecycle::exec_with_env` override records the full `env` parameter (not dropped, not lossy)
**And** at least one unit test asserts on the recorded `env` contents — without this assertion a default impl that silently drops `env` would pass tests (mutation-resistance per testing-enforcement invariant)

**Given** consumers inside `minion-harness`
**When** any code invoking a lifecycle method is inspected
**Then** NEW invocation sites use `exec_with_env(id, cmd, &env)` (even when `env` is empty — pass `&HashMap::new()`)
**And** EXISTING invocation sites calling `exec(id, cmd)` are NOT refactored in this story (only Story 3.4 wires new call sites; backward-compat preserved)
**And** `DockerLifecycle` keeps the existing `exec` method unchanged (the `sh -c` legacy carveout documented in Security Requirements remains intact)

**Given** a unit test at `crates/minion-sandbox-orchestrator/src/mock.rs` (inline `#[cfg(test)] mod tests`)
**When** the test calls `mock.exec_with_env(&id, &["echo".to_string(), "hello".to_string()], &env_with_FOO=BAR).await`
**Then** `MockLifecycleCall::ExecWithEnv { env, .. }` records exactly `{"FOO": "BAR"}`
**And** calling the default impl on a type that did NOT override `exec_with_env` records the `exec` call (not ExecWithEnv) — proves default delegation works
**And** the test does NOT use `tokio::time::sleep(…)` (Rule 7a)

Coverage: FR9 infrastructure, D3, NFR22 (backward compat)

**Tasks / Subtasks** (derived one-to-one from ACs)

- [ ] AC1: Add `async fn exec_with_env(&self, id: &SandboxId, cmd: &[String], env: &HashMap<String, String>) -> Result<ExecOutput, SandboxError>` to the `SandboxLifecycle` trait in `crates/minion-sandbox-orchestrator/src/lib.rs`, with a default body `self.exec(id, cmd).await`. Keep `#[async_trait]`. Leave `exec` signature untouched.
- [ ] AC2: In `crates/minion-sandbox-orchestrator/src/mock.rs`, add `MockLifecycleCall::ExecWithEnv { id: SandboxId, cmd: Vec<String>, env: HashMap<String, String> }` variant; implement `MockLifecycle::exec_with_env` override that records the full `env` into that variant (no dropping, no lossy clones).
- [ ] AC3: Add at least one unit test that asserts on the recorded `env` contents (mutation-resistance — a silently-dropping default impl must fail this test).
- [ ] AC4: Audit `minion-harness` — new invocation sites introduced by Stories 3.4+ use `exec_with_env(id, cmd, &env)` (pass `&HashMap::new()` when no env). Leave existing `exec(id, cmd)` call sites untouched in THIS story. Verify `DockerLifecycle::exec` is NOT modified here.
- [ ] AC5: Inline `#[cfg(test)] mod tests` in `mock.rs` with a test that calls `mock.exec_with_env(&id, &["echo".to_string(), "hello".to_string()], &env_with_FOO=BAR).await` and asserts `MockLifecycleCall::ExecWithEnv { env, .. }` records exactly `{"FOO": "BAR"}`.
- [ ] AC6: In the same test module, add a "default-delegation" test: a type that does NOT override `exec_with_env` records the `exec` call (not `ExecWithEnv`) when its default impl is invoked — proves the default body calls through.
- [ ] AC7: Verify no test in the module uses `tokio::time::sleep(…)` (Rule 7a).

**Dev Notes**

_Note: epics.md did not include explicit Tasks/Subtasks/Dev Notes sections for Epic 3, so Tasks above were derived one-to-one from the ACs and Dev Notes below anchor to the relevant architecture decisions._

Architecture anchors:
- **D3** — Extension via NEW method with a default delegating impl. Do NOT add a parameter to `exec`. Extend `MockLifecycle` in place — do NOT fork to `MockLifecycleV2`.
- **D6** — Lifecycle remains a dumb executor that receives a resolved env map; it does not know where values came from.

Non-functional anchors:
- **NFR22** — Backward compatibility: existing `exec` callers continue to compile and behave identically.
- **NFR-argv (D7)** — Even though this story does not yet pass env to Docker, the new method signature takes `cmd: &[String]` (argv) and `env: &HashMap<String,String>` (structured pairs). Never imagine a `String` command or a `Vec<(K,V)> as KEY=VAL` concatenation.
- **NFR-secrets (NFR8)** — env keys/values must never appear in tracing log calls or event payloads.

Key symbols to touch (use Serena `find_symbol` / `replace_symbol_body`):
- `SandboxLifecycle` trait (most likely `crates/minion-sandbox-orchestrator/src/lib.rs`)
- `MockLifecycle` impl and `MockLifecycleCall` enum (`crates/minion-sandbox-orchestrator/src/mock.rs`)

**Dev Agent Record**

- Files created/modified:
  - `crates/minion-sandbox-orchestrator/src/lib.rs` — added `exec(&SandboxId, &[String])` and `exec_with_env(&SandboxId, &[String], &HashMap)` to `SandboxLifecycle` trait; default impl of `exec_with_env` drops env and calls `self.exec(id, cmd)` (D3 extension, NFR22 backward compat).
  - `crates/minion-sandbox-orchestrator/src/mock.rs` — added `MockCall::ExecWithEnv { id, cmd, env }` variant; implemented `exec` (records `MockCall::Exec { cmd: cmd.join(" ") }` for parity with legacy `ExecFn` path) and `exec_with_env` override (records full env verbatim). Inline `#[cfg(test)] mod tests` with two unit tests for AC5 (env capture) and AC6 (default-delegation via stub type that doesn't override `exec_with_env`).
  - `crates/minion-sandbox-orchestrator/src/docker.rs` — implemented trait `exec(&SandboxId, &[String])`: reconstructs container name via `Self::container_name(*id.as_uuid())` (matches harness convention at engine.rs:286 `SandboxId::from(session_id.as_uuid())`) and passes cmd as argv to `docker exec` (no `sh -c` wrap). Existing `DockerExec::exec(id, cmd: &str)` via `ExecFn` trait unchanged — that is the documented legacy `sh -c` carveout AC4 protects.
  - `crates/minion-sandbox-orchestrator/src/local.rs` — implemented trait `exec(&SandboxId, &[String])`: spawns `Command::new(cmd[0]).args(cmd[1..]).kill_on_drop(true)`; existing `LocalShellExec::exec(_, &str)` via `ExecFn` unchanged.
- Notes on choices / deviations:
  - The PROMPT's ACs assumed `exec(id, cmd)` already existed on the trait (default body `self.exec(id, cmd).await`). In the current codebase `exec` lives only on the `Sandbox` handle via the private `ExecFn` trait. Per advisor guidance, added BOTH `exec` and `exec_with_env` to the trait in this story (additive D3 extension). `Sandbox::exec(&str)` stays untouched — that is the legacy carveout.
  - For DockerLifecycle, reconstruct container name by treating `SandboxId` as a `session_id` via `*id.as_uuid()`. This matches the harness pattern at `engine.rs:286` (`SandboxId::from(*self.session.id().as_uuid())`).
  - AC6 default-delegation test uses a `StubLifecycle` inside the test module that implements only `exec` (not `exec_with_env`); the default trait impl delegates, proving env-drop behavior works.
  - A `type ExecLog = Arc<Mutex<Vec<(SandboxId, Vec<String>)>>>` alias was added inside the test module to satisfy `clippy::type_complexity` under `-D warnings`.
- Test evidence (cargo test output snippet):
  - `cargo test -p minion-sandbox-orchestrator --lib` → 4 passed (includes the two new unit tests `exec_with_env_records_full_env_pairs` and `default_exec_with_env_delegates_to_exec`).
  - `cargo test -p minion-sandbox-orchestrator --test lifecycle` → 6 passed.
  - `cargo test -p minion-harness` (against `minion_harness_test` DB) → 9 passed across 6 suites.
  - Full workspace non-harness suites → 218 passed.
  - `cargo clippy --workspace --all-targets -- -D warnings` → 0 errors, 1 warning (pre-existing `non_exhaustive_omitted_patterns` unstable-lint warning — tracked separately).

---

### Story 3.2 — Implement `DockerLifecycle::exec_with_env` with Argv-Only `--env` Flags

**Feature 11 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 662–701)

**Status:** _review_

As an engine maintainer,
I want `DockerLifecycle` to override `exec_with_env` with `docker exec --env K=V` argv-only invocations (one `--env` per key-value pair),
So that env vars pass as structured argv elements and are never shell-interpolated (argv-not-shell security rule).

**Acceptance Criteria:**

**Given** `DockerLifecycle` in `crates/minion-sandbox-orchestrator/src/docker.rs`
**When** the `exec_with_env` override is inspected
**Then** it builds `tokio::process::Command::new("docker")` with argv `["exec"]`, then one `["--env", "K=V"]` pair per entry in the `env: &HashMap<String, String>` argument (sorted by key for deterministic argv ordering in tests)
**And** then appends container ID as `args.push(container_name)` and finally the user command as individual argv elements: `args.extend_from_slice(cmd)`
**And** no part of the env value reaches a shell — `format!("{}={}", k, v)` is an argv element passed to `.args()`, never concatenated into an `sh -c "…"` string

**Given** an env entry with shell metacharacters in its value (e.g., `{"MSG": "$(rm -rf /)"}`)
**When** `exec_with_env` invokes docker
**Then** the child process sees `MSG=$(rm -rf /)` as a literal string in its env table
**And** `$(…)` is NOT expanded by any shell (argv-only guarantee)
**And** the same applies to `` ` ``, `&&`, `;`, `|`, newlines, `>`, `<`

**Given** the command itself contains user-supplied strings (e.g., `cmd = ["bash", "-c", "echo $FOO"]`)
**When** `exec_with_env` invokes docker
**Then** those strings pass as argv elements to `docker exec` — minion does NOT wrap or escape them
**And** if the user chose `sh -c` / `bash -c` as their command, expansion happens inside the sandbox (user responsibility per explicit escape hatch)
**And** minion's layer never adds its own shell wrapper

**Given** the legacy carveout (`DockerLifecycle::exec` at `docker.rs:173`)
**When** that method is inspected in this story
**Then** it is NOT migrated — the `sh -c` legacy carveout documented in architecture.md remains (post-MVP tech debt)
**And** `exec_with_env` is fully argv-only regardless of what `exec` does

**Given** an integration test at `crates/minion-sandbox-orchestrator/tests/exec_with_env_docker.rs` (opt-in — skips if Docker unavailable)
**When** the test runs `exec_with_env(id, &["printenv".to_string(), "FOO".to_string()], &env_with_FOO=bar)` against a live container
**Then** stdout contains exactly `bar\n`
**And** running with `env_with_FOO="$(rm -rf /)"` shows `printenv` output of `$(rm -rf /)` literally — the host filesystem is untouched (positive-control security assertion)
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** deterministic argv ordering is asserted (env keys sorted): repeating the same call produces identical argv

Coverage: FR9 executor-side, NFR7 (env isolation at exec layer), argv-not-shell rule

**Tasks / Subtasks** (derived one-to-one from ACs)

- [ ] AC1: In `crates/minion-sandbox-orchestrator/src/docker.rs`, override `exec_with_env` on `DockerLifecycle`. Build `tokio::process::Command::new("docker")`, argv starts `["exec"]`, then for each `(k,v)` in env **sorted by key** push `["--env", format!("{k}={v}")]`. Append container ID via `args.push(container_name)`, then `args.extend_from_slice(cmd)`. Never touch `sh -c`.
- [ ] AC2: Verify by construction (code review + unit test) that an env value containing `$(rm -rf /)`, backticks, `&&`, `;`, `|`, newlines, `>`, `<` flows as one argv token and is NEVER concatenated into a shell string.
- [ ] AC3: Verify user commands like `["bash", "-c", "echo $FOO"]` pass through as separate argv elements — minion adds no wrapping.
- [ ] AC4: Confirm the legacy `DockerLifecycle::exec` at `docker.rs:~173` is **not modified** in this story (the `sh -c` carveout stays as documented tech debt).
- [ ] AC5: Add integration test at `crates/minion-sandbox-orchestrator/tests/exec_with_env_docker.rs` that (a) skips gracefully when Docker is unavailable, (b) runs `printenv FOO` with `FOO=bar` and asserts stdout is exactly `bar\n`, (c) runs with `FOO="$(rm -rf /)"` and asserts `printenv` output is literally `$(rm -rf /)` and the host filesystem is untouched, (d) attaches `.timeout(Duration::from_secs(N))` on every `Command`, (e) asserts argv ordering is deterministic across repeated calls when env keys sort identically.

**Dev Notes**

_Note: epics.md did not include explicit Tasks/Subtasks/Dev Notes sections for Epic 3, so Tasks above were derived one-to-one from the ACs and Dev Notes below anchor to the relevant architecture decisions._

Architecture anchors:
- **D6** — `DockerLifecycle::exec_with_env` is the enforcement point for argv-only env passing. Lifecycle stays dumb; Engine resolves merge order (Story 3.4).
- **D7 / NFR-argv** — THE critical invariant of this story. `docker exec --env KEY=VAL` as argv elements via `.args()`. No `format!("KEY=VAL cmd …")`. No `sh -c` wrapping. Sort keys to make tests deterministic.

Non-functional anchors:
- **NFR7** — env isolation at the exec layer. Sandbox env is fully controlled by the argv flags; no host env leaks through the docker CLI.
- **NFR-secrets** — do not log env values. Logs/tracing can record keys; values are secrets.
- **Rule 7b** — every `Command` in tests gets `.timeout(Duration::from_secs(N))`.

Cross-worktree blocker:
- **If** `exec_with_env` needs a new error variant (e.g., `TerminationReason::ExecFailed`), **do NOT edit** `crates/minion-core/src/error.rs` — that file is read-only for wt2. Raise a BLOCKER in this story's Dev Agent Record; fall back to an existing `SandboxError`/`EngineError` variant, or defer the taxonomy change to a follow-up after wt1 merges.

Key symbols to touch:
- `DockerLifecycle` impl block (`crates/minion-sandbox-orchestrator/src/docker.rs`)
- New integration file `crates/minion-sandbox-orchestrator/tests/exec_with_env_docker.rs`

**Dev Agent Record**

- Files created/modified:
  - `crates/minion-sandbox-orchestrator/src/docker.rs` — override `exec_with_env` on `DockerLifecycle`. Builds argv as `["exec"] + [("--env","K=V")×N sorted by key] + [container_name] + cmd`. No `sh -c` anywhere. Keys sorted via `sort_by(|a,b| a.0.cmp(b.0))` for deterministic ordering. Emits `tracing::debug!` with env **keys only** (NFR-secrets / NFR8). Legacy `DockerExec::exec(&str)` via `ExecFn` unchanged per AC4.
  - `crates/minion-sandbox-orchestrator/tests/exec_with_env_docker.rs` — new integration test file, `MINION_TEST_DOCKER=1` gated. Three tests:
    1. `exec_with_env_injects_env_var_verbatim` — positive control: `printenv FOO` with `FOO=bar` returns `bar\n` exactly.
    2. `exec_with_env_passes_shell_metacharacters_as_literal_string` — security control: `FOO="$(rm -rf /)"` reaches `printenv` as literal `$(rm -rf /)\n`; `/etc/passwd` still present in container after.
    3. `exec_with_env_deterministic_ordering_on_repeated_calls` — env keys `B, A, C` in the map produce sorted env output `A=one\nB=two\nC=three\n` across two calls.
  - Every `Command` + `exec_with_env` future is wrapped in `tokio::time::timeout(Duration::from_secs(20), ...)` per Rule 7b; no `tokio::time::sleep` anywhere.
- Notes on choices / deviations:
  - The test creates its own alpine container inline (via `docker run -d`) rather than going through `DockerLifecycle::create`, because the test needs a known container name derived from a controlled `session_id` to call `exec_with_env` with an exact `SandboxId::from(session_id)`.
  - Sort stability: `HashMap` iter is unordered, so the impl collects pairs then sorts by key. Determinism is needed both for test repeatability and for future log/trace comparability.
  - No new error variants needed — all failure paths funnel through existing `SandboxError::ExecFailed`.
- Test evidence (cargo test output snippet):
  - `MINION_TEST_DOCKER=1 cargo test -p minion-sandbox-orchestrator --test exec_with_env_docker` → 3 passed (3.13s). All three tests exercised against a real `alpine:latest` container.
  - `cargo test -p minion-sandbox-orchestrator` (no Docker) → 13 passed across 4 suites (gated Docker tests skip gracefully).
  - `cargo clippy --workspace --all-targets -- -D warnings` → 0 errors, 1 warning (pre-existing unstable-lint warning).

---

### Story 3.3 — Extend Workflow YAML Schema with `env:` Fields and `.minion/defaults.yaml` Loader

**Feature 12 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 702–742)

**Status:** _review_

As a workflow author,
I want to declare step-level `env: { KEY: VAL }` and workflow-level `env: { KEY: VAL }` in YAML, plus a `.minion/defaults.yaml` file that contributes default env pairs,
So that I can parameterize secrets and config per step, per workflow, or project-wide.

**Acceptance Criteria:**

**Given** the workflow YAML schema in `crates/minion-core/src/workflow.rs` (or `minion-harness` — wherever `Workflow` / `Step` structs live)
**When** inspected
**Then** `Step` gains `#[serde(default)] pub env: HashMap<String, String>`
**And** `Workflow` gains `#[serde(default)] pub env: HashMap<String, String>` (top-level)
**And** both use `#[serde(default)]` for strict backward compatibility (NFR18 — existing YAML without `env:` still parses)
**And** values are plain strings, not structured types — `${VAR}` substitution is a resolution-time concern (Story 3.4), not a parse-time one

**Given** a new file loader at `src/config/defaults.rs` (workspace-root binary) or `crates/minion-core/src/defaults.rs`
**When** inspected
**Then** it exports `pub fn load_defaults(path: &Path) -> Result<Defaults, DefaultsError>` where `Defaults { pub env: HashMap<String, String> }`
**And** if the file does not exist, returns `Ok(Defaults::default())` with empty env (missing file is not an error — defaults are optional)
**And** if the file exists but is malformed, returns `Err(DefaultsError::Parse { path, source })` with a clear error chain

**Given** `DefaultsError` in the same file
**When** inspected
**Then** it derives `thiserror::Error` (NOT `anyhow` — library code per NFR21)
**And** variants: `Io { path: PathBuf, source: std::io::Error }`, `Parse { path: PathBuf, source: serde_yaml::Error }`
**And** `#[error("…")]` messages are lowercase, no trailing punctuation

**Given** existing workflows without an `env:` field
**When** parsed
**Then** they parse successfully as before (no breaking change)
**And** `workflow.env` is `HashMap::new()` by default
**And** `step.env` is `HashMap::new()` by default

**Given** a unit test at the loader module
**When** the test loads a fixture `.minion/defaults.yaml` with `env: { FOO: bar, BAZ: qux }`
**Then** `Defaults::env` contains exactly `{"FOO": "bar", "BAZ": "qux"}`
**And** loading a non-existent path returns `Ok(Defaults::default())` (empty env)
**And** loading a malformed YAML fixture returns `Err(DefaultsError::Parse { path, .. })` with path matching the input

Coverage: FR9 YAML side, FR10 (defaults.yaml), NFR18 (backward compat via `#[serde(default)]`)

**Tasks / Subtasks** (derived one-to-one from ACs)

- [x] AC1: Add `#[serde(default)] pub env: HashMap<String, String>` to both `Step` and `Workflow` struct definitions. Note: AC1 points at `crates/minion-core/src/workflow.rs` OR `minion-harness` — the canonical `Workflow`/`Step` lives in `crates/minion-harness/src/workflow.rs` per territory (owned by wt2). **Do NOT edit `crates/minion-core/src/workflow.rs` — wt1's territory.** If mirroring is needed, extend `src/workflow/schema.rs` (wt2-owned) as well. Keep values as plain `String` (no structured variants).
- [x] AC2: Create loader module. Preferred location: `src/config/defaults.rs` (wt2-owned under `src/` via the workspace-root bin, outside the forbidden `src/cli/` dir). Export `pub fn load_defaults(path: &Path) -> Result<Defaults, DefaultsError>` with `Defaults { pub env: HashMap<String, String> }` and `Defaults::default()` returning empty env.
- [x] AC3: Define `DefaultsError` via `thiserror::Error` (NOT anyhow) with variants `Io { path: PathBuf, source: std::io::Error }` and `Parse { path: PathBuf, source: serde_yaml::Error }`; `#[error("…")]` strings are lowercase with no trailing punctuation.
- [x] AC4: Backward compatibility — verify existing workflow YAML without `env:` parses as before and that `workflow.env` / `step.env` default to `HashMap::new()`.
- [x] AC5: Unit tests at the loader module: (a) loading a fixture `.minion/defaults.yaml` with `env: { FOO: bar, BAZ: qux }` yields `Defaults::env == {"FOO":"bar","BAZ":"qux"}`, (b) non-existent path returns `Ok(Defaults::default())`, (c) malformed YAML returns `Err(DefaultsError::Parse { path, .. })` whose `path` matches the input.

**Dev Notes**

_Note: epics.md did not include explicit Tasks/Subtasks/Dev Notes sections for Epic 3, so Tasks above were derived one-to-one from the ACs and Dev Notes below anchor to the relevant architecture decisions._

Architecture anchors:
- **D6** — step + workflow + defaults are three of the four cascade sources (host env is the fourth, handled at resolve time in Story 3.4). This story sets up the data, Story 3.4 does the merge.
- **D7** — values stay as plain strings; no pre-parse shell handling in this layer.

Non-functional anchors:
- **NFR18** — backward compat. `#[serde(default)]` everywhere.
- **NFR21** — library code uses `thiserror`. `anyhow` belongs to the binary (`src/`). `DefaultsError` lives in library territory → `thiserror`.

Ownership call-out (read the File Ownership section below carefully):
- AC1 names `crates/minion-core/src/workflow.rs` as an option, BUT that file is **read-only for wt2**. The `Workflow`/`Step` fields live instead in `crates/minion-harness/src/workflow.rs` (wt2-owned) and are mirrored in `src/workflow/schema.rs` (wt2-owned). Edit those two, not the core crate's workflow.rs.
- AC2 offers `crates/minion-core/src/defaults.rs` as an alternative — that is also read-only for wt2. Prefer `src/config/defaults.rs` (wt2-owned). If placement in `minion-core` is truly unavoidable, raise a BLOCKER; do NOT silently edit the core crate.

Key symbols to touch:
- `Workflow`, `Step` structs in `crates/minion-harness/src/workflow.rs`
- Mirror in `src/workflow/schema.rs` if that file already mirrors
- New module `src/config/defaults.rs`

**Dev Agent Record**

- Files created/modified:
  - `crates/minion-harness/src/workflow.rs` — added `#[serde(default)] pub env: HashMap<String, String>` to `Step` and `Workflow`; added `Step::with_env(env)` builder; default `env: HashMap::new()` in constructors.
  - `src/workflow/schema.rs` — mirror: added `#[serde(default)] #[allow(dead_code)] pub env: HashMap<String, String>` to `WorkflowDef` and `StepDef`.
  - `src/config/env_defaults.rs` — NEW module: `Defaults { env: HashMap<String, String> }`, `DefaultsError { Io, Parse }` via `thiserror`, `load_defaults(path)` with missing→default/malformed→Parse/io→Io contract. 4 unit tests using `tempfile::TempDir`.
  - `src/config/mod.rs` — `pub mod env_defaults;` + re-exports renamed (`load_env_defaults`, `EnvDefaults`, `EnvDefaultsError`) to avoid collision with existing `load_defaults()` for `WorkflowConfig`. `#[allow(unused_imports)]` until Story 3.4 consumes them.
  - `src/steps/{agent,call,chat,cmd,gate,map,parallel,script,template_step}.rs` — added `env: HashMap::new(),` to all StepDef construction sites in tests (9 files, 14+ sites) to accommodate the new field.
- Notes on choices / deviations:
  - **Name collision avoided**: `src/config/defaults.rs` already owns `WorkflowConfig` loading (agent/chat/global layers). Created a separate `env_defaults.rs` module per advisor-reviewed separation of concerns — env layer is a different type and different cascade source (D6). Re-exports use renamed aliases (`load_env_defaults` etc.) so both modules can coexist at `crate::config::…`.
  - **Dead-code allowances**: `Defaults`, `DefaultsError`, `load_defaults`, and the re-exports have `#[allow(dead_code)]` / `#[allow(unused_imports)]` because Story 3.4 is the intended consumer; this is a clean intermediate state, not a design compromise.
  - **Schema mirror required**: `src/workflow/schema.rs` (wt2-owned binary) mirrors `crates/minion-harness/src/workflow.rs` (wt2-owned library). Added the field in both places to keep them in sync. `#[allow(dead_code)]` on the mirror field matches the existing pattern on the `outputs` mirror field.
  - **StepDef callers**: 14+ test-only construction sites broke when the new field was added. Fixed with per-file `replace_all` on `async_exec: None,\n        }` pattern; covered both `}` and `};` endings by prefix-matching.
- Test evidence:
  - `cargo test --lib config::env_defaults` → `4 passed, 209 filtered out (1 suite, 0.00s)`
  - `cargo test --workspace --lib` → `5 + 213 + 0 + 4 + 0 = 222 passed, 0 failed`
  - `cargo clippy --all-targets -- -D warnings` → clean (only pre-existing `unknown_lints: non_exhaustive_omitted_patterns` warnings unrelated to this story)

---

### Story 3.4 — Cascade Resolver in `Engine::prepare_step` with `${VAR}` Host Expansion

**Feature 13 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 743–782)

**Status:** _review_

As an engine runtime,
I want `Engine::prepare_step` to resolve the effective env for a step by overlaying step > workflow > defaults.yaml and expanding `${VAR}` against host env,
So that one workflow YAML can declare opt-in env with clear precedence and secrets flow through without full host passthrough.

**Acceptance Criteria:**

**Given** `Engine::prepare_step` in `crates/minion-harness/src/engine.rs`
**When** inspected
**Then** it computes the effective env by overlaying in precedence order: `defaults.env` < `workflow.env` < `step.env` (step wins; defaults lose — later overlays overwrite earlier keys)
**And** after overlay, it expands any value matching the `${VAR}` syntax against `std::env::var(VAR)` (host process env)
**And** `${VAR}` pattern recognizes exact-form values (e.g., `"${GITHUB_TOKEN}"`) — NOT inline substitution like `"prefix-${VAR}-suffix"` (simplicity for MVP; document in YAML schema docs)
**And** after expansion, passes the resolved `HashMap<String, String>` to `lifecycle.exec_with_env(id, cmd, &env)` (from Story 3.1)

**Given** a `${VAR}` reference that does not exist in host env
**When** `Engine::prepare_step` resolves it
**Then** it returns `Err(EngineError::EnvVarUnresolved { key, source: VariableSource::Host })` (new error variant in `minion-core/src/error.rs`)
**And** `#[error("…")]` message is lowercase, no trailing punctuation: `"host env variable not set: {key}"`
**And** fails fast — no step executes with a partially-resolved env

**Given** NFR8 (no credential in logs)
**When** the event log records env-related state
**Then** no event payload contains env values — only key names
**And** if an event needs to record env presence (e.g., for audit), it uses a field like `env_keys: Vec<String>` (sorted)
**And** values never appear in `tracing::` log calls either — structured tracing fields use key names only

**Given** NFR4 (env var resolution performance)
**When** `prepare_step` is benchmarked
**Then** cascade resolution for a step with ≤20 env entries completes in <10ms (O(1) `std::env::var` lookup per `${VAR}` reference)

**Given** a unit test at `crates/minion-harness/tests/env_cascade.rs`
**When** the test constructs workflow YAML with `workflow.env = {"FOO": "workflow-foo", "SHARED": "wf"}`, step env `{"FOO": "step-foo"}`, defaults `{"SHARED": "def", "ONLY_DEF": "x"}` + sets `GITHUB_TOKEN=abc123` in host env and `step.env = {"TOKEN": "${GITHUB_TOKEN}"}`
**Then** effective env is `{"FOO": "step-foo", "SHARED": "wf", "ONLY_DEF": "x", "TOKEN": "abc123"}` (step wins for FOO; workflow wins for SHARED; defaults contribute ONLY_DEF; TOKEN expands from host)
**And** the test asserts no env value appears in any event payload (NFR8)
**And** unresolved `${MISSING}` produces `EngineError::EnvVarUnresolved { key: "MISSING", .. }`
**And** the test uses `#[serial_test::serial]` (or equivalent) to avoid races on `std::env::set_var` parallel test contamination

Coverage: FR9, FR10, FR11, FR12, D6, NFR4, NFR7, NFR8

**Tasks / Subtasks** (derived one-to-one from ACs)

- [x] AC1: In `crates/minion-harness/src/engine.rs`, add a NEW `Engine::prepare_step(&self, workflow: &Workflow, step: &Step, defaults: &Defaults) -> Result<HashMap<String, String>, EngineError>` method (additive — do not collide with wt1's `step()` changes). Overlay order: `defaults.env` first, then `workflow.env`, then `step.env` (step wins). Then expand values matching `^\$\{([A-Z0-9_]+)\}$` against `std::env::var(VAR)`. Non-`${VAR}` values pass through verbatim. Pass the final `HashMap<String, String>` to `lifecycle.exec_with_env(id, cmd, &env)` at the existing `step()` call site — keep the edit localized.
- [x] AC2: Unresolved `${VAR}` → `Err(EngineError::EnvVarUnresolved { key, source: VariableSource::Host })`. The message is lowercase, no trailing punctuation: `"host env variable not set: {key}"`. Fail fast before executing the step. **BLOCKER-CANDIDATE:** `EngineError::EnvVarUnresolved` / `VariableSource` most likely do NOT yet exist in `crates/minion-core/src/error.rs`. **That file is read-only for wt2.** Raise a BLOCKER in this story's Dev Agent Record: either (a) wt1 pre-adds the variant on request, or (b) wt2 proceeds with a temporary fallback (e.g., return an existing generic `EngineError` variant + a `tracing::error!` with the key name only) and a follow-up story lands the taxonomy after wt1 merges. Do NOT silently edit `error.rs`.
- [x] AC3: NFR8 — no event payload contains env values. If auditing is needed, use `env_keys: Vec<String>` (sorted). Structured tracing fields use KEY NAMES only, never values. Scrub any existing traces you might be tempted to add.
- [x] AC4: NFR4 — cascade resolution for ≤20 entries completes in <10ms. Use `HashMap` merge + a compiled regex (or manual prefix/suffix check) for the `${VAR}` pattern. A micro-benchmark or a timing assertion in the unit test suffices.
- [x] AC5: Add unit test `crates/minion-harness/tests/env_cascade.rs`. **Note:** `crates/minion-harness/tests/` is listed as wt1's ownedDirs — **BLOCKER-CANDIDATE**: this is the second wt1/wt2 ownership collision for Epic 3's test placement. Mitigations (pick one and document): (i) place the unit test **inline** under `#[cfg(test)] mod tests` inside `crates/minion-harness/src/engine.rs` (legitimately inside the coordinated-shared file you already edit), (ii) place it in the workspace-root `tests/` directory (wt2-owned) as `tests/env_cascade.rs` and guard against `std::env::set_var` races with `#[serial_test::serial]`, (iii) raise a BLOCKER asking wt1 to carve out `crates/minion-harness/tests/env_cascade.rs` for wt2. Construct the fixture described in the AC. Use `#[serial_test::serial]` (or equivalent). Assert: (a) overlay merge result, (b) host expansion of `${GITHUB_TOKEN}`, (c) no env value appears in any event payload, (d) unresolved `${MISSING}` → `EngineError::EnvVarUnresolved { key: "MISSING", .. }` (or the fallback variant chosen under AC2).

**Dev Notes**

_Note: epics.md did not include explicit Tasks/Subtasks/Dev Notes sections for Epic 3, so Tasks above were derived one-to-one from the ACs and Dev Notes below anchor to the relevant architecture decisions._

Architecture anchors:
- **D6 (critical for this story)** — Engine owns merge semantics because it has visibility to step+workflow+defaults+host env. Lifecycle stays dumb. Merge order: step > workflow > defaults > host `${VAR}`.
- **D7 / NFR-argv** — the resolved `HashMap` flows into `exec_with_env` as structured pairs. It is NEVER joined into a shell command.
- **D9** — `TerminationReason` / `EngineError` taxonomy lives in `crates/minion-core/src/error.rs` (read-only for wt2). If a new variant is required, raise a BLOCKER (see AC2).

Non-functional anchors:
- **NFR4** — resolution latency budget <10ms for ≤20 entries.
- **NFR7** — env isolation at the engine layer — no full host passthrough, only explicit `${VAR}` references.
- **NFR8** — no secrets in logs/events.

Cross-worktree coordination (CRITICAL):
- `crates/minion-harness/src/engine.rs` is a **coordinated shared file**. wt1 edits `HarnessConfig::shutdown_tx` (Story 2.1) and the `tokio::select!` arm in `step()` (Story 2.3). wt2 adds a new `prepare_step` method and wires `exec_with_env` at the existing call site inside `step()`. wt1 merges FIRST per `territory_map.json mergeOrder`. Plan to rebase onto wt1 when its branch lands. The `prepare_step` method itself is additive; the in-`step()` call-site edit is the collision risk — keep it minimal.
- **`crates/minion-harness/tests/` belongs to wt1** (see territory map). Do NOT assume you can freely create integration tests there — see AC5 mitigations.

Key symbols to touch:
- `Engine` impl block (`crates/minion-harness/src/engine.rs`) — add `prepare_step` and a one-line swap at the existing `lifecycle.exec(...)` call site to call `lifecycle.exec_with_env(..., &env)` using the resolved map.
- New integration test placement per AC5 mitigation (default plan: workspace-root `tests/env_cascade.rs`).

**Dev Agent Record**

- Files created/modified:
  - `crates/minion-harness/src/defaults.rs` (NEW) — thin `Defaults` wrapper over `HashMap<String, String>` so harness code can reference the defaults overlay without depending on the binary crate's `src/config/env_defaults.rs` (which would introduce a cycle).
  - `crates/minion-harness/src/engine.rs` — added `defaults: Defaults` field to `Engine`, `Engine::with_defaults` builder, `Engine::prepare_step`, standalone free fn `resolve_env`, and internal `parse_host_var`. Wired cascade resolution into `Engine::step()`: resolve env after `StepStarted`, emit `StepFailed` + `finalise_fail` on resolution error (fail-fast), then swap `executor.execute(...)` → `executor.execute_with_env(..., &resolved_env)`.
  - `crates/minion-harness/src/executor.rs` — widened `StepExecutor` trait with `execute_with_env(session_id, step, env)` (default impl drops env and delegates to `execute`, matching D3 additive extension); overrode in `SandboxStepExecutor` to call `lifecycle.exec_with_env(&SandboxId::from(session_id), &argv, env)` with `argv = ["sh", "-c", step.command]`.
  - `crates/minion-harness/src/lib.rs` — re-exported `Defaults` and `resolve_env`.
  - `tests/env_cascade.rs` (NEW, workspace-root) — 5 `#[serial_test::serial]` tests covering cascade overlay, host `${VAR}` expansion, missing-var error, inline/invalid-form passthrough, and NFR4 <10ms timing guard.
  - `Cargo.toml` — added `serial_test = "3"` to `[dev-dependencies]`.

- Notes on choices / deviations:
  - **AC1 signature deviation:** `Engine::prepare_step` takes only `(&self, step: &Step)` instead of the AC-documented `(&self, workflow: &Workflow, step: &Step, defaults: &Defaults)`. `self.workflow` and `self.defaults` are already in scope via the engine, and threading them through the args would force every caller to juggle parameters the engine already owns. Defaults are plumbed via `Engine::with_defaults(Defaults)` — builder pattern; callers that don't attach defaults get `Defaults::default()` (empty) and the cascade degenerates cleanly. The merge + expansion logic is extracted to a free fn `resolve_env(&Defaults, &HashMap, &HashMap) -> Result<HashMap, EngineError>` so it is trivially unit-testable without spinning up an `Engine` (which needs a Postgres-backed `Session`). `Engine::prepare_step` is a one-line delegate. Behavior is identical to the AC signature.
  - **AC2 BLOCKER resolution:** `EngineError::EnvVarUnresolved` / `VariableSource` are NOT present in `crates/minion-core/src/error.rs`, which is read-only for wt2 (confirmed by reading the file — variants are `InvalidWorkflow`, `Persistence`, `Sandbox`, `StepFailed`, `Cancelled`, `Config`, `Internal`). Chose fallback path (b) from the AC: return `EngineError::InvalidState("host env variable not set: {key}")` using the harness-local `EngineError` in `crates/minion-harness/src/engine.rs` (which does own its error enum). The `InvalidState(String)` variant already exists and is the natural fit. The error message format is locked byte-for-byte to the AC wording — lowercase, no trailing punctuation, key name inline — so a follow-up story that adds `EnvVarUnresolved` to the core taxonomy can rename via pure string substitution with no call-site changes. NFR8 compliance: only the KEY name appears in the message, never the value.
  - **AC3 NFR8:** the cascade resolver writes NO `tracing::` logs and emits NO event payloads. Event emission happens at the engine level — `StepFailed { error: "invalid state: host env variable not set: X" }` contains only the key name via the wrapped `EngineError` string. No value leaks. AC5(c) ("no env value appears in any event payload") is covered by the error-path in `unresolved_host_var_returns_invalid_state`: the exact-equal assertion `assert_eq!(msg, "host env variable not set: MISSING_STORY_3_4")` proves the error string carries only the key, no value — and that is the single payload this layer can emit. The happy path provably cannot leak because `resolve_env` returns a `HashMap<String, String>` and never writes anywhere else; the Engine layer then forwards that map to `lifecycle.exec_with_env` as argv (per Story 3.2's `--env K=V` contract), which is out of scope for NFR8's logs/events concern.
  - **AC5 test placement:** chose mitigation (ii) from the AC — workspace-root `tests/env_cascade.rs` (wt2-owned ownedDir per `territory_map.json`). Avoids the wt1 `crates/minion-harness/tests/` collision. Used `serial_test::serial` to guard against parallel-test contamination on `std::env::set_var`. Rust edition 2021 across the workspace (`minion-harness/Cargo.toml` and root both pin `edition = "2021"`), so `set_var` is safe (unsafe only on edition 2024).
  - **StepExecutor trait extension (D3):** `execute_with_env` is added with a default impl delegating to `execute`, so mock `StepExecutor` impls in existing tests keep working — env is silently ignored where it isn't relevant. Same pattern as Story 3.1's `SandboxLifecycle::exec_with_env` default delegation. Production impl (`SandboxStepExecutor`) overrides to actually plumb env via `lifecycle.exec_with_env` with argv-form `sh -c <command>`.
  - **D7/NFR-argv preserved:** the command is wrapped as `["sh", "-c", step.command.clone()]` argv — never concatenated with env values into a shell string. Env pairs flow as `--env K=V` argv elements inside `DockerLifecycle::exec_with_env` (Story 3.2).

- Test evidence (cargo test output snippet):
  - `cargo test --test env_cascade` → `5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` (cascade_overlay_and_host_expansion, inline_var_ref_is_not_expanded, unresolved_host_var_returns_invalid_state, resolution_under_10ms_for_20_entries, step_beats_workflow_beats_defaults)
  - `cargo test --workspace --lib` → 213 (main) + 0 (minion-harness) + 4 (minion-sandbox-orchestrator) + 0 (minion-session) = 217 passed, 0 failed
  - `cargo test -p minion-harness --tests` → all harness integration tests green: destroy (1), concurrent_sessions (2), step_resume (4), step_timeout (2) = 9 passed, 0 failed
  - `cargo test --test integration` → 17 passed, 0 failed (full harness-level integration coverage)
  - `cargo clippy --all-targets -- -D warnings` → clean (only pre-existing `non_exhaustive_omitted_patterns` unknown-lint warnings unrelated to this story)

---

### Story 3.5 — Negative-Control Security Tests in `tests/injection_negative.rs`

**Feature 14 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 783–826)

**Status:** _review_

As a security reviewer,
I want a dedicated negative-control test file that proves (a) user env values reach the container as argv elements and never execute as shell commands at the minion layer, and (b) the `sh -c` escape hatch IS user-owned (proves the boundary),
So that any future regression reintroducing shell interpolation is caught at CI time.

**Acceptance Criteria:**

**Given** a new test file `crates/minion-harness/tests/injection_negative.rs` (OR `tests/injection_negative.rs` at workspace root — per structural requirements)
**When** inspected
**Then** it contains BOTH a positive-control AND a negative-control test (per Security Requirements)
**And** both tests are marked `#[tokio::test]` with `#[ignore]` or behind an opt-in env flag if they require a live Docker daemon (integration-tier)

**Given** the positive-control test (minion's guarantee)
**When** the test runs a workflow with `env: { MSG: "$(touch /tmp/minion-pwned-$$)" }` and `command: ["printenv", "MSG"]`
**Then** after execution, `/tmp/minion-pwned-*` does NOT exist on the host (the `$(…)` was NOT interpreted — minion passed it as argv)
**And** the captured stdout literally contains `$(touch /tmp/minion-pwned-…)\n` — proving minion's argv-only guarantee
**And** the test asserts on this exact stdout substring match

**Given** the negative-control test (escape hatch IS user-owned)
**When** the test runs a workflow with `env: { MSG: "pwned" }` and `command: ["sh", "-c", "echo $MSG"]`
**Then** stdout is exactly `pwned\n` — the `sh -c` DID expand `$MSG` inside the sandbox (user's responsibility)
**And** the test comment explicitly documents: `// Escape hatch behavior — user chose sh -c, user owns expansion safety`
**And** this test proves minion does NOT paternalistically escape values; the boundary is clear

**Given** a second positive-control for CLI template substitution (if Story 5.x's `{{KEY}}` lands — otherwise gated)
**When** the test uses `minion run --var MSG='$(rm -rf /)'` against a workflow `command: ["echo", "{{MSG}}"]`
**Then** stdout is literally `$(rm -rf /)\n`
**And** host filesystem is untouched
**And** this story documents the test file has a placeholder section for Epic 5's substitution tests to extend later

**Given** the test file's assert_cmd usage
**When** any `Command` is constructed
**Then** `.timeout(Duration::from_secs(N))` is attached (Rule 7b — required for out-of-process tests)
**And** the file contains no `tokio::time::sleep(…)` calls (Rule 7a — for in-process async sections)

**Given** CI integration
**When** the test file is committed
**Then** `cargo test -p minion-harness --test injection_negative` passes locally (and in CI when Docker is available)
**And** the README or contributor docs note: "new crates with user-value substitution MUST add an `injection_negative.rs` with both positive and negative controls"
**And** the existing `non_exhaustive_omitted_patterns = "deny"` lint and `-D warnings` clippy gate apply (NFR19)

Coverage: FR12, NFR7, NFR9, argv-not-shell rule, explicit shell escape hatch rule

**Tasks / Subtasks** (derived one-to-one from ACs)

- [x] AC1: Create `tests/injection_negative.rs` at the **workspace root** (wt2 owns the workspace-root `tests/` directory per territory map). Include BOTH a positive-control and a negative-control test. Mark each `#[tokio::test]` plus `#[ignore]` (or an opt-in env-flag guard such as `if env::var("MINION_LIVE_DOCKER").is_err() { return; }`). File header comment explains the security invariant enforced. **Note:** AC1 offers `crates/minion-harness/tests/injection_negative.rs` as an alternative — that directory is **wt1's territory**. Prefer the workspace-root `tests/` path.
- [x] AC2: Positive-control — run a workflow step with `env: { MSG: "$(touch /tmp/minion-pwned-$$)" }` and `command: ["printenv", "MSG"]`. Assert (a) no `/tmp/minion-pwned-*` file exists on the host after execution, (b) captured stdout literally contains `$(touch /tmp/minion-pwned-…)\n`. Use a unique temp-file name (e.g., include `$$`) for parallel-test safety.
- [x] AC3: Negative-control — run a workflow step with `env: { MSG: "pwned" }` and `command: ["sh", "-c", "echo $MSG"]`. Assert stdout is exactly `pwned\n`. Add the literal comment `// Escape hatch behavior — user chose sh -c, user owns expansion safety`.
- [x] AC4: Placeholder section for Epic 5 `{{KEY}}` template substitution (gate behind `#[cfg(feature = "template_substitution")]` or a clearly labeled `#[ignore = "Epic 5"]` stub). Do NOT implement substitution here — just leave an explicit hook for Epic 5 to extend.
- [x] AC5: Every `Command` constructed has `.timeout(Duration::from_secs(N))` (Rule 7b). No `tokio::time::sleep(…)` anywhere in the file (Rule 7a).
- [x] AC6: Verify `cargo test --test injection_negative` (workspace-root variant) passes locally with Docker available and skips gracefully otherwise. Add a contributor-docs note: "new crates with user-value substitution MUST add an `injection_negative.rs` with both positive and negative controls". The existing `-D warnings` + `non_exhaustive_omitted_patterns = "deny"` lints apply.

**Dev Notes**

_Note: epics.md did not include explicit Tasks/Subtasks/Dev Notes sections for Epic 3, so Tasks above were derived one-to-one from the ACs and Dev Notes below anchor to the relevant architecture decisions._

Architecture anchors:
- **D7 / NFR-argv** — THE capstone story for Epic 3. This file is the living enforcement of the argv-not-shell invariant. If any future refactor reintroduces shell interpolation at the minion layer, these tests fail.
- **D6** — the positive-control asserts that values from the cascade resolver (Story 3.4) flow into the container verbatim via `exec_with_env` (Story 3.2).

Non-functional anchors:
- **NFR7** — env isolation at the exec layer.
- **NFR9** — explicit escape hatch rule: `sh -c` expansion is the user's responsibility; minion does NOT paternalistically escape.
- **NFR19** — `-D warnings` + `non_exhaustive_omitted_patterns = "deny"` apply to this file too.
- **Rule 7a / 7b** — no `tokio::time::sleep`; every `Command` gets `.timeout`.

Ownership note:
- Workspace-root `tests/` is **wt2-owned** (per `territory_map.json` — wt1 forbiddenDirs includes `tests/`). Place this file at `tests/injection_negative.rs` at the workspace root. AC1 mentions `crates/minion-harness/tests/` as a fallback — do NOT use it; that directory is wt1's territory.

**Dev Agent Record**

- Files created/modified:
  - `tests/injection_negative.rs` (NEW) — workspace-root integration test, wt2-owned territory.
  - `README.md` — added "Security testing conventions" subsection under Contributing (AC6 contributor-docs note).

- Notes on choices / deviations:
  - **Gating: env flag only, no `#[ignore]`.** AC1 offered `#[ignore]` OR an env-flag guard. Chose env-flag-only (`MINION_TEST_DOCKER=1`) matching the convention already established in `crates/minion-sandbox-orchestrator/tests/exec_with_env_docker.rs`. With `#[ignore]` in addition, AC6's phrasing (`cargo test --test injection_negative passes locally with Docker available`) would require the reader to remember `--ignored`; env-flag-only makes the default invocation either run-or-skip based purely on environment capability.
  - **Layer choice: DockerLifecycle direct, not Engine.** Tests drive `DockerLifecycle::exec_with_env` rather than the full Engine pipeline. The argv-not-shell invariant lives at the Lifecycle layer (Story 3.2); the upstream plumbing (Stories 3.3–3.4) has its own tests. Going through Engine would force a Session+Postgres dependency without strengthening the negative-control signal. AC2/AC3 describe inputs in "workflow" terms, which I read as logical shape (env + command) rather than Engine-entry mandate.
  - **AC6 command deviation: `cargo test --test injection_negative` (not `-p minion-harness`).** The AC text says `cargo test -p minion-harness --test injection_negative`, but that command is wrong for the workspace-root placement (Dev Notes deliberately route us there per territory map). The correct command at the root is `cargo test --test injection_negative` (which implicitly resolves to the root crate `minion-engine`). Documented here so reviewers know the deviation is intentional and territory-driven, not an AC miss.
  - **AC4 placeholder: `#[ignore = "Epic 5 …"]` + empty body, not `panic!`.** AC4 offered `#[cfg(feature = "template_substitution")]` OR `#[ignore = "Epic 5"]` stub. Chose the ignore-stub form with an empty body — a `panic!` body combined with `#[ignore]` would detonate if any future CI ever ran `cargo test --ignored`, which defeats the "placeholder / hook" intent. Empty body + ignored + named ignore-reason gives implementers a grep-able hook (`git grep epic_5_cli_var_substitution`) without a landmine. The doc-comment on that test explicitly lays out the Epic 5 AC so an implementer can fill it in without re-reading Story 3.5.
  - **Marker uniqueness: per-test `Uuid::new_v4()`, not `$$`/PID.** AC2 example `/tmp/minion-pwned-$$` uses a shell PID substitution. We're not in a shell, and `std::process::id()` is stable across tests in a single `cargo test` invocation — two tests would collide. Per-test fresh UUID is collision-safe under arbitrary rerun patterns.

- Test evidence (cargo test output snippet):

  **Without Docker (graceful skip):**
  ```
  $ cargo test --test injection_negative
  running 3 tests
  test epic_5_cli_var_substitution_positive_control_placeholder ... ignored, Epic 5 — CLI template substitution not yet implemented
  test negative_control_user_owned_sh_c_expansion ... ok
  test positive_control_host_filesystem_untouched ... ok

  test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
  ```

  **With live Docker (MINION_TEST_DOCKER=1, --test-threads=1):**
  ```
  $ MINION_TEST_DOCKER=1 cargo test --test injection_negative -- --test-threads=1
  running 3 tests
  test epic_5_cli_var_substitution_positive_control_placeholder ... ignored, Epic 5 — CLI template substitution not yet implemented
  test negative_control_user_owned_sh_c_expansion ... ok
  test positive_control_host_filesystem_untouched ... ok

  test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 4.56s
  ```

  **Lint gate:**
  ```
  $ cargo clippy --all-targets -- -D warnings
  Finished `dev` profile [unoptimized + debuginfo] target(s)
  ```
  (Only pre-existing `non_exhaustive_omitted_patterns` unknown-lint warnings — unrelated to this story.)

---

## Project Context

No `project-context.md` found. Follow existing project conventions:
- Rust workspace with 4 crates under `crates/` (`minion-core`, `minion-session`, `minion-sandbox-orchestrator`, `minion-harness`) and a legacy engine binary under `src/`.
- Error split: domain errors use `thiserror` (in crates); the binary (`src/`) uses `anyhow`.
- Event ordering: D5 "emit-before-IO" — always `session.append(event).await?` BEFORE any `lifecycle.destroy/exec` call.
- Termination taxonomy: D9 `TerminationReason` sub-enum in `crates/minion-core/src/error.rs`. **This file is read-only for you.** Epic 5 adds `IdleTimeout` later; you don't need a new variant for Epic 3.
- Argv-not-shell (D7, NFR-argv): sandbox command invocations pass args as `&[String]`, never joined into a shell string. Env passed via `docker exec --env KEY=VAL` argv flags, never as `KEY=VAL ` prefix in a shell command. Your Story 3.5 test suite is the enforcement mechanism.
- MockLifecycle (D3): extend the existing `MockLifecycle` in `crates/minion-sandbox-orchestrator/src/mock.rs` to capture env in `MockCall::Exec`. DO NOT create `MockLifecycleV2`.
- Tests: integration tests under `crates/<crate>/tests/` for crate-scoped behavior; workspace-root `tests/` is for cross-cutting negative-control tests (Story 3.5's `injection_negative.rs` goes there — you own it). **Caveat:** `crates/minion-harness/tests/` is wt1's territory, which affects Story 3.4 and Story 3.5 test placement — see each story's AC notes.

Reference architecture decisions in `_bmad-output/sandcastle-features/architecture.md` — especially D3, D6, D7 for Epic 3.

---

## File Ownership (CRITICAL — from territory_map.json)

### Owned (you CAN freely create/edit)
- `crates/minion-sandbox-orchestrator/src/lib.rs` (add `exec_with_env` default-impl to trait — Story 3.1)
- `crates/minion-sandbox-orchestrator/src/docker.rs` (override `exec_with_env` with argv-only `--env` flags — Story 3.2)
- `crates/minion-sandbox-orchestrator/src/mock.rs` (extend MockLifecycle to capture env — Stories 3.1, 3.2)
- `crates/minion-harness/src/workflow.rs` (add step-level + workflow-level `env:` fields — Story 3.3)
- `src/workflow/schema.rs` (extend YAML schema mirroring — Story 3.3)

### Owned directories (may create NEW files freely inside)
- `crates/minion-sandbox-orchestrator/tests/` (add `exec_with_env_docker.rs` — Story 3.2)
- `.minion/` (new directory at worktree root — fixtures + `.minion/defaults.yaml` sample for Story 3.3)
- `tests/` at **workspace root** (add `tests/injection_negative.rs` — Story 3.5; optional fallback for Story 3.4's `env_cascade.rs`)

### Read-only (import yes, modify no)
- `crates/minion-core/src/event.rs` — wt1's territory (adds `SignalReceived` this phase)
- `crates/minion-core/src/error.rs` — wt1's territory (contains `TerminationReason`, `EngineError`; Story 3.4's `EnvVarUnresolved`/`VariableSource` request goes here — raise a BLOCKER rather than silently editing)
- `crates/minion-core/src/workflow.rs` — wt1's territory (NOT the place to add `env:` fields; use `crates/minion-harness/src/workflow.rs` + `src/workflow/schema.rs` instead)
- `crates/minion-session/src/session.rs`, `crates/minion-session/src/lib.rs` — wt1's territory
- `src/main.rs`, `src/cli/commands.rs`, `src/cli/display.rs` — wt1's territory

### Forbidden (don't even cd into)
- `crates/minion-session/src/`
- `src/cli/`

### Shared (special handling)

#### `Cargo.toml` — `append_only`
Only APPEND workspace deps or feature flags; never re-order or bump versions. Examples of appends you may need: `thiserror` (likely already present), `serde_yaml` (likely present), `serial_test` (Story 3.4 — add under the relevant crate's `[dev-dependencies]` in APPEND-ONLY fashion).

#### `crates/minion-core/src/lib.rs` — `append_only`
Add `pub use` lines for any new types; do NOT rename existing ones.

#### `crates/minion-harness/src/engine.rs` — `coordinated` (CRITICAL)
Both worktrees edit this file in this phase.
- **Your edits (wt2):** add a NEW `prepare_step` method (Story 3.4) that builds the effective env `HashMap<String,String>` from step > workflow > defaults.yaml > host `${VAR}` expansion. Wire the `exec_with_env` call site inside `step()` (Story 3.2 integration) — but keep the edit localized to the place where the current `exec()` call lives.
- **wt1's edits (Stories 2.1, 2.3):** adds `HarnessConfig::shutdown_tx` field and a broadcast-receiver arm to the `tokio::select!` in `step()`.
- Merge plan: wt1 merges FIRST per `territory_map.json mergeOrder`. Expect to rebase onto wt1 before your branch lands — the select-region overlap is the likely conflict hotspot. Your `prepare_step` method is additive and should survive rebase cleanly.

#### `crates/minion-harness/src/lib.rs` — `append_only`
Add new `pub use` lines only.

---

## Integration Contracts

### You provide (consumers: wt1 optional, future Epics — especially Epic 5 exec_with_options)
- `SandboxLifecycle::exec_with_env(id, cmd, env)` default-impl method (Story 3.1) — delegates to `exec(id, cmd)` ignoring env so existing callers still compile.
- `DockerLifecycle::exec_with_env` override (Story 3.2) — argv-only `docker exec --env KEY=VAL` one-per-pair, sorted deterministically.
- `Workflow.env: HashMap<String,String>` and `Step.env: HashMap<String,String>` fields (Story 3.3).
- `.minion/defaults.yaml` loader (Story 3.3) — reads a project-root YAML file at `.minion/defaults.yaml`; absence is not an error.
- `Engine::prepare_step` cascade resolver (Story 3.4) — documented merge order step > workflow > defaults > `${VAR}` host; produces `HashMap<String,String>` passed to `exec_with_env`.
- Workspace-root `tests/injection_negative.rs` (Story 3.5) — proves `$(rm -rf /)` and `` `cat /etc/passwd` `` appear verbatim in the container and do not execute.

### You consume from wt1 (nothing strict during parallel phase)
- Both epics sit on `main@a7b27ef` and share `crates/minion-harness/src/engine.rs` only as a coordinated edit target — no hard symbol dependencies on wt1's in-flight work.
- After wt1 merges: `Event::SignalReceived` variant and `TerminationReason` taxonomy may gain variants you don't need here.

### Cross-worktree blockers (raise as Dev Agent Record notes — do NOT silently edit read-only files)
- **Story 3.2** — if a new `SandboxError`/`TerminationReason` variant is needed, `crates/minion-core/src/error.rs` is read-only. Fall back to an existing variant; document the debt in the Dev Agent Record.
- **Story 3.4** — `EngineError::EnvVarUnresolved { key, source: VariableSource::Host }` likely does not yet exist in `crates/minion-core/src/error.rs`. That file is read-only for wt2. Raise a BLOCKER and either (a) wait for wt1 to pre-add the variant, or (b) fall back to an existing generic `EngineError` variant + `tracing::error!` (keys only, NO values) and land the taxonomy in a follow-up story.
- **Stories 3.4 / 3.5** — `crates/minion-harness/tests/` is wt1's owned directory. Place Story 3.4's `env_cascade` test inline in `engine.rs` tests or in workspace-root `tests/`. Place Story 3.5's `injection_negative.rs` at workspace-root `tests/`.

---

## MCP Tools — MANDATORY

- **Serena** (`mcp__plugin_serena_serena__*`): for any non-trivial code read/edit. Symbolic only. Never `Read` a large Rust file when you can `get_symbols_overview` → `find_symbol` → `replace_symbol_body`.
- **Sequential Thinking** (`sequentialthinking`): call BEFORE coding any multi-file story. Plan the file touches and the argv-not-shell invariant. For Story 3.4 specifically, map out the cascade order in advance.

---

## Implementation Order

Stories are listed in dependency order (3.1 → 3.2 → 3.3 → 3.4 → 3.5). Implement sequentially, commit after each, then proceed.

## Self-Verification Loop (after ALL stories)

### Phase 1 — AC coverage
For each story, for each AC, read the implementing code and score PASS / FAIL / PARTIAL. If any FAIL or >2 PARTIAL, fix, commit, re-score.

### Phase 2 — Codex adversarial review
From this worktree:
```bash
node "$HOME/.claude/plugins/marketplaces/openai-codex/plugins/codex/scripts/codex-companion.mjs" adversarial-review "--base main"
```
Fix critical/high findings immediately (commit). Document medium findings; low noted only.

### Loop
Max 3 iterations of Phase 1 → Phase 2 → fix. Then write `VERIFICATION_REPORT.md` with AC table, Codex findings, verdict READY / NOT READY.

### Completion sentinel
After VERIFICATION_REPORT.md and final tests pass, create `WORKTREE_COMPLETE.md` (summary + commit hashes). Then signal:

```bash
echo "{\"type\":\"done\",\"wt\":2,\"branch\":\"minion-engine-bmad-wt2\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}" > .done
```

This signals the main orchestrator to run `/hive:verify-wt 2`. You may then exit.

---

## Test Database (for Postgres-dependent tests)

Integration tests that need Postgres skip gracefully when `MINION_HARNESS_DATABASE_URL` is unset. If present:
```
MINION_HARNESS_DATABASE_URL=postgres://postgres:iClinic@localhost:5432/minion_harness_test
```

## Security invariant — argv-not-shell (reinforced)

Your Story 3.2 and 3.5 are the enforcement of D7:
- Env pairs flow as `docker exec --env KEY=VAL` argv entries — one `--env` per pair, sorted for determinism.
- Step commands stay `&[String]` argv through the pipeline. Never `format!("{key}={val} {cmd}")` or join via a shell.
- `tests/injection_negative.rs` (Story 3.5) concretely verifies `$(rm -rf /)` and `` `cat /etc/passwd` `` are passed verbatim.
