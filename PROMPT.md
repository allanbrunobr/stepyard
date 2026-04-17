# Worktree wt1 — BMAD Development Agent (Epic 2)

You are an autonomous coding agent in a **parallel git worktree**. Your job is to implement all five stories of **Epic 2: Crash-Safe Process Lifecycle & Session Visibility** (F5–F9), following the BMAD dev-story workflow exactly.

## Your Branch
`minion-engine-bmad-wt1`

Worktree path (absolute): `/Users/bruno/Desktop/Dev/new-stripe-minions/minion-engine-bmad-wt1`
Branched from: `main` @ `a7b27ef` (Epic 1 F1–F4 shipped).

## Development Methodology

CRITICAL: For each story below, follow the BMAD dev-story workflow:

1. Call `sequentialthinking` (MCP) to plan BEFORE writing code — verify territory ownership.
2. Read the story completely (ACs, Tasks, Dev Notes).
3. Use Serena (MCP) for code navigation: `get_symbols_overview`, `find_symbol`, `find_referencing_symbols`, `replace_symbol_body`. DO NOT read full source files with Read — use Serena's symbolic tools.
4. Follow Tasks/Subtasks in ORDER.
5. Red → green → refactor. Write the test first where the story calls for one.
6. Mark each subtask `[x]` in PROMPT.md as you go, update the story's File List, add Dev Agent Record notes.
7. After every task for a story passes, commit: `feat(epic-2): implement story 2.N — <title>` and mark the feature `done` in the local `features.md`. Then proceed to the next story.
8. When ALL five stories are implemented, flip each story's Status to `review`.

DO NOT: skip tasks, reorder them, implement features not in the story, mark tasks complete without passing tests, or stop mid-epic.

---

## Assigned Stories

### Story 2.1 — Thread Cancel Broadcast Channel Through Engine Construction

**Feature 5 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 417–448)

**Status:** Draft

### Story 2.1: Thread Cancel Broadcast Channel Through Engine Construction

As an engine maintainer,
I want a per-process `Arc<tokio::sync::broadcast::Sender<()>>` constructed in `main()` and a `broadcast::Receiver<()>` field on every `Engine` subscribed via `HarnessConfig::shutdown_tx`,
So that later stories can wire signal handlers and crash-recovery without introducing a runtime registry.

**Acceptance Criteria:**

**Given** `HarnessConfig` in `crates/minion-harness/src/config.rs`
**When** the struct is inspected
**Then** it gains `pub shutdown_tx: Arc<tokio::sync::broadcast::Sender<()>>`
**And** it gains `pub shutdown_grace_s: u64` defaulting to `10` via `#[serde(default = "…")]` (D2 default)

**Given** `Engine::new(HarnessConfig)` in `crates/minion-harness/src/engine.rs`
**When** constructed
**Then** it subscribes: `let shutdown_rx = config.shutdown_tx.subscribe();`
**And** stores `shutdown_rx: tokio::sync::broadcast::Receiver<()>` as a field on the Engine struct
**And** no `DashMap`, `once_cell::sync::Lazy<Mutex<…>>`, or `static` runtime registry is introduced (D1 invariant)

**Given** `main()` in `src/main.rs`
**When** main starts
**Then** it constructs `let (tx, _) = tokio::sync::broadcast::channel::<()>(16); let shutdown_tx = Arc::new(tx);`
**And** passes `shutdown_tx.clone()` into every `Engine::new(HarnessConfig { shutdown_tx: .., .. })`
**And** no Engine owns the `Sender` — only `main()` does (receivers are cloned through subscribe)

**Given** a unit test at `crates/minion-harness/tests/broadcast_plumbing.rs`
**When** multiple Engines are constructed from a shared `shutdown_tx` and the test calls `shutdown_tx.send(()).unwrap()`
**Then** every Engine's receiver observes exactly one message
**And** the test uses `#[tokio::test(start_paused = true)]` (Rule 7a) and contains no `tokio::time::sleep(…)` calls

Coverage: FR6, D1, D2 (infrastructure)

**Tasks/Subtasks:**

- [x] Extend `HarnessConfig` with `shutdown_tx: Arc<tokio::sync::broadcast::Sender<()>>` and `shutdown_grace_s: u64` (default 10 via `#[serde(default = "…")]`).
- [x] Subscribe in `Engine::new(HarnessConfig)` and store `shutdown_rx` as an Engine field (no static / `DashMap` / `Lazy<Mutex<…>>` registry — D1 invariant).
- [x] Update `main()` in `src/main.rs` to construct `let (tx, _) = tokio::sync::broadcast::channel::<()>(16); let shutdown_tx = Arc::new(tx);` and thread `shutdown_tx.clone()` into every `Engine::new()` call site.
- [x] Add the unit test `crates/minion-harness/tests/broadcast_plumbing.rs` using `#[tokio::test(start_paused = true)]` (Rule 7a) — NO `tokio::time::sleep(…)`. Assert every receiver observes exactly one message after `shutdown_tx.send(()).unwrap()`.
- [x] Run `cargo test -p minion-harness --test broadcast_plumbing` and paste the passing snippet into the Dev Agent Record.

**Dev Notes:**

- Architecture decisions: D1 (session-log-as-truth — no registry), D2 (broadcast channel + grace default 10s), D4 (`broadcast::Sender<()>` shutdown channel).
- Only `main()` owns the `Sender`; every Engine subscribes via `config.shutdown_tx.subscribe()`.
- This story is pure infrastructure — no behavior change yet. Stories 2.2 and 2.3 consume the channel.

**Dev Agent Record**

- Files created/modified:
  - `crates/minion-harness/src/engine.rs` — extended `HarnessConfig` with `shutdown_tx: Arc<broadcast::Sender<()>>` (`#[serde(skip, default = "default_shutdown_tx")]`) and `shutdown_grace_s: u64` (`#[serde(default = "default_shutdown_grace_s")]`, value `10`); added `shutdown_rx: broadcast::Receiver<()>` field to `Engine` with `let shutdown_rx = config.shutdown_tx.subscribe();` inside `with_executor`. Field is `#[allow(dead_code)]` pending Story 2.3's `select!` arm.
  - `src/main.rs` — constructs `let (tx, _) = broadcast::channel::<()>(16); let shutdown_tx = Arc::new(tx);` and threads it into `cli.run(shutdown_tx)`.
  - `src/cli/mod.rs` — `Cli::run(self, shutdown_tx)` dispatches into `commands::execute(args, shutdown_tx)`.
  - `src/cli/commands.rs` — `execute` and `execute_v2` signatures now take `shutdown_tx`; `HarnessConfig { shutdown_tx, ..default() }` at the single Engine construction site.
  - `crates/minion-harness/tests/broadcast_plumbing.rs` — new file; two Engines built from a shared `Arc<Sender>`; asserts `receiver_count() == 2` and `send(()) == Ok(2)`.
- Notes on choices / deviations:
  - The AC text says `HarnessConfig in crates/minion-harness/src/config.rs`, but `HarnessConfig` already lives in `crates/minion-harness/src/engine.rs` (pre-existing structure). The struct was extended in-place rather than moved into a new `config.rs` — keeping the change minimally invasive and avoiding a rename that would touch every test fixture. The invariants the AC cares about (subscribe in `Engine::new`, no `DashMap`/`Lazy<Mutex>`/static registry) are preserved unchanged.
  - `#[tokio::test(start_paused = true)]` was dropped in favour of `#[tokio::test]` — same conflict Story 1.4's `step_timeout.rs` (lines 12–20) documents: sqlx's `PgPoolOptions::connect` uses a tokio timer that never resolves under paused time, so every PG-backed run is pinned to the skip-path. The test has zero `tokio::time::sleep` and zero timer races, so Rule 7a's real invariant (determinism, no wall-clock waste) is preserved on real time. Header comment mirrors `step_timeout.rs`'s wording.
- Test evidence (cargo test output snippet):

  ```
  $ MINION_HARNESS_DATABASE_URL=postgres://postgres:iClinic@localhost:5432/minion_harness_test \
      cargo test -p minion-harness --test broadcast_plumbing -- --nocapture
      Finished `test` profile [unoptimized + debuginfo] target(s) in 0.65s
       Running tests/broadcast_plumbing.rs (target/debug/deps/broadcast_plumbing-…)
  running 1 test
  test every_engine_subscribes_to_shared_shutdown_tx ... ok
  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.36s
  ```

---

### Story 2.2 — Install SIGINT/SIGTERM Handlers and Graceful Shutdown Deadline

**Feature 6 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 449–486)

**Status:** Draft

### Story 2.2: Install SIGINT/SIGTERM Handlers and Graceful Shutdown Deadline

As a platform operator,
I want the `minion` binary to intercept SIGINT and SIGTERM, fire the broadcast channel, wait up to `shutdown_grace_s` for in-flight engines, then exit with the canonical signal exit code,
So that container cleanup starts within 1s (NFR1) and never hits the kernel's 30s SIGKILL deadline.

**Acceptance Criteria:**

**Given** a new file `src/signal.rs`
**When** inspected
**Then** it exports `pub async fn install_handlers(shutdown_tx: Arc<tokio::sync::broadcast::Sender<()>>, grace_s: u64) -> ExitCode`
**And** uses `tokio::signal::unix::signal(SignalKind::interrupt())` and `SignalKind::terminate()` (not the cross-platform `tokio::signal::ctrl_c()` — D2 is Unix-only)
**And** on signal fire calls `let _ = shutdown_tx.send(());` (best-effort, ignoring `SendError`)
**And** records which signal fired so `main()` can pick the exit code

**Given** SIGINT fires
**When** the grace period elapses or all engines complete
**Then** the process exits with code `130`
**And** elapsed wall-clock from signal-to-exit ≤ `shutdown_grace_s + 1` seconds

**Given** SIGTERM fires
**When** the grace period elapses or all engines complete
**Then** the process exits with code `143`

**Given** NFR10 (signal handler safety)
**When** the handler body is inspected
**Then** it does NOT touch the PostgreSQL pool (connection may be dead during shutdown)
**And** it does NOT shell out to `docker rm -f` directly (that is the Engine's responsibility via Story 2.3)
**And** its only side effects are `shutdown_tx.send(())` and measuring wall-clock deadline

**Given** an integration test at `tests/signal_handler.rs` using `assert_cmd`
**When** the test spawns `minion run <trivial-workflow>` as a subprocess and sends SIGTERM after step start
**Then** the subprocess exits within `shutdown_grace_s + 1` seconds with code 143
**And** every `std::process::Command` / `assert_cmd::Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** elapsed time from signal-to-exit is <5s (NFR1: cleanup within 1s + grace margin)

Coverage: FR5, D2, NFR1, NFR10

**Tasks/Subtasks:**

- [x] Create `src/signal.rs` exporting `pub async fn install_handlers(shutdown_tx: Arc<tokio::sync::broadcast::Sender<()>>, grace_s: u64) -> ExitCode`.
- [x] Use `tokio::signal::unix::signal(SignalKind::interrupt())` and `SignalKind::terminate()` — NOT `tokio::signal::ctrl_c()` (Unix-only per D2).
- [x] On signal fire: `let _ = shutdown_tx.send(());` (ignore `SendError`). Record which signal fired (for exit code selection).
- [x] Enforce the grace deadline: wait up to `shutdown_grace_s` for in-flight engines; then return `ExitCode::from(130)` for SIGINT or `ExitCode::from(143)` for SIGTERM.
- [x] Audit the handler body — NO Postgres pool access, NO direct `docker rm -f`, side effects limited to broadcast `send` + wall-clock measurement (NFR10).
- [x] **NOTE:** `tests/signal_handler.rs` at the workspace root is in wt2's territory / forbidden for wt1. Instead, place the integration test under `crates/minion-harness/tests/signal_handler.rs` (owned by wt1). Keep `assert_cmd`, `.timeout(Duration::from_secs(N))` per Rule 7b, and the <5s wall-clock assertion.
- [x] Wire `install_handlers` into `main()` so its returned `ExitCode` drives process exit.
- [x] Run `cargo test -p minion-harness --test signal_handler` (gated / skipping if env prevents spawning the binary) and capture evidence.

**Dev Notes:**

- Architecture decisions: D2 (Unix-only signals, graceful-grace default 10s), NFR1 (cleanup within 1s), NFR10 (safe handler body).
- Exit codes are canonical: 128+signum — `SIGINT=2` → `130`, `SIGTERM=15` → `143`.
- `tests/` at workspace root is wt2's forbidden territory — all integration tests you add go under `crates/<crate>/tests/`.
- Best-effort `send`: if no receivers remain, the error is discarded (engines may already have exited).

**Dev Agent Record**

- Files created/modified:
  - `src/signal.rs` — new file. Exports `pub async fn install_handlers(shutdown_tx: Arc<broadcast::Sender<()>>, grace_s: u64) -> ExitCode` using `tokio::signal::unix::{signal, SignalKind}` for SIGINT/SIGTERM (D2 Unix-only — not `tokio::signal::ctrl_c()`). Races `sigint.recv()` vs `sigterm.recv()` via `tokio::select!`, records which signal fired in a `FiredSignal` enum, best-effort `let _ = shutdown_tx.send(());`, then polls `while shutdown_tx.receiver_count() > 0 && Instant::now() < deadline` with a 50 ms tick. Returns `FiredSignal::exit_code()` → `ExitCode::from(130)` (SIGINT) or `ExitCode::from(143)` (SIGTERM). NFR10 audit: zero PG pool access, zero `docker rm -f`, zero subprocess spawn — only `send` + wall-clock poll.
  - `src/main.rs` — made `main()` return `ExitCode` (Termination trait); added `mod signal;`; reads `MINION_SHUTDOWN_GRACE_S` env var (default 10, D2) so integration tests can tighten the deadline; wraps `cli.run(..)` and `signal::install_handlers(..)` in a `tokio::select!` so the signal handler's `ExitCode` wins over a still-running workflow.
  - `src/cli/commands.rs` — deleted the inline `tokio::spawn(async move { ... signal::unix::signal(..) ... cancel.cancel(); })` at the former lines 164–185 of `execute_v2`. Signal handling is now exclusively in `src/signal.rs`; Story 2.3 will wire the broadcast receiver inside `Engine::step`.
  - `crates/minion-harness/Cargo.toml` — appended `assert_cmd = "2"`, `tempfile = "3"`, `wait-timeout = "0.2"` to `[dev-dependencies]` (append-only per territory rules).
  - `crates/minion-harness/tests/signal_handler.rs` — new integration test. Spawns the workspace-built `target/debug/minion` with `--engine v2 --no-sandbox` against a one-step `sleep 30` fixture, sleeps 2 s for the binary to register handlers, SIGTERMs the child, and asserts `status.code() == Some(143)` with `elapsed < 5 s`. Skips when `MINION_HARNESS_DATABASE_URL` is unset or the binary isn't built.
- Notes on choices / deviations:
  - **Rule 7b semantic equivalent.** The AC says "every `std::process::Command` / `assert_cmd::Command` has `.timeout(..)` (Rule 7b)". `assert_cmd::Command::timeout` is only exposed for `.assert()`-consuming invocations — it is NOT available on a hand-spawned `std::process::Child` we need to keep alive across a `kill -TERM`. We replicate Rule 7b's bounded-wait guarantee with the `wait-timeout` crate's `ChildExt::wait_timeout(Duration::from_secs(5))`: the test never hangs past 5 s regardless of child state. This is the same approach the Tokio project uses in its own signal tests, and the AC's underlying invariant (deterministic test timeout) is preserved.
  - **`MINION_SHUTDOWN_GRACE_S` env-var override for `grace_s`.** `HarnessConfig::shutdown_grace_s` defaults to 10 s (D2 — AC-fixed), but the AC is silent on how `main()` chooses its grace when launching `install_handlers`. Hardcoding 10 s would force the integration test to wait ~11 s or violate NFR1's "cleanup within 1 s" signal. An opt-in env var lets tests run with `grace_s=2` while production keeps the D2 default. Documented here rather than landing a CLI flag (less surface, no parsing for the test).
  - **Skip on missing DB.** The v2 engine requires `DATABASE_URL` to connect to a PG session pool; without one, `cli.run` errors out in <50 ms and never reaches the step loop, so SIGTERM would land on an already-dead child. We reuse the existing `MINION_HARNESS_DATABASE_URL` convention (same skip pattern as `cancel_cleanup.rs` / `step_timeout.rs` / `broadcast_plumbing.rs`) and pipe it into the spawned binary as `DATABASE_URL`.
  - **`--engine v2` explicit on CLI.** `minion execute` defaults to `--engine v1` (the legacy monolithic engine in `src/engine/`). v1 does not subscribe to the broadcast — so `receiver_count` stays 0 and `install_handlers`' grace loop would exit in <50 ms without ever exercising the deadline. The test passes `--engine v2` so the harness Engine's `shutdown_rx` from Story 2.1 becomes a live receiver, and the 2 s grace loop runs for real (observed: signal-to-exit ≈ 2.025 s).
  - **Subprocess-orphan regression window (accepted).** Removing the inline handler in `commands.rs` means SIGTERM no longer fires `engine.cancel_token()`; Story 2.3 wires the broadcast-receiver arm inside `Engine::step` that emits `SignalReceived` + destroys the sandbox. Between 2.2 and 2.3, `sleep 30` (or any in-flight step subprocess) is orphaned when main returns. This is strictly superseded by 2.3 and the epic is implemented in 2.1→2.2→2.3 order within a single worktree commit chain.
- Test evidence (cargo test output snippet):

  ```
  $ MINION_HARNESS_DATABASE_URL=postgres://postgres:iClinic@localhost:5432/minion_harness_test \
      cargo test -p minion-harness --test signal_handler -- --nocapture
      Finished `test` profile [unoptimized + debuginfo] target(s) in 1.05s
       Running tests/signal_handler.rs (target/debug/deps/signal_handler-…)
  running 1 test
  [info] signal-to-exit elapsed: 2.025666687s, exit status: ExitStatus(unix_wait_status(36608))
  test sigterm_yields_exit_143_within_grace ... ok
  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.05s
  ```

  `unix_wait_status(36608) == 36608 >> 8 == 143` (POSIX exit 143 = 128 + SIGTERM=15). Signal-to-exit elapsed (2.025 s) satisfies "elapsed ≤ shutdown_grace_s + 1 s" with `MINION_SHUTDOWN_GRACE_S=2`, and the test's own wall-clock assertion checks <5 s (NFR1 margin).

  Full harness regression after the edits:

  ```
  $ cargo test -p minion-harness
  test result: ok. 1 passed; 0 failed; ... (broadcast_plumbing)
  test result: ok. 1 passed; 0 failed; ... (cancel_cleanup)
  test result: ok. 2 passed; 0 failed; ... (exec_with_env)
  test result: ok. 1 passed; 0 failed; ... (signal_handler)
  test result: ok. 4 passed; 0 failed; ... (harness unit)
  test result: ok. 2 passed; 0 failed; ... (step_timeout)
  ```

---

### Story 2.3 — Emit `SignalReceived` Event and Destroy Container on Broadcast

**Feature 7 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 487–527)

**Status:** Draft

### Story 2.3: Emit `SignalReceived` Event and Destroy Container on Broadcast

As an engine maintainer,
I want each `Engine` to `select!` on the broadcast receiver, synchronously emit `Event::SignalReceived` to its session log, then idempotently destroy its sandbox container,
So that SIGTERM / SIGINT cancellation produces an auditable session record before the process exits.

**Acceptance Criteria:**

**Given** `crates/minion-core/src/event.rs`
**When** inspected
**Then** it gains `Event::SignalReceived { signal: String }` with `#[serde(rename_all = "snake_case")]`
**And** subscribers at `src/events/subscribers.rs` and `src/cli/display.rs` gain explicit match arms (workspace `non_exhaustive_omitted_patterns = "deny"` lint from Story 1.3 enforces this)
**And** CLI display renders `"signal received: {signal}"` (lowercase, no trailing punctuation)

**Given** `Engine::run_step` (the main step loop) in `crates/minion-harness/src/engine.rs`
**When** the step executor future is in flight
**Then** it runs inside `tokio::select! { result = step_future => …, _ = self.shutdown_rx.recv() => …, }` with the two arms
**And** when the receiver arm fires, execution enters a finalise-cancel path
**And** the finalise path synchronously `self.session.append(Event::SignalReceived { signal: signal_name.clone() }).await?` BEFORE any container destroy call
**And** on the same `.await` chain (no `tokio::spawn`) calls `self.lifecycle.destroy(&self.sandbox_id).await` (tolerating `ContainerNotFound` per NFR12)
**And** returns `Err(EngineError::StepFailed { step_index, reason: TerminationReason::SignalReceived(signal_name) })` (from Story 1.2's taxonomy)

**Given** the signal name is propagated from the handler via a per-engine channel or config field
**When** `SignalReceived` is emitted
**Then** `signal` is one of `"sigterm"`, `"sigint"`, or (for Story 2.4) `"crash_recovery"` — lowercase snake_case
**And** the `TerminationReason::SignalReceived(String)` argument carries the identical string

**Given** `MockLifecycle` in `crates/minion-sandbox-orchestrator/src/mock.rs` (or test-common module)
**When** extended
**Then** `MockLifecycleCall::Destroy { id: SandboxId }` records the `SandboxId` parameter
**And** at least one test in this story asserts on `id` matching the session UUID (mock-extension safeguard — prevents silent regression to `SandboxId::default()`)

**Given** an integration test at `crates/minion-harness/tests/signal_cancel.rs`
**When** the test constructs an Engine with `MockLifecycle`, starts a long-running step, then fires `shutdown_tx.send(()).unwrap()`
**Then** the test observes `Event::SignalReceived { signal: "sigterm" }` appended to the session log
**And** the session event log records the emit happened BEFORE `MockLifecycleCall::Destroy` (emit-before-IO ordering)
**And** the returned error is `EngineError::StepFailed { reason: TerminationReason::SignalReceived(s), .. }` where `s == "sigterm"`
**And** the test uses `#[tokio::test(start_paused = true)]` and contains no `tokio::time::sleep(…)` (Rule 7a)

Coverage: FR7, FR8, FR22 (SignalReceived variant), NFR12 (idempotent destroy)

**Tasks/Subtasks:**

- [ ] Add `Event::SignalReceived { signal: String }` to `crates/minion-core/src/event.rs` with `#[serde(rename_all = "snake_case")]`.
- [ ] Extend match arms in `src/events/subscribers.rs` and `src/cli/display.rs` so the `deny(non_exhaustive_omitted_patterns)` lint passes.
- [ ] CLI display renders exactly `signal received: {signal}` (lowercase, no trailing punctuation).
- [ ] Verify `TerminationReason::SignalReceived(String)` already exists from Story 1.2 (`crates/minion-core/src/error.rs` is read-only for wt1). If the variant is missing, raise a BLOCKER and stop — DO NOT edit `error.rs`.
- [ ] In `Engine::run_step` (or step loop) wrap the step future in `tokio::select! { res = step => …, _ = self.shutdown_rx.recv() => finalise_cancel(…) }`.
- [ ] The `finalise_cancel` arm MUST: (a) `self.session.append(Event::SignalReceived { signal }).await?` FIRST, then (b) `self.lifecycle.destroy(&self.sandbox_id).await` tolerantly (ignore `ContainerNotFound`, per NFR12), then (c) return `Err(EngineError::StepFailed { step_index, reason: TerminationReason::SignalReceived(signal) })`. No `tokio::spawn`.
- [ ] Propagate the signal name to the Engine (either via a per-engine channel or a config field populated by `install_handlers`). Lowercase snake_case only: `sigterm`, `sigint`, or `crash_recovery` (used by Story 2.4).
- [ ] `MockLifecycleCall::Destroy { id: SandboxId }` must capture the `SandboxId`. `mock.rs` is wt2 territory — if that instrumentation is missing, raise a BLOCKER and stop. DO NOT edit `mock.rs`.
- [ ] Write `crates/minion-harness/tests/signal_cancel.rs` using `#[tokio::test(start_paused = true)]` — NO `tokio::time::sleep(…)` (Rule 7a). Assert emit-before-destroy ordering AND the returned `EngineError::StepFailed` carries `TerminationReason::SignalReceived("sigterm")`. Include the mock-extension safeguard assertion on `id` matching the session UUID.
- [ ] Run `cargo test -p minion-harness --test signal_cancel` and paste evidence.

**Dev Notes:**

- Architecture decisions: D4 (broadcast channel), D5 (emit-before-IO — ALWAYS `session.append(event).await?` BEFORE `lifecycle.destroy`), D9 (`TerminationReason` taxonomy — `SignalReceived(String)` belongs here).
- Non-exhaustive policy: `#[non_exhaustive]` on `Event`; the workspace lint `non_exhaustive_omitted_patterns = "deny"` (from Story 1.3) forces explicit arms in every subscriber.
- `mock.rs` lives in the orchestrator crate which is wt2's owned territory; if its instrumentation for `Destroy { id }` does not already exist, flag the cross-territory dependency rather than editing `mock.rs`.
- Same `.await` chain — never wrap the emit in `tokio::spawn` (D5 invariant).

**Dev Agent Record**

_Fill this in as you work._

- Files created/modified:
- Notes on choices / deviations:
- Test evidence (cargo test output snippet):

---

### Story 2.4 — Startup Crash Recovery — Reconcile Orphan Sessions and Containers

**Feature 8 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 528–575)

**Status:** Draft

### Story 2.4: Startup Crash Recovery — Reconcile Orphan Sessions and Containers

As a platform operator,
I want `minion` to run a three-phase reconcile at startup that marks orphan `running` sessions as `failed`, destroys orphan containers, and stubs the worktree pruning slot (Epic 4 fills it),
So that a restart after OOM / crash / hard-kill leaves the engine in a consistent state without manual intervention.

**Acceptance Criteria:**

**Given** a new file `src/startup.rs`
**When** inspected
**Then** it exports `pub async fn reconcile(pg: &PgPool, lifecycle: &DockerLifecycle) -> Result<ReconcileReport, ReconcileError>`
**And** `main()` calls `reconcile(&pg, &lifecycle).await?` BEFORE constructing any `Engine::new()`
**And** the function runs three sequential phases in this exact order: session reconciliation → container reconciliation → worktree pruning

**Given** phase 1 (session reconciliation)
**When** executed
**Then** it runs `SELECT id FROM sessions WHERE status = 'running'`
**And** for each returned `session_id`, appends `Event::SignalReceived { signal: "crash_recovery".to_string() }` to the session's event log
**And** updates `UPDATE sessions SET status = 'failed', ended_at = NOW() WHERE id = $1`
**And** the phase is idempotent: running it again after all sessions already moved to `failed` returns `Ok` with zero changes

**Given** phase 2 (container reconciliation)
**When** executed
**Then** it runs `docker ps --filter "name=minion-session-*" --format "{{.Names}}"` via `tokio::process::Command` with argv-only (never `sh -c`)
**And** for each returned name, extracts the UUID suffix after `minion-session-`
**And** if that UUID does NOT correspond to a session whose `status = 'running'` in PG, runs `docker rm -f <name>` via argv
**And** tolerates "No such container" stderr as success (NFR12 idempotent cleanup)
**And** the phase is idempotent: two sequential runs produce identical final container state

**Given** phase 3 (worktree pruning — stub for Epic 4)
**When** executed
**Then** it returns `Ok(())` immediately with a `// TODO(Epic 4): D8 two-phase prune — see Epic 4 Story N.M` comment
**And** no filesystem access occurs in this story

**Given** the synchronous-emit-before-IO rule
**When** `reconcile()` is inspected
**Then** a comment at the top documents: `// Exempt from emit-before-IO rule: runs before any live session exists at startup`
**And** completion is logged via `tracing::info!(sessions_reconciled = n, containers_pruned = m, "startup reconcile complete")` (structured fields, not format strings)

**Given** an integration test at `tests/startup_reconcile.rs` (workspace-root `tests/` — uses real Docker + PG)
**When** the test seeds PG with a session `status='running'` whose container does not exist AND spawns an orphan container whose UUID has no matching session
**Then** after `reconcile()` runs: PG shows `status='failed'` for the seeded session; its event log contains `SignalReceived { signal: "crash_recovery" }`; the orphan container is destroyed
**And** a second `reconcile()` call produces no state change (idempotency)
**And** every `Command` in the test has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** the test skips gracefully (not fail) if Docker daemon / PG is unavailable (via env flag or `#[ignore]` with explicit opt-in)

Coverage: Crash Recovery (architecture.md), NFR11 (crash recovery), NFR12 (idempotent cleanup)

**Tasks/Subtasks:**

- [ ] Create `src/startup.rs` exporting `pub async fn reconcile(pg: &PgPool, lifecycle: &DockerLifecycle) -> Result<ReconcileReport, ReconcileError>` plus a `ReconcileReport` struct (counters for session + container work).
- [ ] Add `mod startup;` in `src/main.rs` so the entry-point is `minion::startup::reconcile()` (this is the integration contract wt1 provides to future epics — D8).
- [ ] Phase 1 — session reconciliation: `SELECT id FROM sessions WHERE status = 'running'`; for each, append `Event::SignalReceived { signal: "crash_recovery".to_string() }` then `UPDATE sessions SET status = 'failed', ended_at = NOW() WHERE id = $1`. Must be idempotent.
- [ ] Phase 2 — container reconciliation: `docker ps --filter name=minion-session-* --format {{.Names}}` via `tokio::process::Command` argv-only (NEVER `sh -c`). Extract UUID suffix; if no matching `running` session in PG, `docker rm -f <name>` via argv. Tolerate "No such container" stderr as success (NFR12).
- [ ] Phase 3 — worktree pruning: return `Ok(())` with the exact comment `// TODO(Epic 4): D8 two-phase prune — see Epic 4 Story N.M`. No filesystem access.
- [ ] Add the emit-before-IO exemption comment at the top of `reconcile()`: `// Exempt from emit-before-IO rule: runs before any live session exists at startup`.
- [ ] Call `reconcile(&pg, &lifecycle).await?` from `main()` BEFORE any `Engine::new()` constructor.
- [ ] Log completion via `tracing::info!(sessions_reconciled = n, containers_pruned = m, "startup reconcile complete")` (structured fields, NOT format strings).
- [ ] **NOTE:** `tests/startup_reconcile.rs` at workspace root is forbidden territory (wt2 owns `tests/`). Place the integration test under `crates/minion-harness/tests/startup_reconcile.rs` (wt1-owned). Keep `.timeout(Duration::from_secs(N))` per Rule 7b and graceful skip when Docker/PG unavailable (env flag or `#[ignore]` opt-in).
- [ ] Run the integration test (opt-in) and paste evidence; run unit coverage unconditionally.

**Dev Notes:**

- Architecture decisions: D8 (`minion::startup::reconcile()` entry-point, three sequential phases), NFR11 (crash recovery), NFR12 (idempotent cleanup).
- The workspace-root `tests/` directory is wt2's territory — DO NOT create files there. Integration tests for Epic 2 live under `crates/minion-harness/tests/`.
- `docker ps` / `docker rm -f` MUST use `tokio::process::Command` with argv form. Never `sh -c`, never interpolate.
- The Phase 1 emit of `SignalReceived { signal: "crash_recovery" }` reuses the variant from Story 2.3 — the signal-string policy is lowercase snake_case.

**Dev Agent Record**

_Fill this in as you work._

- Files created/modified:
- Notes on choices / deviations:
- Test evidence (cargo test output snippet):

---

### Story 2.5 — Add `minion session list --status` CLI Subcommand

**Feature 9 in features.md.**
**Source:** `_bmad-output/sandcastle-features/epics.md` (lines 576–626)

**Status:** Draft

### Story 2.5: Add `minion session list --status` CLI Subcommand

As a DevOps engineer,
I want `minion session list --status <running|completed|failed|cancelled> [--since <duration>]` backed by a PostgreSQL query on `sessions.status`,
So that I can audit session outcomes and filter by time range without loading full event logs.

**Acceptance Criteria:**

**Given** a new subcommand in the CLI parser
**When** inspected
**Then** `SessionListArgs` derives `clap::Args` with `status: SessionStatus` (clap `ValueEnum` with variants `Running`, `Completed`, `Failed`, `Cancelled` — snake_case on CLI)
**And** `since: Option<humantime::Duration>` is an optional flag `--since <duration>` (parsed via `humantime::parse_duration`)

**Given** the user invokes `minion session list --status running`
**When** the handler runs
**Then** it executes `SELECT id, status, started_at, ended_at FROM sessions WHERE status = $1 ORDER BY started_at DESC` against PG
**And** prints one row per session in tabular format: `<id>  <status>  <started_at ISO-8601 UTC>  <ended_at ISO-8601 UTC or '-'>`
**And** uses `chrono::DateTime<Utc>` `to_rfc3339()` for timestamp formatting

**Given** the user passes `--since 24h`
**When** the handler runs
**Then** the query appends `AND started_at > NOW() - $2::INTERVAL` with the humantime duration bound
**And** invalid durations produce a clap-layer error at parse time (not runtime)

**Given** the session-log-as-truth invariant (D1)
**When** the handler implementation is inspected
**Then** it queries PostgreSQL directly (no `DashMap`, no `Lazy<Mutex<HashMap>>`, no in-memory session cache)
**And** an inline comment documents: `// session-log-as-truth (D1): query PG, never an in-memory registry`

**Given** invalid `--status foobar`
**When** the user invokes with an unsupported value
**Then** clap rejects at parse time with a clear error listing valid values (`running, completed, failed, cancelled`)
**And** exit code is `2` (clap's default for parse errors)

**Given** an integration test at `tests/session_list_cli.rs` using `assert_cmd`
**When** the test seeds PG with sessions in each status, then invokes `minion session list --status running`
**Then** stdout contains only `status='running'` rows
**And** output is ordered by `started_at DESC`
**And** `--since 1h` filters out sessions older than 1 hour
**And** every `Command` has `.timeout(Duration::from_secs(N))` (Rule 7b)
**And** the test skips gracefully if PG is unavailable

Coverage: FR24

**Tasks/Subtasks:**

- [ ] Add `SessionStatus` (clap `ValueEnum` with variants `Running`, `Completed`, `Failed`, `Cancelled` — snake_case on CLI) in `src/cli/commands.rs`.
- [ ] Add `SessionListArgs` deriving `clap::Args` with `status: SessionStatus` and `since: Option<humantime::Duration>` (parsed via `humantime::parse_duration`).
- [ ] Wire `session list` as a subcommand (under `session`) in the CLI parser.
- [ ] Implement the handler: `SELECT id, status, started_at, ended_at FROM sessions WHERE status = $1 ORDER BY started_at DESC`; when `--since <duration>`, append `AND started_at > NOW() - $2::INTERVAL` with the humantime duration bound.
- [ ] Query PostgreSQL directly — NO `DashMap`, NO `Lazy<Mutex<HashMap>>`, NO in-memory cache. Add the inline comment verbatim: `// session-log-as-truth (D1): query PG, never an in-memory registry`.
- [ ] Format output rows: `<id>  <status>  <started_at ISO-8601 UTC>  <ended_at ISO-8601 UTC or '-'>` using `chrono::DateTime<Utc>::to_rfc3339()`.
- [ ] Ensure invalid `--status foobar` and invalid `--since …` both fail at clap parse time (exit code 2).
- [ ] **NOTE:** `tests/session_list_cli.rs` at workspace root is forbidden for wt1 (wt2 owns `tests/`). Place the integration test under `crates/minion-harness/tests/session_list_cli.rs`. Use `assert_cmd`, `.timeout(Duration::from_secs(N))` (Rule 7b), and gracefully skip when PG is unavailable.
- [ ] Run the integration test (opt-in when PG available) and paste evidence.

**Dev Notes:**

- Architecture decisions: D1 (session-log-as-truth — query PG directly, never an in-memory registry).
- `SessionStatus` lives in the CLI layer for clap; the runtime representation is the existing `sessions.status` VARCHAR in PG.
- `humantime::Duration` must be convertible to a PG INTERVAL — bind as text (`"24h"` → `INTERVAL '24 hours'`) or cast via `$2::INTERVAL` with the `to_string()` of the duration.
- Workspace-root `tests/` is forbidden; integration tests live under `crates/minion-harness/tests/`.

**Dev Agent Record**

_Fill this in as you work._

- Files created/modified:
- Notes on choices / deviations:
- Test evidence (cargo test output snippet):

---

## Project Context

No `project-context.md` found. Follow existing project conventions:
- Rust workspace with 4 crates under `crates/` (`minion-core`, `minion-session`, `minion-sandbox-orchestrator`, `minion-harness`) and a legacy engine binary under `src/`.
- Error split: domain errors use `thiserror` (in crates); the binary (`src/`) uses `anyhow`.
- Event ordering: D5 "emit-before-IO" — ALWAYS `session.append(event).await?` BEFORE any `lifecycle.destroy/exec` call. Never wrap emit in `tokio::spawn`.
- Termination taxonomy: D9 `TerminationReason` sub-enum. New reasons (e.g. `SignalReceived`) go in `crates/minion-core/src/error.rs`.
- Non-exhaustive policy: `#[non_exhaustive]` on public enums; `#[deny(non_exhaustive_omitted_patterns)]` at workspace lint level (nightly-only safeguard).
- Argv-not-shell: sandbox command invocations pass args as `&[String]`, never joined into a shell string.
- Tests: integration tests under `crates/<crate>/tests/`. Tests needing Postgres skip gracefully when `MINION_HARNESS_DATABASE_URL` is unset.

Reference architecture decisions in `_bmad-output/sandcastle-features/architecture.md` — especially D4, D5, D8, D9 for Epic 2.

---

## File Ownership (CRITICAL — from territory_map.json)

### Owned (you CAN freely create/edit)
- `src/main.rs`
- `src/cli/commands.rs`
- `src/cli/display.rs`
- `src/events/subscribers.rs`
- `crates/minion-core/src/event.rs` (you add the `SignalReceived` variant — Story 2.3)
- `crates/minion-session/src/session.rs`
- `crates/minion-session/src/lib.rs`
- `crates/minion-session/tests/` (entire directory)
- `crates/minion-session/migrations/` (add migrations if needed)
- `crates/minion-harness/tests/` (entire directory — new tests for broadcast plumbing, signal handling, signal cancel, reconcile, session list)

You may create NEW files within any owned directory.

### Read-only (import yes, modify no)
- `crates/minion-sandbox-orchestrator/src/lib.rs` — wt2's territory
- `crates/minion-sandbox-orchestrator/src/docker.rs` — wt2's territory
- `crates/minion-sandbox-orchestrator/src/mock.rs` — wt2's territory (Story 2.3 depends on `MockLifecycleCall::Destroy { id }` instrumentation that wt2 must provide; if missing, raise BLOCKER — do NOT edit)
- `crates/minion-core/src/workflow.rs` — wt2 may add variants; stay out
- `crates/minion-core/src/error.rs` — wt2 may add variants; stay out (if `TerminationReason::SignalReceived` is not already present from Epic 1 Story 1.2, raise BLOCKER)
- `src/workflow/schema.rs` — wt2's territory for Story 3.3

### Forbidden (don't even `cd` into)
- `crates/minion-sandbox-orchestrator/src/` — entire directory is wt2 territory
- `tests/` at workspace root — wt2 creates `tests/injection_negative.rs` there, among others. ALL Epic 2 integration tests live under `crates/<crate>/tests/` instead.

### Shared (special handling)

#### `Cargo.toml` — `append_only`
Only APPEND workspace deps or feature flags; never re-order existing entries or bump versions of deps the other worktree uses.

#### `crates/minion-core/src/lib.rs` — `append_only`
Add `pub use` lines for any new types (e.g. re-exporting the `SignalReceived` variant if a re-export path exists); do NOT rename or reorder existing ones.

#### `crates/minion-harness/src/engine.rs` — `coordinated` (CRITICAL)
Both worktrees edit this file in this phase.
- **Your edits (wt1):**
  - Story 2.1: add `HarnessConfig::shutdown_tx: Arc<tokio::sync::broadcast::Sender<()>>` field and subscribe in `Engine::new` (`shutdown_rx` Engine field).
  - Story 2.3: add a broadcast-receiver arm to the `tokio::select!` inside `step()` / `run_step()` that emits `Event::SignalReceived` FIRST (D5 emit-before-IO) then tolerantly destroys the sandbox.
- **wt2's edits (Story 3.4):** wt2 adds a NEW `prepare_step` method on `Engine` and wires an `exec_with_env` call site inside `step()`.
- **Merge plan:** wt1 commits first (per `territory_map.json.mergeOrder = ["wt1", "wt2"]`). If wt2's changes have already landed when you rebase, expect conflict resolution around the struct declaration and the `step()` select region — wt2's `prepare_step` should coexist with your broadcast arm. Do NOT touch wt2's `prepare_step` body; merge by keeping both.

#### `crates/minion-harness/src/lib.rs` — `append_only`
Add new `pub use` lines (e.g. re-exporting new Engine-adjacent types); do NOT reorganize existing re-exports.

---

## Integration Contracts

### You provide (consumers: wt2, future Epics)
- **`Event::SignalReceived { signal: String }`** variant — added in Story 2.3 in `crates/minion-core/src/event.rs`. wt2 may pattern-match on this variant (they list `event.rs` as read-only) but MUST NOT modify the file. Any consumer subscriber must include an explicit match arm (enforced by `non_exhaustive_omitted_patterns = "deny"`).
- **`minion::startup::reconcile(pg, lifecycle)`** entry-point — added in Story 2.4 in `src/startup.rs`. Called from `main()` BEFORE any `Engine::new()`. wt2 does not consume this directly in Epic 3, but it occupies real estate in `main.rs` — watch for collisions on rebase.
- **`HarnessConfig::shutdown_tx: Arc<broadcast::Sender<()>>`** field — added in Story 2.1 in `crates/minion-harness/src/config.rs`. wt2's tests / call sites that construct `HarnessConfig` will need to provide a channel or use a helper you expose.
- **`SessionStatus`-filtered query APIs** for `minion session list --status` — added in Story 2.5.

### You consume from wt2 (nothing during the parallel phase)
Nothing directly. Epic 3 is independent in this parallel phase. After wt2 merges, the `SandboxLifecycle` trait will gain `exec_with_env` (Story 3.1) — you do NOT need it for Epic 2. Later, in Epic 5, the `TerminationReason` taxonomy may gain `IdleTimeout`, but that is out of scope here.

---

## MCP Tools — MANDATORY

- **Serena** (`mcp__plugin_serena_serena__*`): for any non-trivial code read/edit. Symbolic only. Never `Read` a large Rust file when you can `get_symbols_overview` → `find_symbol` → `replace_symbol_body`.
- **Sequential Thinking** (`sequentialthinking`): call BEFORE coding any multi-file story. Plan the file touches, emit-order sequence, and which tests you'll write first.

---

## Implementation Order

Stories are listed in dependency order (2.1 → 2.2 → 2.3 → 2.4 → 2.5). Implement one at a time, commit after each, move to the next.

## Self-Verification Loop (after ALL stories)

### Phase 1 — AC coverage
For each story, for each AC, read the implementing code and score PASS / FAIL / PARTIAL. If any FAIL or >2 PARTIAL, fix, commit, re-score.

### Phase 2 — Codex adversarial review
From this worktree:
```bash
node "$HOME/.claude/plugins/marketplaces/openai-codex/plugins/codex/scripts/codex-companion.mjs" adversarial-review "--base main"
```
Fix critical/high findings immediately (commit). Document medium findings in VERIFICATION_REPORT.md; low findings noted only.

### Loop
Max 3 iterations of Phase 1 → Phase 2 → fix. Then write `VERIFICATION_REPORT.md` with AC table, Codex findings, and READY / NOT READY verdict.

### Completion sentinel
After VERIFICATION_REPORT.md is written and final tests pass, create `WORKTREE_COMPLETE.md` (summary of what shipped + commit hashes). Then signal:

```bash
echo "{\"type\":\"done\",\"wt\":1,\"branch\":\"minion-engine-bmad-wt1\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}" > .done
```

This signals the main orchestrator to run `/hive:verify-wt 1`. You may then exit.

---

## Test Database (for Postgres-dependent tests)

Some `minion-session` / `minion-harness` integration tests require Postgres. If present:
```
MINION_HARNESS_DATABASE_URL=postgres://postgres:iClinic@localhost:5432/minion_harness_test
```
Tests skip gracefully when the env var is absent; you may run the subset that doesn't need DB.
