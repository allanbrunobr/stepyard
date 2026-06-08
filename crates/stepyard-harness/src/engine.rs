//! The [`Engine`] type — step/resume/cancel loop over a [`Session`] and a
//! [`SandboxLifecycle`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use chrono::Utc;
use stepyard_core::{ChatMessage, Event, Signal, StepOutputSnapshot};
use stepyard_sandbox_orchestrator::{
    BranchStrategy, CreateOptions, SandboxError, SandboxLifecycle, WorkflowOutcome, Workspace,
    WorkspaceError, WorkspaceManager,
};
use stepyard_session::{Session, SessionError, SessionEvent, SessionId, SessionStatus};
use tokio::sync::broadcast;

use crate::chat_exec::ChatClient;
use crate::defaults::Defaults;
use crate::executor::{SandboxStepExecutor, StepExecutor};
use crate::gate::{evaluate_bool, outcome_for, GateAction, GateError, GateOutcome};
use crate::render::{render, RenderContext};
use crate::workflow::{Step, StepKind, Workflow};

/// Canonical lower-case label for a [`StepKind`], used as `step_type` on
/// every emitted event. Keeping it a free function means the engine never
/// calls `serde_json::to_string` on the enum just to get a wire label.
fn step_type_label(kind: &StepKind) -> &'static str {
    match kind {
        StepKind::Cmd => "cmd",
        StepKind::Agent => "agent",
        StepKind::Chat => "chat",
        StepKind::Gate => "gate",
        StepKind::Repeat => "repeat",
        StepKind::Map => "map",
        StepKind::Parallel => "parallel",
        StepKind::Call => "call",
        StepKind::Template => "template",
        StepKind::Script => "script",
    }
}

/// Default grace period (in seconds) for in-flight engines to wrap up after a
/// shutdown broadcast fires. D2: matches the architecture-decided 10s budget
/// (NFR1 keeps cleanup well under the kernel's 30s SIGKILL deadline).
fn default_shutdown_grace_s() -> u64 {
    10
}

/// Build a throwaway shutdown broadcast sender. Real production wiring flows
/// the sender down from `main()` — this helper keeps `HarnessConfig::default`
/// and serde-deserialised configs ergonomic for tests without breaking the
/// "only `main()` owns the real Sender" invariant.
fn default_shutdown_tx() -> Arc<broadcast::Sender<()>> {
    let (tx, _) = broadcast::channel::<()>(16);
    Arc::new(tx)
}

/// Build an empty shared signal-name slot. `install_handlers` populates it
/// with `"sigint"` / `"sigterm"` (or `"crash_recovery"` in Story 2.4) before
/// firing the broadcast; each engine reads it via its `tokio::select!` arm
/// (Story 2.3 Option B — avoids widening the `broadcast::channel<()>` generic
/// and touching Story 2.1's frozen signature).
fn default_shutdown_signal() -> Arc<OnceLock<String>> {
    Arc::new(OnceLock::new())
}

/// Host API proxy settings for sandbox cmd steps — keeps the real
/// `ANTHROPIC_API_KEY` on the host while the container receives a
/// per-session token and `ANTHROPIC_BASE_URL` pointing at the proxy.
#[derive(Debug, Clone)]
pub struct AnthropicProxyEnv {
    pub port: u16,
    pub auth_token: String,
}

/// Runtime configuration for the harness.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct HarnessConfig {
    /// Tenant key used for new sessions (maps to `sessions.tenant_id`).
    pub tenant_id: String,
    /// Per-process shutdown broadcast. Constructed once in `main()` and
    /// cloned into every `HarnessConfig`; `Engine::new` calls `.subscribe()`
    /// on it to obtain its own `Receiver`. Never serialised — the
    /// `#[serde(skip, default = …)]` attribute reconstructs a disconnected
    /// sender if a `HarnessConfig` is deserialised (configs loaded from YAML
    /// are rewired by `main()` before any engine spawns).
    #[serde(skip, default = "default_shutdown_tx")]
    pub shutdown_tx: Arc<broadcast::Sender<()>>,
    /// Seconds the signal handler waits after broadcasting before forcing a
    /// non-zero exit. D2 default 10s.
    #[serde(default = "default_shutdown_grace_s")]
    pub shutdown_grace_s: u64,
    /// Shared signal-name slot (Story 2.3 — Option B). `main()` constructs a
    /// single `Arc<OnceLock<String>>` and clones it into this field; the
    /// signal handler (`src/signal.rs`) calls `.set("sigint" | "sigterm")`
    /// **before** firing `shutdown_tx.send(())`, so by the time this engine
    /// sees the broadcast, the name is already populated. The `OnceLock`
    /// guarantees read-your-write across threads without a lock.
    #[serde(skip, default = "default_shutdown_signal")]
    pub shutdown_signal: Arc<OnceLock<String>>,
    /// Injected chat-completion client for [`StepKind::Chat`] dispatch
    /// (PR 5c commit 2 of Task #31). `None` in commit 2 — an in-process
    /// caller that constructs a chat step without injecting a client
    /// surfaces a typed `StepFailed` (`ChatExecError::NoClientConfigured`)
    /// rather than a panic. Commit 3 flips the CLI adapter to accept
    /// `chat` steps and wires a production default here. Never
    /// serialised — `#[serde(skip, default)]` keeps
    /// YAML-deserialised configs buildable (main() rewires the field
    /// before any engine spawns, mirroring `shutdown_tx`).
    #[serde(skip, default)]
    pub chat_client: Option<Arc<dyn ChatClient>>,
    /// Structured sandbox creation options derived from workflow sandbox config.
    #[serde(skip, default)]
    pub create_options: CreateOptions,
    /// When set, sandbox cmd steps route Anthropic traffic through the host proxy.
    #[serde(skip, default)]
    pub anthropic_proxy: Option<AnthropicProxyEnv>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            tenant_id: "default".into(),
            shutdown_tx: default_shutdown_tx(),
            shutdown_grace_s: default_shutdown_grace_s(),
            shutdown_signal: default_shutdown_signal(),
            chat_client: None,
            create_options: CreateOptions::default(),
            anthropic_proxy: None,
        }
    }
}

/// Strip the real API key from a resolved env map and inject proxy routing
/// pairs for sandbox execution.
pub fn apply_anthropic_proxy_env(env: &mut HashMap<String, String>, proxy: &AnthropicProxyEnv) {
    env.remove("ANTHROPIC_API_KEY");
    env.insert(
        "ANTHROPIC_BASE_URL".into(),
        format!("http://host.docker.internal:{}", proxy.port),
    );
    env.insert("ANTHROPIC_API_KEY".into(), proxy.auth_token.clone());
}

/// Per-invocation inputs threaded into the renderer — data that varies
/// from run to run (the CLI's `--target` and `--var k=v` flags) rather
/// than process-lifetime config.
///
/// PR 2 of Task #31. Kept separate from [`HarnessConfig`] per the
/// architecture review: `target` and `vars` are execution inputs to a
/// single workflow run, not engine-lifetime state, so they should not
/// compete with the shutdown broadcast / tenant id for space on the
/// long-lived config. Defaults to empty — callers that don't need
/// rendering can skip the builder.
#[derive(Debug, Clone, Default)]
pub struct RunContext {
    /// Deployment target selected for this run. Exposed to templates as
    /// `{{ target }}`. Empty when no target is specified.
    pub target: String,
    /// Key/value overrides collected from the CLI's `--var k=v` flags.
    /// Exposed to templates as `{{ vars.name }}`.
    pub vars: HashMap<String, String>,
}

/// Outcome of a single [`Engine::step`] call.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// One more step was executed successfully; there are more steps to run.
    StepCompleted { step_name: String },
    /// One step failed; the workflow should not advance further without
    /// operator intervention.
    StepFailed { step_name: String, error: String },
    /// All steps have been executed (success path).
    WorkflowCompleted,
    /// The session was cancelled (via [`Engine::cancel`] or a prior
    /// cancellation is still in effect). The caller should stop the loop.
    Cancelled,
}

/// Domain errors from the harness.
///
/// `StepFailed` mirrors the shape of [`stepyard_core::EngineError::StepFailed`]
/// so timeout / cancel / signal termination reasons funnel through a single
/// taxonomy (D9). Callers can match on `reason` instead of listing sibling
/// variants.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("session error: {0}")]
    Session(#[from] SessionError),

    #[error("sandbox error: {0}")]
    Sandbox(#[from] stepyard_sandbox_orchestrator::SandboxError),

    #[error("workspace error: {0}")]
    Workspace(#[from] WorkspaceError),

    #[error("invalid state: {0}")]
    InvalidState(String),

    #[error("step {step_index} failed: {reason}")]
    StepFailed {
        step_index: u32,
        reason: stepyard_core::TerminationReason,
    },
}

/// Clone-friendly cancel flag tied to a session. Shared with [`Engine`].
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn cancel(&self) {
        self.inner.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }
}

/// The harness — orchestrates one workflow for one session, step by step.
///
/// `Engine` owns:
/// * a [`Session`] handle (the append-only log)
/// * a [`StepExecutor`] (the thing that runs a step inside a sandbox)
/// * a [`Workflow`] definition (the ordered steps)
/// * a [`CancelToken`] (the cancel signal)
/// * a [`HarnessConfig`]
///
/// The key design property: between two `step` calls the engine holds no
/// state about what has already run. `step` always asks the session log
/// "how many steps have completed?" and executes the next one. This is
/// what makes `resume` and `cancel` trivially correct (Invariante 11).
pub struct Engine {
    session: Session,
    executor: Arc<dyn StepExecutor>,
    lifecycle: Arc<dyn SandboxLifecycle>,
    workflow: Workflow,
    cancel: CancelToken,
    #[allow(dead_code)]
    config: HarnessConfig,
    /// Receiver handed out by `config.shutdown_tx.subscribe()` at construction
    /// time. Story 2.3 reads from it inside `step()`'s `tokio::select!` to fold
    /// SIGINT/SIGTERM into the same cancel path as `CancelToken`. `#[allow]`
    /// while Story 2.3 lands — the field is load-bearing even before the
    /// select arm exists.
    #[allow(dead_code)]
    shutdown_rx: broadcast::Receiver<()>,
    /// First-step timestamp. Plain `Option` is Send-safe and `&mut self` on
    /// every public mutator means we never need a lock here.
    started_at: Option<Instant>,
    /// Cascade resolver's weakest layer (Story 3.4). Attached via
    /// [`Engine::with_defaults`]; empty by default so existing callers keep
    /// compiling.
    defaults: Defaults,
    /// Per-run inputs exposed to the gate/cmd renderer (PR 2 of Task #31).
    /// Attached via [`Engine::with_run_context`]; empty by default.
    run_context: RunContext,
    /// Optional git workspace manager. Disabled by default until the CLI/YAML
    /// branch-strategy story wires a concrete manager into production paths.
    workspace_manager: Option<Arc<dyn WorkspaceManager>>,
    workspace_strategy: BranchStrategy,
    workspace_path: Option<PathBuf>,
    workspace: Option<Workspace>,
    workspace_prepared: bool,
}

impl Engine {
    /// Construct a new engine for an already-started session and a workflow.
    /// Typically the caller creates the `Session` via `Session::new(...)`
    /// and passes it here.
    pub fn new(
        config: HarnessConfig,
        session: Session,
        workflow: Workflow,
        lifecycle: Arc<dyn SandboxLifecycle>,
    ) -> Self {
        let executor = Arc::new(
            SandboxStepExecutor::new(lifecycle.clone())
                .with_create_options(config.create_options.clone()),
        );
        Self::with_executor(config, session, workflow, lifecycle, executor)
    }

    /// Like [`Engine::new`] but with a custom [`StepExecutor`] — used by
    /// tests to bypass sandbox creation.
    pub fn with_executor(
        config: HarnessConfig,
        session: Session,
        workflow: Workflow,
        lifecycle: Arc<dyn SandboxLifecycle>,
        executor: Arc<dyn StepExecutor>,
    ) -> Self {
        let shutdown_rx = config.shutdown_tx.subscribe();
        Self {
            session,
            executor,
            lifecycle,
            workflow,
            cancel: CancelToken::default(),
            config,
            shutdown_rx,
            started_at: None,
            defaults: Defaults::default(),
            run_context: RunContext::default(),
            workspace_manager: None,
            workspace_strategy: BranchStrategy::Head,
            workspace_path: None,
            workspace: None,
            workspace_prepared: false,
        }
    }

    /// Attach `.stepyard/defaults.yaml` defaults to this engine (Story 3.4).
    /// The cascade resolver overlays these below `workflow.env` and
    /// `step.env`. Builder-style so call sites stay compact.
    pub fn with_defaults(mut self, defaults: Defaults) -> Self {
        self.defaults = defaults;
        self
    }

    /// Attach per-run renderer inputs (CLI `--target` and `--var k=v`).
    /// PR 2 of Task #31. Builder-style so tests and the CLI call site both
    /// stay one-liners.
    pub fn with_run_context(mut self, run_context: RunContext) -> Self {
        self.run_context = run_context;
        self
    }

    /// Attach a workspace manager for Epic 4 worktree preparation. Production
    /// CLI wiring arrives with the branch-strategy story; tests and future
    /// callers can use this builder to exercise the emit-before-IO contract.
    pub fn with_workspace_manager(
        mut self,
        manager: Arc<dyn WorkspaceManager>,
        strategy: BranchStrategy,
        planned_path: PathBuf,
    ) -> Self {
        self.workspace_manager = Some(manager);
        self.workspace_strategy = strategy;
        self.workspace_path = Some(planned_path);
        self
    }

    async fn prepare_workspace_if_configured(&mut self) -> Result<(), EngineError> {
        if self.workspace_prepared {
            return Ok(());
        }

        let Some(manager) = self.workspace_manager.clone() else {
            return Ok(());
        };

        let session_id = self.session.id();
        let path = self.workspace_path.clone().unwrap_or_else(|| {
            PathBuf::from(format!(
                ".stepyard/workspaces/stepyard-session-{session_id}"
            ))
        });
        let strategy = self.workspace_strategy.clone();

        self.emit(Event::WorkspacePrepared {
            path: path.display().to_string(),
            strategy: strategy.as_str().to_string(),
        })
        .await?;

        match &strategy {
            BranchStrategy::Head => {}
            BranchStrategy::MergeToHead { target } => {
                self.emit(Event::BranchCreated {
                    branch: format!("stepyard/session-{}", session_id.as_uuid()),
                    base: target.clone(),
                })
                .await?;
            }
            BranchStrategy::NamedBranch { name } => {
                self.emit(Event::BranchCreated {
                    branch: name.clone(),
                    base: "HEAD".to_string(),
                })
                .await?;
            }
            _ => {}
        }

        let workspace = manager.prepare(&session_id, &strategy).await?;
        self.workspace = Some(workspace);
        self.workspace_prepared = true;
        Ok(())
    }

    async fn finalize_workspace_success_if_configured(&mut self) -> Result<(), EngineError> {
        let Some(manager) = self.workspace_manager.clone() else {
            return Ok(());
        };
        let Some(workspace) = self.workspace.clone() else {
            return Ok(());
        };

        let merge_pair = workspace
            .branch
            .as_ref()
            .zip(workspace.merge_target.as_ref())
            .map(|(source, target)| (source.clone(), target.clone()));
        if let Some((source, target)) = &merge_pair {
            self.emit(Event::MergeAttempted {
                source: source.clone(),
                target: target.clone(),
            })
            .await?;
        }

        match manager.finalize(&workspace, WorkflowOutcome::Success).await {
            Ok(_) => Ok(()),
            Err(WorkspaceError::MergeConflict { files }) => {
                if let Some((source, target)) = merge_pair {
                    self.emit(Event::MergeConflict {
                        source,
                        target,
                        files: files.clone(),
                    })
                    .await?;
                }
                Err(WorkspaceError::MergeConflict { files }.into())
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Resolve the effective env for `step` by overlaying
    /// `defaults.env` < `workflow.env` < `step.env` and expanding any
    /// exact-form `${VAR}` values against the host process env
    /// (`std::env::var`). Returns [`EngineError::InvalidState`] if a
    /// referenced host variable is not set — fails fast so no step runs
    /// with a partially-resolved env (Story 3.4 AC2).
    ///
    /// The AC's documented signature takes `&Defaults` as a parameter; we
    /// store defaults on the `Engine` instead (per the builder
    /// [`Engine::with_defaults`]) so the call site in [`Engine::step`]
    /// stays a single-line swap. Behavior is identical.
    pub fn prepare_step(&self, step: &Step) -> Result<HashMap<String, String>, EngineError> {
        resolve_env(&self.defaults, &self.workflow.env, &step.env)
    }

    /// Handle to cancel this engine from another task. Keep a clone before
    /// spawning the workflow loop so you still have a reference.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Signal cancellation. A currently running step completes as
    /// `Cancelled`; subsequent `step` calls return `StepOutcome::Cancelled`
    /// immediately without executing anything.
    pub async fn cancel(&self) -> Result<(), EngineError> {
        self.cancel.cancel();
        Ok(())
    }

    /// The session this engine operates on.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Execute exactly one step of the workflow. Emits `StepStarted` +
    /// (`StepCompleted` | `StepFailed`) into the session log. Returns
    /// [`StepOutcome::WorkflowCompleted`] once every step has a completion
    /// event in the log.
    pub async fn step(&mut self) -> Result<StepOutcome, EngineError> {
        // Ask the log how far we are. "Completed" counts only include
        // successful StepCompleted events; a StepFailed means the workflow
        // is stuck and no new step should be executed.
        let progress = self.progress_from_log().await?;
        if progress.has_failure {
            // Workflow is in failed state; do not advance. Make sure the
            // session row reflects that even if a previous process died
            // before flipping it (Story 2.4 AC: status=failed on step fail).
            self.finalise_fail().await?;
            return Ok(StepOutcome::StepFailed {
                step_name: progress.last_failed_step.unwrap_or_default(),
                error: "workflow previously failed".into(),
            });
        }
        if progress.workflow_completed {
            self.finalise_success().await?;
            return Ok(StepOutcome::WorkflowCompleted);
        }
        if progress.completed_steps >= self.workflow.steps.len() {
            // Happy path: every step has a completed event. Mark session.
            self.finalise_success().await?;
            return Ok(StepOutcome::WorkflowCompleted);
        }

        // Fast-path cancel: a cancel token flipped before this step boundary
        // (either pre-workflow or between two steps). Emit a StepFailed event
        // for the would-be-next step so progress_from_log() treats the
        // workflow as terminal on reload — otherwise a restarted engine could
        // advance past a cancelled session (architecture.md §D9 + NFR13).
        // Symmetric with the step-timeout terminality fix.
        if self.cancel.is_cancelled() {
            let next_step = &self.workflow.steps[progress.completed_steps];
            let next_step_type = step_type_label(&next_step.kind);
            // Keep the "log begins with workflow_started" invariant even when
            // a cancel races ahead of the first step — emit WorkflowStarted
            // exactly once per session if it has not been emitted yet.
            if progress.completed_steps == 0 && self.started_at.is_none() {
                self.started_at = Some(Instant::now());
                self.emit(Event::WorkflowStarted {
                    timestamp: Utc::now(),
                })
                .await?;
            }
            self.emit(Event::StepFailed {
                step_name: next_step.name.clone(),
                step_type: next_step_type.into(),
                error: "Cancelled".into(),
                duration_ms: 0,
                timestamp: Utc::now(),
                sandboxed: matches!(next_step.kind, StepKind::Cmd),
            })
            .await?;
            self.finalise_cancel().await?;
            return Ok(StepOutcome::Cancelled);
        }

        let step = &self.workflow.steps[progress.completed_steps].clone();
        let step_type = step_type_label(&step.kind);
        let start = Instant::now();

        // Remember when the workflow actually started (first step).
        if progress.completed_steps == 0 && self.started_at.is_none() {
            self.started_at = Some(start);
            // Emit WorkflowStarted exactly once per session.
            self.emit(Event::WorkflowStarted {
                timestamp: Utc::now(),
            })
            .await?;
            self.prepare_workspace_if_configured().await?;
        }

        self.emit(Event::StepStarted {
            step_name: step.name.clone(),
            step_type: step_type.into(),
            timestamp: Utc::now(),
            // Scope-aware emission lands with the executor in a later
            // commit of PR 3; top-level steps emit `None`.
            scope_context: None,
        })
        .await?;

        // If cancel landed between StepStarted and exec, bail now — the
        // step is still in a recoverable place (no partial exec output
        // yet, so the retry path from `resume` after manual uncancel is
        // clean). In practice we treat it as the same as post-exec cancel.
        if self.cancel.is_cancelled() {
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error: "Cancelled".into(),
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now(),
                sandboxed: matches!(step.kind, StepKind::Cmd),
            })
            .await?;
            self.finalise_cancel().await?;
            return Ok(StepOutcome::Cancelled);
        }

        // Gate dispatch: pure template evaluation, no sandbox, no select.
        // Lives here so the shared pre-step scaffolding (progress check,
        // WorkflowStarted, StepStarted, cancel gates) stays the single
        // source of truth; only the exec step itself forks by kind.
        // PR 2 of Task #31.
        if matches!(step.kind, StepKind::Gate) {
            let step_index = progress.completed_steps as u32;
            return self
                .run_gate_step(step, &progress.outputs, start, step_index)
                .await;
        }

        // Container dispatch (call/repeat/map/parallel): the scope runner owns
        // the whole lifecycle — iteration bodies, scoped event emission, and
        // the terminal container StepCompleted/StepFailed. PR 3 of #31.
        // Parallel reuses the same dispatch (PR 6 of #31): the adapter
        // synthesises a hidden scope from v1's inline `steps:` list, so the
        // harness sees parallel as just another scope-bodied container.
        if matches!(
            step.kind,
            StepKind::Call | StepKind::Repeat | StepKind::Map | StepKind::Parallel
        ) {
            return self.run_container_step(step, start).await;
        }

        // Template dispatch: pure file-read + Tera render, no sandbox,
        // no select. Output is unified onto the cmd shape (stdout = rendered
        // text) so cross-step refs (`{{ steps.tmpl.stdout }}`) resolve with
        // no event-schema change. PR 4 of Task #31.
        if matches!(step.kind, StepKind::Template) {
            let step_index = progress.completed_steps as u32;
            return self
                .run_template_step(step, &progress.outputs, start, step_index)
                .await;
        }

        // Script dispatch: in-process Rhai evaluation against a flat
        // snapshot of cross-step outputs + target. Same unified output
        // shape as template so `{{ steps.sc.stdout }}` resolves without
        // a new event variant. PR 4 of Task #31.
        if matches!(step.kind, StepKind::Script) {
            let step_index = progress.completed_steps as u32;
            return self
                .run_script_step(step, &progress.outputs, start, step_index)
                .await;
        }

        // Agent dispatch: spawn the Claude CLI, pipe the rendered prompt
        // to its stdin, parse the stream-json response, and lift
        // usage/session_id onto the StepCompleted event. The session-id
        // map plus first-wins capture come from `progress` so the argv
        // builder's `resume:` / `fork_session:` / default-shared paths
        // see the same state a replay would reconstruct. PR 5a of #31.
        if matches!(step.kind, StepKind::Agent) {
            let step_index = progress.completed_steps as u32;
            return self
                .run_agent_step(step, &progress, start, step_index)
                .await;
        }

        // Chat dispatch: call the injected `ChatClient` seam and emit
        // the user turn + assistant turn + terminal StepCompleted in
        // the atomic order `compute_progress` relies on (engine.rs
        // around the `step_type == "chat"` drain). Commit 2 of PR 5c
        // ships happy-path; commit 4 wraps the provider call in the
        // same `tokio::select!` race cmd and agent run, so
        // `step_index` is threaded through here for the
        // `EngineError::StepFailed{step_index, ...}` shape on the
        // signal/timeout arms. PR 5c of Task #31.
        if matches!(step.kind, StepKind::Chat) {
            let step_index = progress.completed_steps as u32;
            return self.run_chat_step(step, &progress, start, step_index).await;
        }

        // Kinds other than Cmd/Gate/Call/Repeat/Map/Template/Script/Agent/Chat
        // are still rejected at the adapter boundary. If an in-process
        // caller constructs one anyway, emit a structured StepFailed
        // instead of silently dispatching the cmd path — a typed "not
        // yet supported" outcome is easier to debug than a command with
        // an empty string.
        if !matches!(step.kind, StepKind::Cmd) {
            let error = format!(
                "step type `{step_type}` not yet supported in v2 engine — PR 5c of #31 ships cmd + gate + call/repeat/map + template + script + agent + chat"
            );
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error: error.clone(),
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_fail().await?;
            return Ok(StepOutcome::StepFailed {
                step_name: step.name.clone(),
                error,
            });
        }

        // Resolve env BEFORE the exec select. Failure here is a user-config
        // problem (missing `${VAR}`), not a sandbox/timeout problem — emit
        // StepFailed with the resolution error and stop. Fail-fast per AC2:
        // no step runs with a partial env.
        let resolved_env = match self.prepare_step(step) {
            Ok(env) => env,
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: "cmd".into(),
                    error: error.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: Utc::now(),
                    sandboxed: true,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        let step_index = progress.completed_steps as u32;
        // PR 3 of Task #31: top-level cmd and scope-body cmd share this
        // helper. `scope_context: None` tags the event as top-level so
        // `progress_from_log` counts it against the workflow's main step
        // axis; scoped callers pass `Some(ScopeContext { … })`.
        match self
            .execute_cmd_with_select(step, &resolved_env, step_index, None, start)
            .await?
        {
            CmdOutcome::Success(_) => Ok(StepOutcome::StepCompleted {
                step_name: step.name.clone(),
            }),
            CmdOutcome::Failed(error) => Ok(StepOutcome::StepFailed {
                step_name: step.name.clone(),
                error,
            }),
            CmdOutcome::Cancelled => Ok(StepOutcome::Cancelled),
            CmdOutcome::Signal(signal) => Err(EngineError::StepFailed {
                step_index,
                reason: stepyard_core::TerminationReason::SignalReceived(Signal::from(signal)),
            }),
            CmdOutcome::TimedOut { configured_ms } => Err(EngineError::StepFailed {
                step_index,
                reason: stepyard_core::TerminationReason::StepTimeout { configured_ms },
            }),
            CmdOutcome::IdleTimedOut { idle_ms } => Err(EngineError::StepFailed {
                step_index,
                reason: stepyard_core::TerminationReason::IdleTimeout { idle_ms },
            }),
        }
    }

    /// Race a cmd step against the cancel token, the configured timeout,
    /// and the process-wide shutdown broadcast (SIGINT/SIGTERM) — the
    /// shared execution path for every cmd kind the harness runs, whether
    /// it sits at the top of the workflow or inside a `call`/`repeat`/`map`
    /// scope body (PR 3 of Task #31).
    ///
    /// The helper emits the terminal event(s) and calls the correct
    /// `finalise_*` before returning so callers can map the
    /// [`CmdOutcome`] to a [`StepOutcome`] or [`EngineError`] without
    /// additional IO:
    ///
    /// | Branch  | Events emitted                                | Session state        |
    /// | ------- | --------------------------------------------- | -------------------- |
    /// | Success | `StepCompleted { scope_context, output }`     | Running (unchanged)  |
    /// | Failed  | `StepFailed`                                  | Failed               |
    /// | Cancel  | `StepFailed` (Cancelled)                      | Cancelled            |
    /// | Signal  | `SignalReceived` + `StepFailed`               | Cancelled            |
    /// | Timeout | `StepTimeoutFired` + `StepFailed` + sandbox destroy | Failed         |
    ///
    /// `step_index` is the top-level step index when called from
    /// `Engine::step`, or the container step's top-level index when
    /// called from a scope body — the scope runner is expected to
    /// carry `scope_context` for exact-position attribution since the
    /// engine does not expose a scoped step counter. Documented here
    /// because `StepTimeoutFired.step_index` reads `step_index`
    /// verbatim.
    pub(crate) fn sandbox_env_for_exec(
        &self,
        resolved_env: &HashMap<String, String>,
    ) -> HashMap<String, String> {
        let mut env = resolved_env.clone();
        if let Some(proxy) = &self.config.anthropic_proxy {
            apply_anthropic_proxy_env(&mut env, proxy);
        }
        env
    }

    pub(crate) async fn execute_cmd_with_select(
        &mut self,
        step: &Step,
        resolved_env: &HashMap<String, String>,
        step_index: u32,
        scope_context: Option<stepyard_core::ScopeContext>,
        start: Instant,
    ) -> Result<CmdOutcome, EngineError> {
        // Race the step against the cancel token and the optional step
        // timeout. Cancel gives SIGTERM-during-long-command a ~100 ms abort
        // window (Story 2.4 AC). Timeout is Story 1.4 AC: a step configured
        // with `timeout: N` in YAML is aborted at N ms of wall clock. When
        // `step.timeout` is `None` the timeout branch is `pending` forever
        // so the select degenerates to the old two-arm shape. Clone Arcs so
        // the exec future does not borrow `self` — we need `&mut self`
        // afterwards to finalise the session.
        let executor = self.executor.clone();
        let session_uuid = *self.session.id().as_uuid();
        let step_clone = step.clone();
        let cancel_token = self.cancel.clone();
        // Snapshot the signal-name slot before the `tokio::select!` borrows
        // `self.shutdown_rx`. Cloning an `Arc` doesn't touch the OnceLock —
        // we just need a reader handle the select arm can read without
        // holding another reference to `self`.
        let signal_slot = self.config.shutdown_signal.clone();
        let sandbox_env = self.sandbox_env_for_exec(resolved_env);
        let selection = {
            let exec_fut = executor.execute_with_env(session_uuid, &step_clone, &sandbox_env);
            let cancel_fut = async {
                while !cancel_token.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            let timeout_fut = async {
                match step_clone.timeout {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            };
            let shutdown_rx = &mut self.shutdown_rx;
            let shutdown_fut = shutdown_rx.recv();
            tokio::pin!(exec_fut);
            tokio::pin!(cancel_fut);
            tokio::pin!(timeout_fut);
            tokio::pin!(shutdown_fut);
            tokio::select! {
                r = &mut exec_fut => StepSelection::Done(r),
                _ = &mut cancel_fut => StepSelection::Cancelled,
                _ = &mut timeout_fut => StepSelection::TimedOut,
                // The result (`Ok(())` | `Err(Lagged|Closed)`) is ignored: any
                // resolution of this broadcast arm means shutdown has been
                // initiated by `src/signal.rs`, so we take the signal path.
                // The name slot is populated *before* the broadcast fires
                // (Story 2.3 install_handlers ordering), so `.get()` is
                // guaranteed to return `Some` here.
                _ = &mut shutdown_fut => StepSelection::Signal(
                    signal_slot
                        .get()
                        .cloned()
                        .unwrap_or_else(|| "unknown".into()),
                ),
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        // Signal landed first. D5 emit-before-IO ordering: persist both
        // facts to the session log BEFORE tearing the sandbox down.
        // StepFailed here does NOT carry scope_context — the event
        // schema only tags completions/starts with scope position (PR 3
        // of #31); a scoped cancel/signal still halts the whole session.
        if let StepSelection::Signal(ref signal) = selection {
            let signal = signal.clone();
            self.emit(Event::SignalReceived {
                signal: Signal::from(signal.clone()),
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: "cmd".into(),
                error: format!("Signal: {signal}"),
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: true,
            })
            .await?;
            self.finalise_cancel().await?;
            return Ok(CmdOutcome::Signal(signal));
        }

        // Timeout landed first. D5 emit-before-IO ordering: events
        // persist before sandbox teardown. `step_index` is the top-level
        // / container index — scope position lives on `scope_context`
        // (attached to the surrounding StepStarted) rather than here.
        if let StepSelection::TimedOut = selection {
            let configured_ms = step
                .timeout
                .expect("TimedOut requires step.timeout.is_some()")
                .as_millis() as u64;
            self.emit(Event::StepTimeoutFired {
                step_index,
                configured_ms,
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: "cmd".into(),
                error: format!("step timed out after {configured_ms}ms"),
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: true,
            })
            .await?;
            let _ = self.lifecycle.destroy_by_session(session_uuid).await;
            self.finalise_fail().await?;
            return Ok(CmdOutcome::TimedOut { configured_ms });
        }

        // Cancel landed mid-step: drop the exec future, emit StepFailed and
        // finalise the session as cancelled.
        let exec_result = match selection {
            StepSelection::Done(r) => r,
            StepSelection::Cancelled => {
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: "cmd".into(),
                    error: "Cancelled".into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: true,
                })
                .await?;
                self.finalise_cancel().await?;
                return Ok(CmdOutcome::Cancelled);
            }
            StepSelection::TimedOut => unreachable!("handled above"),
            StepSelection::Signal(_) => unreachable!("handled above"),
        };

        match exec_result {
            Ok(output) if output.is_success() => {
                // Persist stdout/stderr/exit_code in the session log so a later
                // gate step's `{{ steps.X.stdout }}` survives a process crash
                // and reloads via `progress_from_log` — without the snapshot
                // the harness would need per-session memory between steps and
                // break Invariante 11 on resume (PR 2 of Task #31).
                let snapshot = stepyard_core::StepOutputSnapshot {
                    stdout: output.stdout.clone(),
                    stderr: output.stderr.clone(),
                    exit_code: output.exit_code,
                };
                self.emit(Event::StepCompleted {
                    step_name: step.name.clone(),
                    step_type: "cmd".into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                    sandboxed: true,
                    output: Some(snapshot.clone()),
                    scope_context,
                    gate_outcome: None,
                    agent_session_id: None,
                })
                .await?;
                Ok(CmdOutcome::Success(snapshot))
            }
            Ok(output) => {
                let error = format!(
                    "step exited with code {}: {}",
                    output.exit_code,
                    output.stderr.trim()
                );
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: "cmd".into(),
                    error: error.clone(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: true,
                })
                .await?;
                self.finalise_fail().await?;
                Ok(CmdOutcome::Failed(error))
            }
            Err(SandboxError::IdleTimeout { idle_ms }) => {
                self.emit(Event::IdleTimeoutFired {
                    step_index,
                    idle_threshold_ms: idle_ms,
                })
                .await?;
                let error = stepyard_core::TerminationReason::IdleTimeout { idle_ms }.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: "cmd".into(),
                    error: error.clone(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: true,
                })
                .await?;
                let _ = self.lifecycle.destroy_by_session(session_uuid).await;
                self.finalise_fail().await?;
                Ok(CmdOutcome::IdleTimedOut { idle_ms })
            }
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: "cmd".into(),
                    error: error.clone(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: true,
                })
                .await?;
                self.finalise_fail().await?;
                Ok(CmdOutcome::Failed(error))
            }
        }
    }

    /// Drive `step` in a loop until the workflow terminates. After a process
    /// crash, construct a fresh `Engine` via [`Engine::resume_existing`] and
    /// call this to continue from wherever the session log left off.
    pub async fn resume(&mut self) -> Result<StepOutcome, EngineError> {
        loop {
            let outcome = self.step().await?;
            match &outcome {
                StepOutcome::StepCompleted { .. } => continue,
                StepOutcome::StepFailed { .. }
                | StepOutcome::WorkflowCompleted
                | StepOutcome::Cancelled => return Ok(outcome),
            }
        }
    }

    /// Load an existing session by id and attach a fresh engine to it. The
    /// workflow must match the one used when the session was originally
    /// created — the harness trusts the caller here (Story 2.x will add
    /// workflow hash verification).
    #[cfg(feature = "postgres")]
    pub async fn resume_existing(
        config: HarnessConfig,
        pool: &sqlx::PgPool,
        session_id: SessionId,
        workflow: Workflow,
        lifecycle: Arc<dyn SandboxLifecycle>,
    ) -> Result<Self, EngineError> {
        let session = Session::load(pool, session_id).await?;
        Ok(Self::new(config, session, workflow, lifecycle))
    }

    /// Execute a [`StepKind::Gate`] step — render `condition` against the
    /// cross-step outputs map + [`RunContext`], evaluate it as a boolean,
    /// apply `on_pass` / `on_fail`, and emit `StepCompleted` or
    /// `StepFailed` accordingly. PR 2 of Task #31.
    ///
    /// No sandbox, no tokio select — gate is pure in-memory evaluation on
    /// the replay-safe outputs map. `outputs` is rebuilt from the session
    /// log by [`Self::progress_from_log`] so a post-crash resume arrives
    /// here with the same data the pre-crash run would have seen.
    async fn run_gate_step(
        &mut self,
        step: &Step,
        outputs: &HashMap<String, StepOutputSnapshot>,
        start: Instant,
        step_index: u32,
    ) -> Result<StepOutcome, EngineError> {
        let step_type = step_type_label(&step.kind);

        // Validate config once, up front. These are validation errors, not
        // runtime failures, and we want them surfaced before render even
        // starts so a typo in on_pass / on_fail doesn't look like a
        // template problem.
        let on_pass = match GateAction::parse(step.on_pass.as_deref()) {
            Ok(a) => a,
            Err(e) => {
                return self
                    .emit_gate_failure(step, step_type, start, e.to_string())
                    .await
            }
        };
        let on_fail = match GateAction::parse(step.on_fail.as_deref()) {
            Ok(a) => a,
            Err(e) => {
                return self
                    .emit_gate_failure(step, step_type, start, e.to_string())
                    .await
            }
        };
        let Some(condition) = step.condition.as_deref().filter(|c| !c.trim().is_empty()) else {
            let err = GateError::MissingCondition {
                step: step.name.clone(),
            };
            return self
                .emit_gate_failure(step, step_type, start, err.to_string())
                .await;
        };

        let ctx = RenderContext {
            steps: outputs,
            target: &self.run_context.target,
            vars: &self.run_context.vars,
            scope: None,
        };
        let rendered = match render(condition, &ctx) {
            Ok(s) => s,
            Err(e) => {
                return self
                    .emit_gate_failure(step, step_type, start, e.to_string())
                    .await
            }
        };
        let passed = match evaluate_bool(&step.name, &rendered) {
            Ok(b) => b,
            Err(e) => {
                return self
                    .emit_gate_failure(step, step_type, start, e.to_string())
                    .await
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        match outcome_for(passed, on_pass, on_fail, step.message.as_deref()) {
            GateOutcome::Continue => {
                // `output: None` — gate produces no exec output; the
                // snapshot slot exists for cmd steps only. Tokens and cost
                // are also absent, matching the v1 gate executor's
                // no-billable-IO semantics.
                // Gate-continue: record the branch the gate took so
                // replay never has to re-evaluate the condition (PR 3
                // of Task #31). `scope_context` is still `None` here —
                // the scope executor lands in a later commit.
                self.emit(Event::StepCompleted {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                    sandboxed: false,
                    output: None,
                    scope_context: None,
                    gate_outcome: Some(stepyard_core::GateOutcome::Continue),
                    agent_session_id: None,
                })
                .await?;
                Ok(StepOutcome::StepCompleted {
                    step_name: step.name.clone(),
                })
            }
            GateOutcome::Fail { message } => {
                let _ = step_index; // reserved for a future typed termination reason
                let error = GateError::Failed {
                    step: step.name.clone(),
                    message,
                }
                .to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                })
            }
            // Top-level gates route through GateAction::parse, which
            // rejects `skip`/`break` — those actions require a containing
            // scope. The scope runner lands in a later commit and calls
            // run_gate_step_scoped, which routes the scope-only outcomes
            // through its own match arm.
            GateOutcome::Skip | GateOutcome::Break => unreachable!(
                "top-level gate cannot produce Skip/Break — GateAction::parse rejects these outside a scope"
            ),
        }
    }

    /// Shared emit helper for gate validation/render/eval errors —
    /// keeps every bail-out branch in [`Self::run_gate_step`] a one-liner.
    async fn emit_gate_failure(
        &mut self,
        step: &Step,
        step_type: &'static str,
        start: Instant,
        error: String,
    ) -> Result<StepOutcome, EngineError> {
        let duration_ms = start.elapsed().as_millis() as u64;
        self.emit(Event::StepFailed {
            step_name: step.name.clone(),
            step_type: step_type.into(),
            error: error.clone(),
            duration_ms,
            timestamp: Utc::now(),
            sandboxed: false,
        })
        .await?;
        self.finalise_fail().await?;
        Ok(StepOutcome::StepFailed {
            step_name: step.name.clone(),
            error,
        })
    }

    /// Execute a [`StepKind::Template`] step — read the prompt file under
    /// the workflow's `prompts_dir`, render it against the current
    /// context, and persist the rendered text as `stdout` in a
    /// `StepCompleted` event. Cross-step refs (`{{ steps.tmpl.stdout }}`)
    /// see the output without a new event-schema variant. PR 4 of Task #31.
    ///
    /// No sandbox, no tokio select — template is pure filesystem +
    /// in-memory Tera. Replay comes for free: if the log already has a
    /// `StepCompleted` for this step, [`Self::progress_from_log`]
    /// advances past it and the runner never re-enters this method.
    async fn run_template_step(
        &mut self,
        step: &Step,
        outputs: &HashMap<String, StepOutputSnapshot>,
        start: Instant,
        _step_index: u32,
    ) -> Result<StepOutcome, EngineError> {
        let step_type = step_type_label(&step.kind);

        let prompts_dir = std::path::PathBuf::from(
            self.workflow
                .prompts_dir
                .as_deref()
                .unwrap_or(crate::template_exec::DEFAULT_PROMPTS_DIR),
        );
        let ctx = RenderContext {
            steps: outputs,
            target: &self.run_context.target,
            vars: &self.run_context.vars,
            scope: None,
        };
        let rendered = match crate::template_exec::render_template(
            &prompts_dir,
            step.prompt.as_deref(),
            &step.name,
            &ctx,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        let snapshot = stepyard_core::StepOutputSnapshot {
            stdout: rendered,
            stderr: String::new(),
            exit_code: 0,
        };
        self.emit(Event::StepCompleted {
            step_name: step.name.clone(),
            step_type: step_type.into(),
            duration_ms: start.elapsed().as_millis() as u64,
            timestamp: Utc::now(),
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
            sandboxed: false,
            output: Some(snapshot),
            scope_context: None,
            gate_outcome: None,
            agent_session_id: None,
        })
        .await?;
        Ok(StepOutcome::StepCompleted {
            step_name: step.name.clone(),
        })
    }

    /// Execute a [`StepKind::Script`] step — evaluate the Rhai source
    /// stored in `step.command` against a snapshot built from the
    /// cross-step outputs + target, and persist the rendered value as
    /// `stdout` in a `StepCompleted` event. PR 4 of Task #31.
    ///
    /// No sandbox, no tokio select — the Rhai engine runs synchronously on
    /// the current task, bounded by its `MAX_OPERATIONS` cap. Replay is
    /// the same story as template: a completed event in the log makes
    /// `progress_from_log` advance past this step, so we never re-evaluate.
    async fn run_script_step(
        &mut self,
        step: &Step,
        outputs: &HashMap<String, StepOutputSnapshot>,
        start: Instant,
        _step_index: u32,
    ) -> Result<StepOutcome, EngineError> {
        let step_type = step_type_label(&step.kind);

        let rendered = match crate::script_exec::execute_script(
            &step.name,
            &step.command,
            outputs,
            &self.run_context.target,
        ) {
            Ok(s) => s,
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        let snapshot = stepyard_core::StepOutputSnapshot {
            stdout: rendered,
            stderr: String::new(),
            exit_code: 0,
        };
        self.emit(Event::StepCompleted {
            step_name: step.name.clone(),
            step_type: step_type.into(),
            duration_ms: start.elapsed().as_millis() as u64,
            timestamp: Utc::now(),
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
            sandboxed: false,
            output: Some(snapshot),
            scope_context: None,
            gate_outcome: None,
            agent_session_id: None,
        })
        .await?;
        Ok(StepOutcome::StepCompleted {
            step_name: step.name.clone(),
        })
    }

    /// Execute a [`StepKind::Agent`] step — resolve env, render the
    /// prompt against the current [`RenderContext`], spawn the Claude CLI
    /// via [`crate::agent_exec::run_agent_step`] inside a four-arm
    /// `tokio::select!`, and emit a unified `StepCompleted` carrying
    /// `{ stdout: response, stderr: "", exit_code: 0 }` plus
    /// `{ input_tokens, output_tokens, cost_usd, agent_session_id }` at
    /// the event level. PR 5a of Task #31.
    ///
    /// The select mirrors [`Self::execute_cmd_with_select`] verbatim:
    /// cancel / step-timeout / shutdown-broadcast / exec-done. The
    /// control-plane branches emit the same terminal events and map to
    /// the same outcomes as cmd, so replay, auditing, and external
    /// consumers can't tell cmd from agent for a cancelled/signalled/
    /// timed-out run:
    ///
    /// | Branch  | Events emitted                        | Return                                                       |
    /// | ------- | ------------------------------------- | ------------------------------------------------------------ |
    /// | Done(OK)| `StepCompleted`                       | `Ok(StepOutcome::StepCompleted)`                             |
    /// | Done(Err)| `StepFailed`                         | `Ok(StepOutcome::StepFailed)`                                |
    /// | Cancel  | `StepFailed` (Cancelled)              | `Ok(StepOutcome::Cancelled)`                                 |
    /// | Signal  | `SignalReceived` + `StepFailed`       | `Err(EngineError::StepFailed { SignalReceived })`            |
    /// | Timeout | `StepTimeoutFired` + `StepFailed`     | `Err(EngineError::StepFailed { StepTimeout { configured_ms } })` |
    ///
    /// `step_index` is read verbatim by `StepTimeoutFired.step_index`
    /// and by the `EngineError::StepFailed` returned on timeout/signal;
    /// the caller is expected to pass the top-level index
    /// (`progress.completed_steps as u32`).
    ///
    /// Replay is the same story as every other step: once the session log
    /// holds a terminal event for this step, [`Self::progress_from_log`]
    /// advances past it and the runner never re-enters this method. That
    /// is how the log's first-wins `first_agent_session_id` capture
    /// survives a crash — the event with the captured `agent_session_id`
    /// is durable before we return.
    async fn run_agent_step(
        &mut self,
        step: &Step,
        progress: &Progress,
        start: Instant,
        step_index: u32,
    ) -> Result<StepOutcome, EngineError> {
        let step_type = step_type_label(&step.kind);

        // Env resolution: fail-fast on unresolved `${VAR}` (Story 3.4 AC2),
        // same contract as cmd. A missing host var is a user-config bug,
        // not a runtime bug — stop the workflow instead of running the CLI
        // with a half-built env.
        let resolved_env = match self.prepare_step(step) {
            Ok(env) => env,
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        // Adapter enforces `prompt: Some(_)` for agent kind
        // (`cli::harness_adapter::AdapterError::AgentMissingPrompt`), so
        // an absent prompt here means an in-process caller bypassed the
        // adapter. Surface it as StepFailed rather than unwrapping —
        // replay should show the breach, not panic.
        let Some(prompt_template) = step.prompt.as_deref() else {
            let error = format!(
                "agent step `{}` has no prompt — the adapter should have rejected this at load time",
                step.name
            );
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error: error.clone(),
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_fail().await?;
            return Ok(StepOutcome::StepFailed {
                step_name: step.name.clone(),
                error,
            });
        };

        let ctx = RenderContext {
            steps: &progress.outputs,
            target: &self.run_context.target,
            vars: &self.run_context.vars,
            scope: None,
        };
        let rendered_prompt = match render(prompt_template, &ctx) {
            Ok(s) => s,
            Err(e) => {
                let error = format!("agent prompt render failed: {e}");
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        let state = crate::agent_exec::AgentSessionState {
            agent_session_ids: &progress.agent_session_ids,
            first_agent_session_id: progress.first_agent_session_id.as_deref(),
        };

        // Race the exec future against cancel / timeout / shutdown. Same
        // shape as `execute_cmd_with_select` — keeping the two inline
        // instead of factoring out a generic helper avoids dragging the
        // battle-tested cmd path through the agent refactor's blast
        // radius. `.kill_on_drop(true)` on the child inside `agent_exec`
        // guarantees SIGKILL when any non-Done branch drops this future.
        let cancel_token = self.cancel.clone();
        let signal_slot = self.config.shutdown_signal.clone();
        let step_timeout = step.timeout;
        let selection = {
            // Completion-signal matching happens inside the stdout reader
            // below. It races naturally with this outer timeout/cancel/signal
            // select: if a timeout and a matching stdout read become ready in
            // the same tick, the first `tokio::select!` arm selected wins.
            let exec_fut = crate::agent_exec::run_agent_step(
                step,
                &rendered_prompt,
                &state,
                &resolved_env,
                self.workflow.completion_signal.as_deref(),
            );
            let cancel_fut = async {
                while !cancel_token.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            let timeout_fut = async {
                match step_timeout {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            };
            let shutdown_rx = &mut self.shutdown_rx;
            let shutdown_fut = shutdown_rx.recv();
            tokio::pin!(exec_fut);
            tokio::pin!(cancel_fut);
            tokio::pin!(timeout_fut);
            tokio::pin!(shutdown_fut);
            tokio::select! {
                r = &mut exec_fut => StepSelection::Done(r),
                _ = &mut cancel_fut => StepSelection::Cancelled,
                _ = &mut timeout_fut => StepSelection::TimedOut,
                _ = &mut shutdown_fut => StepSelection::Signal(
                    signal_slot
                        .get()
                        .cloned()
                        .unwrap_or_else(|| "unknown".into()),
                ),
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        // Signal landed first. D5 emit-before-IO ordering: persist both
        // facts to the session log before returning. Agent has no
        // sandbox to destroy, so `sandboxed: false` and no lifecycle
        // call — otherwise identical to cmd's signal arm.
        if let StepSelection::Signal(ref signal) = selection {
            let signal = signal.clone();
            self.emit(Event::SignalReceived {
                signal: Signal::from(signal.clone()),
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error: format!("Signal: {signal}"),
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_cancel().await?;
            return Err(EngineError::StepFailed {
                step_index,
                reason: stepyard_core::TerminationReason::SignalReceived(Signal::from(signal)),
            });
        }

        // Timeout landed first. `step.timeout` is guaranteed Some here
        // because the timeout arm used `std::future::pending` when it
        // was None. Agent has no sandbox to destroy, so we skip the
        // `lifecycle.destroy_by_session` call cmd makes.
        if let StepSelection::TimedOut = selection {
            let configured_ms = step
                .timeout
                .expect("TimedOut requires step.timeout.is_some()")
                .as_millis() as u64;
            self.emit(Event::StepTimeoutFired {
                step_index,
                configured_ms,
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error: format!("step timed out after {configured_ms}ms"),
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_fail().await?;
            return Err(EngineError::StepFailed {
                step_index,
                reason: stepyard_core::TerminationReason::StepTimeout { configured_ms },
            });
        }

        // Cancel landed mid-step.
        let exec_result = match selection {
            StepSelection::Done(r) => r,
            StepSelection::Cancelled => {
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: "Cancelled".into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_cancel().await?;
                return Ok(StepOutcome::Cancelled);
            }
            StepSelection::TimedOut => unreachable!("handled above"),
            StepSelection::Signal(_) => unreachable!("handled above"),
        };

        let output = match exec_result {
            Ok(o) => o,
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        if output.completion_signaled {
            let signal = self
                .workflow
                .completion_signal
                .clone()
                .expect("completion_signaled requires workflow.completion_signal");
            self.emit(Event::CompletionSignaled { step_index, signal })
                .await?;
            self.finalise_success().await?;
            return Ok(StepOutcome::WorkflowCompleted);
        }

        // Snapshot follows the template/script convention: the unified
        // `{ stdout, stderr, exit_code }` shape so `{{ steps.ask.stdout }}`
        // resolves without a schema change. The child's actual process
        // exit code intentionally does *not* leak here — v1 treats a
        // non-zero exit with a captured response as SUCCESS (tool_use
        // failure with a fallback response), and `agent_exec` has already
        // honored that rule before returning an `Ok(_)`.
        let snapshot = StepOutputSnapshot {
            stdout: output.response,
            stderr: String::new(),
            exit_code: 0,
        };
        self.emit(Event::StepCompleted {
            step_name: step.name.clone(),
            step_type: step_type.into(),
            duration_ms,
            timestamp: Utc::now(),
            input_tokens: output.input_tokens,
            output_tokens: output.output_tokens,
            cost_usd: output.cost_usd,
            sandboxed: false,
            output: Some(snapshot),
            scope_context: None,
            gate_outcome: None,
            agent_session_id: output.session_id,
        })
        .await?;
        Ok(StepOutcome::StepCompleted {
            step_name: step.name.clone(),
        })
    }

    /// Execute a [`StepKind::Chat`] step — resolve env, render the
    /// prompt, emit the user turn, invoke the injected
    /// [`ChatClient`] seam, emit the assistant turn, and land a
    /// unified `StepCompleted` carrying `{ stdout: response }` plus
    /// `{ input_tokens, output_tokens, cost_usd }` at the event
    /// level. PR 5c commit 2 of Task #31.
    ///
    /// # Atomicity
    ///
    /// Emission order
    /// `StepStarted → ChatMessageAppended (user) → ChatMessageAppended (assistant) → StepCompleted`
    /// is the boundary `compute_progress` relies on — chat turns
    /// stage under `step_name` on emission and only promote into
    /// `chat_sessions` when the terminal `StepCompleted` with
    /// `step_type: "chat"` commits. A runtime that reorders or
    /// drops one of these events silently corrupts replayed chat
    /// history on a crash.
    ///
    /// # Cancellation contract
    ///
    /// Commit 4 wraps the provider call in the same
    /// `tokio::select!` race the cmd and agent paths run, so a
    /// cancel-token flip, SIGINT/SIGTERM broadcast, or expired
    /// `step.timeout` wins against a slow `ChatClient::complete`.
    /// `step_index` is required for the
    /// `EngineError::StepFailed{step_index, reason}` shape on the
    /// signal/timeout arms; the `Cancelled` arm returns
    /// `Ok(StepOutcome::Cancelled)` symmetric to agent. The
    /// staged user turn at `pending_chat_turns[step_name]` is
    /// dropped (never promoted) on every non-`Done` arm because
    /// `compute_progress` only promotes on a terminal
    /// `StepCompleted{step_type:"chat"}` — atomicity preserved
    /// without a separate transactional rollback.
    async fn run_chat_step(
        &mut self,
        step: &Step,
        progress: &Progress,
        start: Instant,
        step_index: u32,
    ) -> Result<StepOutcome, EngineError> {
        let step_type = step_type_label(&step.kind);

        // Env resolution: same fail-fast contract as cmd/agent. A
        // missing `${VAR}` is a user-config bug, not a runtime bug —
        // stop the workflow instead of invoking the provider with a
        // half-built env.
        let resolved_env = match self.prepare_step(step) {
            Ok(env) => env,
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        // `parse_chat_step` enforces `prompt: Some(_)` for chat going
        // through the adapter. In-process callers that bypass the
        // adapter still get a typed StepFailed rather than a panic.
        let Some(prompt_template) = step.prompt.as_deref() else {
            let error = crate::chat_exec::ChatExecError::MissingPrompt {
                step: step.name.clone(),
            }
            .to_string();
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error: error.clone(),
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_fail().await?;
            return Ok(StepOutcome::StepFailed {
                step_name: step.name.clone(),
                error,
            });
        };

        let ctx = RenderContext {
            steps: &progress.outputs,
            target: &self.run_context.target,
            vars: &self.run_context.vars,
            scope: None,
        };
        let rendered_prompt = match render(prompt_template, &ctx) {
            Ok(s) => s,
            Err(e) => {
                let error = format!("chat prompt render failed: {e}");
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        // Session bucket: explicit `chat_session:` or the step name
        // as fallback, so `compute_progress` keys history on a
        // stable identifier across reruns.
        let session_bucket = step
            .chat_session
            .clone()
            .unwrap_or_else(|| step.name.clone());

        // History reconstructed by compute_progress. First turn in a
        // bucket sees an empty vec; later turns see every
        // previously-committed turn in log-append order.
        let history = progress
            .chat_sessions
            .get(&session_bucket)
            .cloned()
            .unwrap_or_default();

        // Resolve `api_key_env: VAR_NAME` per the field contract at
        // `workflow::Step::api_key_env`: the value is the NAME of an
        // env var, not a key into the merged workflow map. Precedence
        // (workflow env override -> host env fallback) lives in
        // `chat_exec::resolve_api_key` so unit tests can pin it
        // without standing up the engine. Missing everywhere is a
        // user-config problem — fail loud as StepFailed, same
        // contract as env resolution.
        let resolved_api_key = match crate::chat_exec::resolve_api_key(
            &step.name,
            step.api_key_env.as_deref(),
            &resolved_env,
        ) {
            Ok(v) => v,
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        // Client injection. The adapter accepts chat steps as of
        // PR 5c commit 3, but the production default `chat_client`
        // wiring is deferred to commit 3b — until then a CLI run of
        // `type: chat` surfaces a typed StepFailed here rather than
        // an `UnsupportedStepType` from the adapter.
        let Some(client) = self.config.chat_client.clone() else {
            let error = crate::chat_exec::ChatExecError::NoClientConfigured {
                step: step.name.clone(),
            }
            .to_string();
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error: error.clone(),
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_fail().await?;
            return Ok(StepOutcome::StepFailed {
                step_name: step.name.clone(),
                error,
            });
        };

        // Persist the user turn BEFORE calling the provider so
        // replay sees the prompt even if the call fails. The turn
        // stages under `step_name` in `pending_chat_turns` and is
        // dropped on the terminal StepFailed arm, matching the
        // atomicity contract PR 5b locked in.
        self.emit(Event::ChatMessageAppended {
            step_name: step.name.clone(),
            session: session_bucket.clone(),
            role: stepyard_core::ChatRole::User,
            content: rendered_prompt.clone(),
            timestamp: Utc::now(),
        })
        .await?;

        let req =
            crate::chat_exec::build_chat_request(step, rendered_prompt, history, resolved_api_key);

        // Race the provider call against cancel / timeout / shutdown.
        // Same shape as agent (engine.rs:1252-1373) and cmd's
        // `execute_cmd_with_select` — keeping this inline rather than
        // factoring out a generic helper avoids dragging the chat
        // path through the cmd/agent refactor blast radius. The
        // chat seam at `chat_exec::run_chat_step` is itself pure
        // `.await`; cancellation works because the future's drop
        // glue cancels the underlying `tokio::time::sleep` /
        // `reqwest` future cooperatively (advisor watchout: any
        // chat client that uses `std::thread::sleep` or otherwise
        // blocks the runtime thread will defeat this).
        let cancel_token = self.cancel.clone();
        let signal_slot = self.config.shutdown_signal.clone();
        let step_timeout = step.timeout;
        let selection = {
            let exec_fut = crate::chat_exec::run_chat_step(step, req, client.as_ref());
            let cancel_fut = async {
                while !cancel_token.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            let timeout_fut = async {
                match step_timeout {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            };
            let shutdown_rx = &mut self.shutdown_rx;
            let shutdown_fut = shutdown_rx.recv();
            tokio::pin!(exec_fut);
            tokio::pin!(cancel_fut);
            tokio::pin!(timeout_fut);
            tokio::pin!(shutdown_fut);
            tokio::select! {
                r = &mut exec_fut => StepSelection::Done(r),
                _ = &mut cancel_fut => StepSelection::Cancelled,
                _ = &mut timeout_fut => StepSelection::TimedOut,
                _ = &mut shutdown_fut => StepSelection::Signal(
                    signal_slot
                        .get()
                        .cloned()
                        .unwrap_or_else(|| "unknown".into()),
                ),
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        // Signal arm. D5 emit-before-IO ordering: `signal_received`
        // then `step_failed`, then flip the session into its
        // terminal status via `finalise_cancel`. Mirrors agent at
        // engine.rs:1303-1322. `ChatExecError::SignalReceived`
        // provides the centralised message format.
        if let StepSelection::Signal(ref signal) = selection {
            let signal = Signal::from(signal.clone());
            let error = crate::chat_exec::ChatExecError::SignalReceived {
                step: step.name.clone(),
                signal: signal.clone(),
            }
            .to_string();
            self.emit(Event::SignalReceived {
                signal: signal.clone(),
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error,
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_cancel().await?;
            return Err(EngineError::StepFailed {
                step_index,
                reason: stepyard_core::TerminationReason::SignalReceived(signal),
            });
        }

        // Timeout arm. `step.timeout` is guaranteed `Some` here
        // because the timeout future used `std::future::pending`
        // when it was `None`. Mirrors agent at engine.rs:1329-1353.
        if let StepSelection::TimedOut = selection {
            let configured_ms = step
                .timeout
                .expect("TimedOut requires step.timeout.is_some()")
                .as_millis() as u64;
            let error = crate::chat_exec::ChatExecError::Timeout {
                step: step.name.clone(),
                configured_ms,
            }
            .to_string();
            self.emit(Event::StepTimeoutFired {
                step_index,
                configured_ms,
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: step_type.into(),
                error,
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_fail().await?;
            return Err(EngineError::StepFailed {
                step_index,
                reason: stepyard_core::TerminationReason::StepTimeout { configured_ms },
            });
        }

        // Cancel arm. Returns `Ok(StepOutcome::Cancelled)` so the
        // outer `step` loop terminates cleanly without bubbling an
        // error — symmetric with cmd / agent.
        let exec_result = match selection {
            StepSelection::Done(r) => r,
            StepSelection::Cancelled => {
                let error = crate::chat_exec::ChatExecError::Cancelled {
                    step: step.name.clone(),
                }
                .to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error,
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_cancel().await?;
                return Ok(StepOutcome::Cancelled);
            }
            StepSelection::TimedOut => unreachable!("handled above"),
            StepSelection::Signal(_) => unreachable!("handled above"),
        };

        let output = match exec_result {
            Ok(o) => o,
            Err(e) => {
                let error = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: step.name.clone(),
                    step_type: step_type.into(),
                    error: error.clone(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                });
            }
        };

        // Assistant turn THEN StepCompleted — the atomic boundary
        // compute_progress relies on. If the assistant append
        // fails, the next scan sees only the staged user turn for
        // this step_name, and the next `step_started` will drop it
        // on rerun (Invariante 11).
        self.emit(Event::ChatMessageAppended {
            step_name: step.name.clone(),
            session: session_bucket,
            role: stepyard_core::ChatRole::Assistant,
            content: output.response.clone(),
            timestamp: Utc::now(),
        })
        .await?;

        let snapshot = StepOutputSnapshot {
            stdout: output.response,
            stderr: String::new(),
            exit_code: 0,
        };
        self.emit(Event::StepCompleted {
            step_name: step.name.clone(),
            step_type: step_type.into(),
            duration_ms,
            timestamp: Utc::now(),
            input_tokens: output.input_tokens,
            output_tokens: output.output_tokens,
            cost_usd: output.cost_usd,
            sandboxed: false,
            output: Some(snapshot),
            scope_context: None,
            gate_outcome: None,
            agent_session_id: None,
        })
        .await?;
        Ok(StepOutcome::StepCompleted {
            step_name: step.name.clone(),
        })
    }

    // ── Internals ───────────────────────────────────────────────────────

    pub(crate) async fn emit(&self, event: Event) -> Result<(), EngineError> {
        let payload = serde_json::to_value(&event)
            .map_err(|e| EngineError::InvalidState(format!("serialize: {e}")))?;
        self.session.append(payload).await?;
        Ok(())
    }

    pub(crate) async fn progress_from_log(&self) -> Result<Progress, EngineError> {
        let events = self.session.replay().await?;
        compute_progress(&events)
    }

    async fn finalise_success(&mut self) -> Result<(), EngineError> {
        if self.session.status() == SessionStatus::Running {
            self.finalize_workspace_success_if_configured().await?;
            let duration_ms = self
                .started_at
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0);
            self.emit(Event::WorkflowCompleted {
                duration_ms,
                timestamp: Utc::now(),
            })
            .await?;
            self.session.complete().await?;
        }
        Ok(())
    }

    pub(crate) async fn finalise_cancel(&mut self) -> Result<(), EngineError> {
        if self.session.status() == SessionStatus::Running {
            // Emit-before-IO (G4): record the cancel decision in the session
            // log *before* tearing down the sandbox, so a crash during
            // `destroy_by_session` cannot leave a session whose final state
            // is unrecoverable from the log alone. Mirrors `finalise_success`
            // shape — same duration_ms / timestamp computation.
            let duration_ms = self
                .started_at
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0);
            self.emit(Event::WorkflowCancelled {
                duration_ms,
                timestamp: Utc::now(),
            })
            .await?;
            // Tear down the sandbox by session UUID — cattle, no regrets.
            // Backends like Docker cannot map a bare SandboxId to the
            // container they created, so the trait teardown contract is
            // `destroy_by_session(uuid)`.
            let session_uuid = *self.session.id().as_uuid();
            let _ = self.lifecycle.destroy_by_session(session_uuid).await;
            self.session.cancel().await?;
        }
        Ok(())
    }

    pub(crate) async fn finalise_fail(&mut self) -> Result<(), EngineError> {
        if self.session.status() == SessionStatus::Running {
            self.session.fail().await?;
        }
        Ok(())
    }

    /// Whether [`CancelToken::cancel`] has fired. Exposed for the scope
    /// runner's loop guards (PR 3 of Task #31) — it checks the same flag
    /// as the top-level `step()` fast-path so a cancel between two scope
    /// iterations short-circuits cleanly.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Workflow attached to this engine. Scope bodies live in
    /// [`Step::body`] on the container step, which the runner resolves
    /// through this accessor.
    pub(crate) fn workflow(&self) -> &Workflow {
        &self.workflow
    }

    /// Per-run renderer inputs (CLI `--target` + `--var k=v`). Scope body
    /// rendering joins these with the per-iteration `scope` binding.
    pub(crate) fn run_context(&self) -> &RunContext {
        &self.run_context
    }

    pub(crate) fn shutdown_signal(&self) -> Arc<OnceLock<String>> {
        self.config.shutdown_signal.clone()
    }

    pub(crate) fn chat_client(&self) -> Option<Arc<dyn ChatClient>> {
        self.config.chat_client.clone()
    }

    pub(crate) fn lifecycle(&self) -> Arc<dyn SandboxLifecycle> {
        self.lifecycle.clone()
    }

    pub(crate) fn shutdown_rx_mut(&mut self) -> &mut broadcast::Receiver<()> {
        &mut self.shutdown_rx
    }

    /// Session handle — scope replay rescans the log to reconstruct the
    /// per-container iteration counter / last-iteration-completed state
    /// without holding per-container memory on the engine.
    pub(crate) fn session_handle(&self) -> &Session {
        &self.session
    }

    /// Cloned executor handle. The scope runner spawns parallel sub-tasks
    /// into a `JoinSet` (PR 6 of #31) and so cannot borrow `self`; each
    /// task gets an `Arc` clone instead.
    pub(crate) fn executor(&self) -> Arc<dyn StepExecutor> {
        self.executor.clone()
    }
}

#[derive(Debug)]
pub(crate) struct Progress {
    pub(crate) completed_steps: usize,
    pub(crate) has_failure: bool,
    pub(crate) last_failed_step: Option<String>,
    pub(crate) workflow_completed: bool,
    /// Cross-step outputs map rebuilt from the session log on every
    /// `progress_from_log` call. Gate steps read this to render
    /// `{{ steps.X.stdout }}`; cmd steps don't consume it yet (the
    /// template pass over `command` is deferred to a later PR of #31).
    /// PR 2 of Task #31.
    pub(crate) outputs: HashMap<String, StepOutputSnapshot>,
    /// Captured Claude CLI `session_id` keyed by top-level agent step
    /// name, rebuilt on every scan. The v2 agent executor consumes this
    /// to resolve explicit `resume: <step_name>` / `fork_session:
    /// <step_name>` argv from the log alone — Invariante 11 means the
    /// live-run `SessionManager` is gone after a crash, so the session
    /// log is the only source of truth. PR 5a of Task #31.
    ///
    /// Only top-level completions contribute. Scope-nested agent steps
    /// (`call` / `repeat` / `map` bodies) are intentionally skipped —
    /// scope-aware session capture is a follow-up design question and
    /// v1 never had scopes, so v1 parity is trivially preserved.
    ///
    /// Last-write-wins on duplicate step names. Workflow validation
    /// rejects duplicates at the adapter boundary, so this branch is
    /// unreachable in practice; the semantics here is just "don't
    /// special-case it".
    pub(crate) agent_session_ids: HashMap<String, String>,
    /// First-wins top-level agent session_id, mirroring v1's
    /// `SessionManager::capture` semantics. Consumed by the v2 agent
    /// executor for the workflow-level `session: shared` default path:
    /// the first successfully-completed agent step's session_id becomes
    /// the workflow's shared session for every later agent that did not
    /// name an explicit `resume:` / `fork_session:` target. PR 5a of
    /// Task #31.
    ///
    /// `None` on a fresh log and on logs containing only non-agent
    /// completions. Scope-nested completions do not contribute (see
    /// [`Self::agent_session_ids`]).
    pub(crate) first_agent_session_id: Option<String>,
    /// Chat-session history keyed by the session bucket name (the
    /// `session` field on [`Event::ChatMessageAppended`]). Each bucket
    /// holds the **committed** turns in log-append order, so a post-
    /// crash replay of a `session: shared` chat step sees the same
    /// conversation the pre-crash run durably produced — Invariante 11
    /// means there's no in-memory chat history between `step` calls,
    /// so the log is the only source of truth (PR 5b of Task #31).
    ///
    /// Commit boundary mirrors [`Event::StepCompleted`]'s semantic for
    /// `output` and `agent_session_id`: a turn only lands in this map
    /// when its producing chat step reaches a non-scoped
    /// [`Event::StepCompleted`] with `step_type: "chat"`. Turns
    /// emitted by a step that crashed before completion (no
    /// StepCompleted), failed, or is still in flight are discarded.
    /// A rerun after resume re-stages from scratch, so the rebuilt
    /// history always matches the durable-run view — a chat step's
    /// second attempt never sees a "phantom" turn from an abandoned
    /// first attempt.
    ///
    /// Malformed rows (missing/invalid `step_name`, `session`, `role`,
    /// or `content`) abort the scan with [`EngineError::InvalidState`]
    /// — silent drop would let a corrupted log hand the next turn a
    /// different history than the live run saw, breaking the exact
    /// determinism this scan enforces.
    ///
    /// Consumed by [`Engine::run_chat_step`] — a chat step's first
    /// read here becomes the `history` it hands the
    /// [`crate::chat_exec::ChatClient`] seam, so a post-crash rerun
    /// sees the same prior turns the pre-crash run durably
    /// produced. PR 5c of Task #31 activates this field; PR 5b
    /// shipped the staging contract first.
    pub(crate) chat_sessions: HashMap<String, Vec<ChatMessage>>,
}

/// Pure scan from a session's persisted events to replay state.
///
/// `progress_from_log` is a one-line delegate over this — factoring the
/// parsing out lets the scan be unit-tested with hand-constructed
/// [`SessionEvent`] values, without a Postgres-backed [`Session`].
///
/// Must stay pure: no IO, no time, no env. Every replay-relevant fact
/// comes from `events`.
fn compute_progress(events: &[SessionEvent]) -> Result<Progress, EngineError> {
    let mut completed = 0usize;
    let mut has_failure = false;
    let mut last_failed: Option<String> = None;
    let mut workflow_completed = false;
    // Rebuild the top-level cross-step outputs map (PR 2 of Task #31)
    // in the same scan as the progress counters — one pass of the log
    // per `step()` call. PR 3 of Task #31 widens this to ignore
    // `step_completed` events that carry a `scope_context`: those are
    // scope-body steps and must NOT advance the top-level step index
    // nor pollute the cross-step refs a later top-level template
    // resolves. Scope bodies rebuild their own local snapshot map by
    // re-scanning the log from within `scope.rs`.
    let mut outputs: HashMap<String, StepOutputSnapshot> = HashMap::new();
    // PR 5a of Task #31: per-step-name session_id map + first-wins
    // workflow-level captured id. Populated from the same non-scoped
    // `step_completed` arm as `outputs` — see the field docs on
    // `Progress` for the v1-parity reasoning behind skipping scoped
    // completions.
    let mut agent_session_ids: HashMap<String, String> = HashMap::new();
    let mut first_agent_session_id: Option<String> = None;
    // PR 5b/3 of Task #31: rebuild the chat-session history from the
    // log in the same scan. Turns stage by `step_name` in
    // `pending_chat_turns` and only commit into `chat_sessions` when a
    // matching non-scoped `step_completed` with `step_type: "chat"`
    // lands — the same atomicity boundary PR 2 gave `outputs` and PR
    // 5a gave `agent_session_ids`. Turns from a step that crashes
    // before completion, fails, or is still in flight are discarded;
    // a `step_started` for the same `step_name` clears the stage so a
    // post-resume rerun re-stages from scratch (Invariante 11).
    let mut chat_sessions: HashMap<String, Vec<ChatMessage>> = HashMap::new();
    let mut pending_chat_turns: HashMap<String, Vec<(String, ChatMessage)>> = HashMap::new();

    for evt in events.iter() {
        let Some(tag) = evt.payload.get("event").and_then(|v| v.as_str()) else {
            continue;
        };
        match tag {
            "workflow_completed" | "completion_signaled" => {
                workflow_completed = true;
            }
            "step_started" => {
                // PR 5b/3 of #31: a fresh StepStarted for `step_name`
                // is the rerun boundary for staged chat turns. The
                // runtime re-emits StepStarted when resume retries an
                // incomplete step, so clearing the stage here drops any
                // turns left behind by the crashed attempt and lets the
                // retry re-stage from scratch. Non-chat step_started
                // events miss the map entirely — `remove` is a no-op,
                // so there's no branch on `step_type`.
                if let Some(name) = evt.payload.get("step_name").and_then(|v| v.as_str()) {
                    pending_chat_turns.remove(name);
                }
            }
            "step_completed" => {
                // Scope-body completions carry a `scope_context` object;
                // top-level completions either omit the field (legacy
                // logs + PR 2 shape) or carry `null`. Short-circuit both
                // so the top-level counter only sees its own axis.
                let is_scoped = evt
                    .payload
                    .get("scope_context")
                    .map(|v| !v.is_null())
                    .unwrap_or(false);
                if is_scoped {
                    continue;
                }
                completed += 1;
                let step_name = evt
                    .payload
                    .get("step_name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                // PR 5b/3 of #31: `step_type` gates the chat-history
                // drain below. Non-chat completions leave
                // `pending_chat_turns` alone (no drain, no drop),
                // which preserves the commit boundary when an agent
                // step completes between two still-in-flight chat
                // steps.
                let step_type = evt
                    .payload
                    .get("step_type")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                // Absent `output` stays OK — that's how log entries
                // written before the PR 2 widening look, and how gate
                // steps (`output: None`) look today. Present but
                // malformed must fail loudly: this payload now
                // participates in replay correctness, so silently
                // dropping it would make a gate in the rerun see a
                // different context than the gate in the original
                // run — the exact invariant this PR establishes.
                let snapshot = match evt.payload.get("output") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(raw) => Some(
                        serde_json::from_value::<StepOutputSnapshot>(raw.clone()).map_err(|e| {
                            EngineError::InvalidState(format!(
                                "step_completed log entry has malformed `output` payload: {e}"
                            ))
                        })?,
                    ),
                };
                // Same strictness contract as `output`: absent is fine
                // (legacy logs, non-agent steps), present-but-malformed
                // fails loudly so a corrupted log can't produce a
                // different resume argv than the live run.
                let captured_session_id = match evt.payload.get("agent_session_id") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(raw) => Some(
                        serde_json::from_value::<String>(raw.clone())
                            .map_err(|e| {
                                EngineError::InvalidState(format!(
                                    "step_completed log entry has malformed `agent_session_id` payload: {e}"
                                ))
                            })?,
                    ),
                };
                if let Some(name) = step_name {
                    if let Some(snap) = snapshot {
                        outputs.insert(name.clone(), snap);
                    }
                    // PR 5b/3 of #31: a non-scoped chat completion is
                    // the commit boundary for staged turns — drain
                    // under `name` into `chat_sessions[session]`.
                    // Non-chat completions don't touch the stage, so
                    // an agent step sharing a name with nothing else
                    // is a no-op here.
                    if step_type.as_deref() == Some("chat") {
                        if let Some(turns) = pending_chat_turns.remove(&name) {
                            for (session, msg) in turns {
                                chat_sessions.entry(session).or_default().push(msg);
                            }
                        }
                    }
                    if let Some(sid) = captured_session_id {
                        if first_agent_session_id.is_none() {
                            first_agent_session_id = Some(sid.clone());
                        }
                        agent_session_ids.insert(name, sid);
                    }
                }
            }
            "step_failed" => {
                has_failure = true;
                let step_name = evt
                    .payload
                    .get("step_name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                // PR 5b/3 of #31: a failed chat step must not pollute
                // history. The runtime treats StepFailed as terminal
                // (no rerun), so any staged turns the attempt emitted
                // are abandoned for good. `remove` on a non-chat name
                // is a no-op.
                if let Some(ref name) = step_name {
                    pending_chat_turns.remove(name);
                }
                last_failed = step_name;
            }
            "chat_message_appended" => {
                // PR 5b/3 of #31: stage this turn by `step_name` — it
                // only becomes durable `chat_sessions` history when
                // the matching non-scoped StepCompleted lands (see
                // the `step_completed` arm above). Every payload is
                // replay-critical: a turn with missing step_name /
                // session / role / content would either mis-stage
                // (wrong bucket / wrong commit key) or silently hand
                // the next turn a shorter history than the live run
                // saw. So all four fields are strict — present +
                // well-typed, or the scan aborts with `InvalidState`.
                //
                // A typed decode of `role` + `content` into
                // `ChatMessage` doubles as the role-enum gate: serde
                // rejects any value outside `user` / `assistant` at
                // parse time.
                let step_name = match evt.payload.get("step_name") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    _ => {
                        return Err(EngineError::InvalidState(
                            "chat_message_appended log entry is missing or has malformed `step_name`"
                                .to_string(),
                        ));
                    }
                };
                let session = match evt.payload.get("session") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    _ => {
                        return Err(EngineError::InvalidState(
                            "chat_message_appended log entry is missing or has malformed `session`"
                                .to_string(),
                        ));
                    }
                };
                let role_raw = evt.payload.get("role").cloned().ok_or_else(|| {
                    EngineError::InvalidState(
                        "chat_message_appended log entry is missing `role`".to_string(),
                    )
                })?;
                let content_raw = evt.payload.get("content").cloned().ok_or_else(|| {
                    EngineError::InvalidState(
                        "chat_message_appended log entry is missing `content`".to_string(),
                    )
                })?;
                let message = serde_json::from_value::<ChatMessage>(serde_json::json!({
                    "role": role_raw,
                    "content": content_raw,
                }))
                .map_err(|e| {
                    EngineError::InvalidState(format!(
                        "chat_message_appended log entry has malformed `role`/`content` payload: {e}"
                    ))
                })?;
                pending_chat_turns
                    .entry(step_name)
                    .or_default()
                    .push((session, message));
            }
            _ => {}
        }
    }

    Ok(Progress {
        completed_steps: completed,
        has_failure,
        last_failed_step: last_failed,
        workflow_completed,
        outputs,
        agent_session_ids,
        first_agent_session_id,
        chat_sessions,
    })
}

/// Which branch of a step-level `tokio::select!` fired first.
///
/// Generic over the payload `T` so both cmd and agent can share this
/// enum: cmd instantiates it with `Result<ExecOutput, SandboxError>`
/// (the sandbox executor's return), agent with
/// `Result<AgentExecOutput, AgentExecError>` (the Claude CLI spawner's
/// return). The three control-plane branches (cancel/timeout/signal)
/// do not carry a payload and are identical across step kinds, so
/// keeping them here removes two otherwise-identical enums.
pub(crate) enum StepSelection<T> {
    /// The executor returned a result (either `Ok(output)` or `Err(e)`).
    Done(T),
    /// The cancel token was flipped mid-step.
    Cancelled,
    /// The configured wall-clock timeout elapsed before the executor returned.
    TimedOut,
    /// The per-process shutdown broadcast fired (Story 2.3). Payload is the
    /// lowercase signal name read from `HarnessConfig::shutdown_signal` at
    /// the moment the select arm resolved.
    Signal(String),
}

/// Result of [`Engine::execute_cmd_with_select`]. All variants except
/// [`CmdOutcome::Success`] have already emitted their terminal event(s)
/// and flipped the session into its terminal status, so callers just
/// map the variant onto a [`StepOutcome`] (top-level) or a scope
/// directive (scope runner) without any additional IO.
#[non_exhaustive]
#[derive(Debug)]
pub(crate) enum CmdOutcome {
    /// cmd completed with exit code 0. The snapshot has already been
    /// persisted on a `StepCompleted` event (with or without
    /// `scope_context` depending on the caller); it is handed back so
    /// the scope runner can feed subsequent scope-body steps and so
    /// container wrappers can attach it as the synthetic output.
    Success(stepyard_core::StepOutputSnapshot),
    /// cmd exited non-zero or the sandbox layer errored. `StepFailed` +
    /// `finalise_fail` already emitted; the string is the same error
    /// shown on the wire.
    Failed(String),
    /// Cancel token flipped mid-exec. `StepFailed("Cancelled")` +
    /// `finalise_cancel` already emitted.
    Cancelled,
    /// Process-wide shutdown broadcast fired (SIGINT/SIGTERM).
    /// `SignalReceived` + `StepFailed` + `finalise_cancel` already
    /// emitted. String is the lowercase signal name.
    Signal(String),
    /// Per-step wall-clock timeout fired. `StepTimeoutFired` +
    /// `StepFailed` + sandbox destroy + `finalise_fail` already
    /// emitted — the caller only needs to surface the configured
    /// duration via `EngineError::StepFailed { TerminationReason::StepTimeout }`.
    TimedOut { configured_ms: u64 },
    /// Output-idle timeout fired inside the sandbox backend.
    /// `IdleTimeoutFired` + `StepFailed` + sandbox destroy +
    /// `finalise_fail` already emitted.
    IdleTimedOut { idle_ms: u64 },
}

/// Overlay `defaults` < `workflow_env` < `step_env` (later layers win) and
/// expand any exact-form `${VAR}` value against the host process env.
///
/// Exposed as a free function so it is trivially unit-testable without
/// spinning up a full `Engine` (which needs a Postgres-backed `Session`).
/// [`Engine::prepare_step`] is a one-line delegate over this.
///
/// # Pattern
///
/// Matches values satisfying `^\$\{[A-Z0-9_]+\}$`. Inline expansions like
/// `"prefix-${VAR}-suffix"` are NOT recognized in MVP (Story 3.4 scope)
/// and pass through verbatim.
///
/// # Errors
///
/// Returns [`EngineError::InvalidState`] when a `${VAR}` reference has no
/// matching host variable. Message format is locked:
/// `"host env variable not set: {key}"` (lowercase, no trailing
/// punctuation) so any future taxonomy rename is a pure string
/// substitution.
pub fn resolve_env(
    defaults: &Defaults,
    workflow_env: &HashMap<String, String>,
    step_env: &HashMap<String, String>,
) -> Result<HashMap<String, String>, EngineError> {
    // Clone the weakest layer, then overlay stronger layers on top so
    // later-inserted keys win. `HashMap::extend` on duplicate keys
    // overwrites — matches the step > workflow > defaults precedence.
    let mut env = defaults.env.clone();
    env.extend(workflow_env.iter().map(|(k, v)| (k.clone(), v.clone())));
    env.extend(step_env.iter().map(|(k, v)| (k.clone(), v.clone())));

    for v in env.values_mut() {
        // Copy the captured var name out of `v` before mutating `v`.
        let var_name_opt = parse_host_var(v.as_str()).map(str::to_string);
        if let Some(var_name) = var_name_opt {
            let resolved = std::env::var(&var_name).map_err(|_| {
                EngineError::InvalidState(format!("host env variable not set: {var_name}"))
            })?;
            *v = resolved;
        }
    }
    Ok(env)
}

/// Match the exact-form `${VAR}` pattern and return the inner key.
///
/// Returns `Some("VAR")` when `value` is exactly `${...}` and the inner
/// characters are all uppercase ASCII, digits, or underscores. Returns
/// `None` otherwise — those values pass through unmodified by
/// [`resolve_env`].
fn parse_host_var(value: &str) -> Option<&str> {
    let inner = value.strip_prefix("${").and_then(|s| s.strip_suffix('}'))?;
    if inner.is_empty() {
        return None;
    }
    let valid = inner
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Some(inner)
    } else {
        None
    }
}

#[cfg(test)]
mod proxy_env_tests {
    use super::*;

    #[test]
    fn apply_anthropic_proxy_env_replaces_real_key() {
        let mut env = HashMap::from([
            ("ANTHROPIC_API_KEY".into(), "sk-ant-real".into()),
            ("GH_TOKEN".into(), "ghp_test".into()),
        ]);
        let proxy = AnthropicProxyEnv {
            port: 4242,
            auth_token: "session-token".into(),
        };
        apply_anthropic_proxy_env(&mut env, &proxy);
        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("session-token")
        );
        assert_eq!(
            env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("http://host.docker.internal:4242")
        );
        assert_eq!(env.get("GH_TOKEN").map(String::as_str), Some("ghp_test"));
    }
}

#[cfg(test)]
mod progress_tests {
    //! Unit tests for [`compute_progress`] — PR 5a of Task #31 adds the
    //! per-step-name `agent_session_ids` map and first-wins
    //! `first_agent_session_id` capture that the v2 agent executor
    //! consumes to rebuild session resume argv after a crash.
    //!
    //! These hand-construct [`SessionEvent`]s so the scan can be tested
    //! without a Postgres-backed [`Session`]. Integration coverage
    //! through a full engine replay lands in a later commit alongside
    //! `agent_exec.rs`.
    use super::*;
    use serde_json::json;
    use stepyard_session::SessionId;
    use uuid::Uuid;

    fn evt(seq: i64, payload: serde_json::Value) -> SessionEvent {
        SessionEvent {
            id: Uuid::new_v4(),
            session_id: SessionId::new(),
            seq,
            created_at: Utc::now(),
            payload,
        }
    }

    fn step_completed(step_name: &str, agent_session_id: Option<&str>) -> serde_json::Value {
        let mut v = json!({
            "event": "step_completed",
            "step_name": step_name,
            "step_type": "agent",
            "duration_ms": 100,
            "timestamp": "2026-04-20T00:00:00Z",
            "sandboxed": false,
        });
        if let Some(sid) = agent_session_id {
            v["agent_session_id"] = json!(sid);
        }
        v
    }

    #[test]
    fn empty_log_yields_empty_session_state() {
        let progress = compute_progress(&[]).expect("pure scan cannot fail on empty log");
        assert_eq!(progress.completed_steps, 0);
        assert!(progress.agent_session_ids.is_empty());
        assert_eq!(progress.first_agent_session_id, None);
    }

    #[test]
    fn single_agent_completion_populates_both_session_fields() {
        let events = vec![evt(1, step_completed("review", Some("ses_abc")))];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.completed_steps, 1);
        assert_eq!(
            progress.agent_session_ids.get("review"),
            Some(&"ses_abc".to_string())
        );
        assert_eq!(progress.first_agent_session_id, Some("ses_abc".into()));
    }

    #[test]
    fn second_agent_step_preserves_first_wins_captured_id() {
        // Mirrors v1's `SessionManager::capture` semantics: the first
        // agent's session_id wins for workflow-level `session: shared`,
        // regardless of how many agents run later.
        let events = vec![
            evt(1, step_completed("plan", Some("ses_one"))),
            evt(2, step_completed("review", Some("ses_two"))),
        ];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.first_agent_session_id, Some("ses_one".into()));
        assert_eq!(
            progress.agent_session_ids.get("plan"),
            Some(&"ses_one".to_string())
        );
        assert_eq!(
            progress.agent_session_ids.get("review"),
            Some(&"ses_two".to_string())
        );
    }

    #[test]
    fn first_wins_survives_interleaved_non_agent_completions() {
        let events = vec![
            evt(1, step_completed("setup_cmd", None)),
            evt(2, step_completed("first_agent", Some("ses_first"))),
            evt(3, step_completed("middle_cmd", None)),
            evt(4, step_completed("second_agent", Some("ses_second"))),
        ];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.first_agent_session_id, Some("ses_first".into()));
        assert_eq!(progress.agent_session_ids.len(), 2);
    }

    #[test]
    fn completion_without_agent_session_id_does_not_pollute_session_maps() {
        // Any completion lacking `agent_session_id` must flow through
        // the scan without touching either session field. Covers both
        // non-agent step kinds and agent completions that never produced
        // a session id (e.g. early CLI failure before the `result` line).
        let events = vec![evt(1, step_completed("build", None))];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.completed_steps, 1);
        assert!(progress.agent_session_ids.is_empty());
        assert_eq!(progress.first_agent_session_id, None);
    }

    #[test]
    fn scoped_completion_with_agent_session_id_is_ignored() {
        // Scope-nested agent captures don't contribute to the
        // top-level session maps — locks the v1-parity-first scope
        // documented on `Progress::agent_session_ids`. A future PR
        // that adds scope-aware session semantics can lift this by
        // widening the field, not by removing the guard.
        let mut payload = step_completed("inner_agent", Some("ses_scoped"));
        payload["scope_context"] = json!({
            "container": "loop",
            "iteration": 0,
            "position": 0,
        });
        let events = vec![evt(1, payload)];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.completed_steps, 0);
        assert!(progress.agent_session_ids.is_empty());
        assert_eq!(progress.first_agent_session_id, None);
    }

    #[test]
    fn legacy_log_without_agent_session_id_field_stays_clean() {
        // Pre-PR-5a logs never carry `agent_session_id`. The scan must
        // treat absence as `None` without error so replay across the
        // version boundary just works.
        let events = vec![evt(
            1,
            json!({
                "event": "step_completed",
                "step_name": "legacy",
                "step_type": "cmd",
                "duration_ms": 50,
                "timestamp": "2026-04-15T00:00:00Z",
                "sandboxed": false,
            }),
        )];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.completed_steps, 1);
        assert!(progress.agent_session_ids.is_empty());
        assert_eq!(progress.first_agent_session_id, None);
    }

    #[test]
    fn malformed_agent_session_id_payload_errors_loudly() {
        // A corrupted log row (non-string `agent_session_id`) must
        // surface as `InvalidState` — silent drop would let a corrupt
        // log produce a different resume argv than the live run,
        // breaking the exact determinism this scan enforces.
        let events = vec![evt(
            1,
            json!({
                "event": "step_completed",
                "step_name": "bad",
                "step_type": "agent",
                "duration_ms": 50,
                "timestamp": "2026-04-20T00:00:00Z",
                "sandboxed": false,
                "agent_session_id": 42,
            }),
        )];
        let err = compute_progress(&events).expect_err("numeric session_id must error");
        match err {
            EngineError::InvalidState(msg) => {
                assert!(
                    msg.contains("agent_session_id"),
                    "error must name the malformed field, got: {msg}"
                );
            }
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    fn chat_message_appended(
        step_name: &str,
        session: &str,
        role: &str,
        content: &str,
    ) -> serde_json::Value {
        json!({
            "event": "chat_message_appended",
            "step_name": step_name,
            "session": session,
            "role": role,
            "content": content,
            "timestamp": "2026-04-22T12:00:00Z",
        })
    }

    fn chat_step_started(step_name: &str) -> serde_json::Value {
        json!({
            "event": "step_started",
            "step_name": step_name,
            "step_type": "chat",
            "timestamp": "2026-04-22T12:00:00Z",
        })
    }

    fn chat_step_completed(step_name: &str) -> serde_json::Value {
        json!({
            "event": "step_completed",
            "step_name": step_name,
            "step_type": "chat",
            "duration_ms": 100,
            "timestamp": "2026-04-22T12:00:00Z",
            "sandboxed": false,
        })
    }

    fn chat_step_failed(step_name: &str) -> serde_json::Value {
        json!({
            "event": "step_failed",
            "step_name": step_name,
            "step_type": "chat",
            "error": "boom".to_string(),
            "duration_ms": 100,
            "timestamp": "2026-04-22T12:00:00Z",
            "sandboxed": false,
        })
    }

    #[test]
    fn empty_log_yields_empty_chat_sessions() {
        let progress = compute_progress(&[]).unwrap();
        assert!(progress.chat_sessions.is_empty());
    }

    #[test]
    fn orphan_chat_event_without_step_completed_is_dropped_from_chat_sessions() {
        // A `chat_message_appended` whose producing step never
        // reaches StepCompleted represents an in-flight or crashed
        // attempt. Committing it would hand the rerun a phantom turn
        // the durable log never contains — the exact atomicity bug
        // this commit boundary exists to prevent.
        let events = vec![evt(
            1,
            chat_message_appended("draft", "shared", "user", "hello"),
        )];
        let progress = compute_progress(&events).unwrap();
        // Chat turns aren't step completions — they must not advance
        // the top-level step index.
        assert_eq!(progress.completed_steps, 0);
        // And without a matching StepCompleted, the stage stays
        // pending and `chat_sessions` is empty.
        assert!(
            progress.chat_sessions.is_empty(),
            "orphan chat turn must not land in chat_sessions, got {:?}",
            progress.chat_sessions
        );
    }

    #[test]
    fn chat_turns_commit_only_after_matching_chat_step_completed() {
        // Positive commit path: StepStarted → two chat turns → a
        // non-scoped chat StepCompleted drains the stage into
        // `chat_sessions[session]`, preserving log-append order.
        let events = vec![
            evt(1, chat_step_started("draft")),
            evt(2, chat_message_appended("draft", "shared", "user", "q1")),
            evt(
                3,
                chat_message_appended("draft", "shared", "assistant", "a1"),
            ),
            evt(4, chat_step_completed("draft")),
        ];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.completed_steps, 1);
        let bucket = progress.chat_sessions.get("shared").expect("shared bucket");
        assert_eq!(
            bucket
                .iter()
                .map(|m| (m.role, m.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (stepyard_core::ChatRole::User, "q1"),
                (stepyard_core::ChatRole::Assistant, "a1"),
            ]
        );
    }

    #[test]
    fn multi_turn_shared_session_preserves_log_append_order_across_steps() {
        // Two `session: shared` chat steps run back-to-back; each
        // step commits its own turns on StepCompleted. The shared
        // bucket must hold all four turns in exact step-by-step,
        // turn-by-turn order — otherwise a re-rendered prompt in a
        // later step would see the assistant's reply before the
        // user's question.
        let events = vec![
            evt(1, chat_step_started("draft")),
            evt(2, chat_message_appended("draft", "shared", "user", "q1")),
            evt(
                3,
                chat_message_appended("draft", "shared", "assistant", "a1"),
            ),
            evt(4, chat_step_completed("draft")),
            evt(5, chat_step_started("review")),
            evt(6, chat_message_appended("review", "shared", "user", "q2")),
            evt(
                7,
                chat_message_appended("review", "shared", "assistant", "a2"),
            ),
            evt(8, chat_step_completed("review")),
        ];
        let progress = compute_progress(&events).unwrap();
        let bucket = progress.chat_sessions.get("shared").expect("shared bucket");
        assert_eq!(
            bucket
                .iter()
                .map(|m| (m.role, m.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (stepyard_core::ChatRole::User, "q1"),
                (stepyard_core::ChatRole::Assistant, "a1"),
                (stepyard_core::ChatRole::User, "q2"),
                (stepyard_core::ChatRole::Assistant, "a2"),
            ]
        );
    }

    #[test]
    fn distinct_sessions_are_isolated_into_separate_buckets() {
        // `session: isolated` puts each step's turns into its own
        // bucket keyed by step name. The scan must keep them apart —
        // merging would let an isolated chat see another step's
        // history on replay. Each step commits on its own
        // StepCompleted.
        let events = vec![
            evt(1, chat_step_started("alpha")),
            evt(2, chat_message_appended("alpha", "alpha", "user", "a_q")),
            evt(
                3,
                chat_message_appended("alpha", "alpha", "assistant", "a_a"),
            ),
            evt(4, chat_step_completed("alpha")),
            evt(5, chat_step_started("beta")),
            evt(6, chat_message_appended("beta", "beta", "user", "b_q")),
            evt(7, chat_step_completed("beta")),
        ];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.chat_sessions.len(), 2);
        assert_eq!(progress.chat_sessions.get("alpha").unwrap().len(), 2);
        assert_eq!(progress.chat_sessions.get("beta").unwrap().len(), 1);
    }

    #[test]
    fn chat_events_do_not_pollute_step_completion_counters() {
        // Verifies the "chat is not a step" axis stays crisp even
        // when chats are interleaved with non-chat completions. The
        // orphan chat event is staged (no matching chat StepCompleted)
        // and ends up discarded — the assertion here is specifically
        // about the completion counter, not chat_sessions population.
        let events = vec![
            evt(1, step_completed("setup", None)),
            evt(2, chat_message_appended("draft", "shared", "user", "hello")),
            evt(3, step_completed("teardown", None)),
        ];
        let progress = compute_progress(&events).unwrap();
        assert_eq!(progress.completed_steps, 2);
        // The orphan turn has no matching chat StepCompleted, so the
        // stage drops it — and `step_completed("setup"/"teardown")`
        // is `step_type: "agent"`, which never drains the chat stage.
        assert!(
            progress.chat_sessions.is_empty(),
            "agent completions must not drain the chat stage, got {:?}",
            progress.chat_sessions
        );
    }

    #[test]
    fn failed_chat_step_drops_staged_turns_from_history() {
        // If the chat step fails, its attempted turns are abandoned:
        // the runtime won't rerun a failed step, and committing the
        // partial history would hand the *next* chat step a prefix
        // the successful run never produced. Must stay empty.
        let events = vec![
            evt(1, chat_step_started("draft")),
            evt(2, chat_message_appended("draft", "shared", "user", "q1")),
            evt(3, chat_step_failed("draft")),
        ];
        let progress = compute_progress(&events).unwrap();
        assert!(progress.has_failure);
        assert_eq!(progress.last_failed_step.as_deref(), Some("draft"));
        assert!(
            progress.chat_sessions.is_empty(),
            "failed chat step must not pollute history, got {:?}",
            progress.chat_sessions
        );
    }

    #[test]
    fn rerun_after_crash_restages_only_the_committed_attempt() {
        // Simulates the atomicity scenario: run 1 emits a turn, then
        // crashes before StepCompleted. Run 2 (after resume) emits a
        // fresh StepStarted for the same step_name, its own turns,
        // then a StepCompleted. The replay must see ONLY run 2's
        // turns — the StepStarted reset drops the orphan run-1 turn.
        let events = vec![
            // Run 1 (crashed — no StepCompleted).
            evt(1, chat_step_started("draft")),
            evt(
                2,
                chat_message_appended("draft", "shared", "user", "q1-run1"),
            ),
            // Run 2 (resume — fresh StepStarted, full turns, commit).
            evt(3, chat_step_started("draft")),
            evt(
                4,
                chat_message_appended("draft", "shared", "user", "q1-run2"),
            ),
            evt(
                5,
                chat_message_appended("draft", "shared", "assistant", "a1-run2"),
            ),
            evt(6, chat_step_completed("draft")),
        ];
        let progress = compute_progress(&events).unwrap();
        let bucket = progress.chat_sessions.get("shared").expect("shared bucket");
        assert_eq!(
            bucket
                .iter()
                .map(|m| (m.role, m.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (stepyard_core::ChatRole::User, "q1-run2"),
                (stepyard_core::ChatRole::Assistant, "a1-run2"),
            ],
            "post-crash replay must not carry the orphan run-1 turn"
        );
    }

    #[test]
    fn chat_event_missing_step_name_fails_loudly() {
        // Replay contract gate: without `step_name` we can't key the
        // stage, so we'd either mis-attribute the turn to another
        // step or silently drop it. Both mis-commit history. Fail
        // the whole scan.
        let events = vec![evt(
            1,
            json!({
                "event": "chat_message_appended",
                "session": "shared",
                "role": "user",
                "content": "hello",
                "timestamp": "2026-04-22T12:00:00Z",
            }),
        )];
        let err = compute_progress(&events).expect_err("missing step_name must error");
        match err {
            EngineError::InvalidState(msg) => assert!(
                msg.contains("step_name"),
                "error must name the missing field, got: {msg}"
            ),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    #[test]
    fn chat_event_with_non_string_step_name_fails_loudly() {
        let events = vec![evt(
            1,
            json!({
                "event": "chat_message_appended",
                "step_name": 42,
                "session": "shared",
                "role": "user",
                "content": "hello",
                "timestamp": "2026-04-22T12:00:00Z",
            }),
        )];
        let err = compute_progress(&events).expect_err("numeric step_name must error");
        match err {
            EngineError::InvalidState(msg) => assert!(
                msg.contains("step_name"),
                "error must reference step_name, got: {msg}"
            ),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    #[test]
    fn chat_event_missing_session_fails_loudly() {
        // Replay contract gate: missing `session` can't be silently
        // defaulted — we wouldn't know which bucket to drain the
        // turn into on StepCompleted. Fail the whole scan so a
        // corrupt log cannot hand the next chat step a
        // silently-truncated history.
        let events = vec![evt(
            1,
            json!({
                "event": "chat_message_appended",
                "step_name": "draft",
                "role": "user",
                "content": "hello",
                "timestamp": "2026-04-22T12:00:00Z",
            }),
        )];
        let err = compute_progress(&events).expect_err("missing session must error");
        match err {
            EngineError::InvalidState(msg) => assert!(
                msg.contains("session"),
                "error must name the missing field, got: {msg}"
            ),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    #[test]
    fn chat_event_missing_content_fails_loudly() {
        let events = vec![evt(
            1,
            json!({
                "event": "chat_message_appended",
                "step_name": "draft",
                "session": "shared",
                "role": "user",
                "timestamp": "2026-04-22T12:00:00Z",
            }),
        )];
        let err = compute_progress(&events).expect_err("missing content must error");
        match err {
            EngineError::InvalidState(msg) => assert!(
                msg.contains("content"),
                "error must name the missing field, got: {msg}"
            ),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    #[test]
    fn chat_event_with_numeric_role_fails_loudly() {
        // The typed ChatRole enum is the replay gate for "unknown
        // role". A log row carrying `role: 42` (or `role: "system"`)
        // must fail the scan — silent coercion would change what
        // prompt the next turn renders.
        let events = vec![evt(
            1,
            json!({
                "event": "chat_message_appended",
                "step_name": "draft",
                "session": "shared",
                "role": 42,
                "content": "",
                "timestamp": "2026-04-22T12:00:00Z",
            }),
        )];
        let err = compute_progress(&events).expect_err("numeric role must error");
        match err {
            EngineError::InvalidState(msg) => assert!(
                msg.contains("role") || msg.contains("content"),
                "error must reference role/content parse, got: {msg}"
            ),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    #[test]
    fn chat_event_with_unknown_role_string_fails_loudly() {
        // `system`, `tool_use`, etc. are future-reserved but not in
        // the v1 wire format. Until a PR widens ChatRole, they must
        // reject at deserialize time so replay stays deterministic.
        let events = vec![evt(
            1,
            chat_message_appended("draft", "shared", "system", "you are ..."),
        )];
        let err = compute_progress(&events).expect_err("system role must error");
        match err {
            EngineError::InvalidState(msg) => assert!(
                msg.contains("role") || msg.contains("variant"),
                "error must reference role parse, got: {msg}"
            ),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }

    #[test]
    fn chat_event_with_non_string_session_fails_loudly() {
        let events = vec![evt(
            1,
            json!({
                "event": "chat_message_appended",
                "step_name": "draft",
                "session": 42,
                "role": "user",
                "content": "hello",
                "timestamp": "2026-04-22T12:00:00Z",
            }),
        )];
        let err = compute_progress(&events).expect_err("numeric session must error");
        match err {
            EngineError::InvalidState(msg) => assert!(
                msg.contains("session"),
                "error must reference session, got: {msg}"
            ),
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }
}

/// Round 3 Story 6 — compile-time `Send` checks for [`Engine`]'s public
/// async surface.
///
/// The harness drives engines via `tokio::spawn`, which requires the
/// returned future to be `Send`. The functions in this module are never
/// invoked at runtime; they exist so the compiler typechecks each
/// `Engine` async method's future against `F: Future + Send`. A
/// regression that captures a `!Send` value (e.g. `Rc`, `RefCell`,
/// non-`Send` field) across an await fails to compile here, blocking
/// the change before it reaches `tokio::spawn`.
///
/// Send-checks only — no `'static` bound, since these futures borrow
/// `&Engine`/`&mut Engine` for their duration and are spawned onto a
/// scoped task by the binary.
#[cfg(test)]
mod send_check {
    use super::*;

    fn assert_send_future<F: std::future::Future + Send>(_: F) {}

    #[allow(dead_code)]
    fn cancel_future_is_send(engine: &Engine) {
        assert_send_future(engine.cancel());
    }

    #[allow(dead_code)]
    fn step_future_is_send(engine: &mut Engine) {
        assert_send_future(engine.step());
    }

    #[allow(dead_code)]
    fn resume_future_is_send(engine: &mut Engine) {
        assert_send_future(engine.resume());
    }

    #[cfg(feature = "postgres")]
    #[allow(dead_code)]
    fn resume_existing_future_is_send(
        config: HarnessConfig,
        pool: &sqlx::PgPool,
        session_id: SessionId,
        workflow: Workflow,
        lifecycle: Arc<dyn SandboxLifecycle>,
    ) {
        assert_send_future(Engine::resume_existing(
            config, pool, session_id, workflow, lifecycle,
        ));
    }
}
