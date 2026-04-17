//! The [`Engine`] type — step/resume/cancel loop over a [`Session`] and a
//! [`SandboxLifecycle`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use chrono::Utc;
use minion_core::Event;
use minion_sandbox_orchestrator::SandboxLifecycle;
use minion_session::{Session, SessionError, SessionId, SessionStatus};
use tokio::sync::broadcast;

use crate::executor::{SandboxStepExecutor, StepExecutor};
use crate::workflow::Workflow;

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
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            tenant_id: "default".into(),
            shutdown_tx: default_shutdown_tx(),
            shutdown_grace_s: default_shutdown_grace_s(),
            shutdown_signal: default_shutdown_signal(),
        }
    }
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
/// `StepFailed` mirrors the shape of [`minion_core::EngineError::StepFailed`]
/// so timeout / cancel / signal termination reasons funnel through a single
/// taxonomy (D9). Callers can match on `reason` instead of listing sibling
/// variants.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("session error: {0}")]
    Session(#[from] SessionError),

    #[error("sandbox error: {0}")]
    Sandbox(#[from] minion_sandbox_orchestrator::SandboxError),

    #[error("invalid state: {0}")]
    InvalidState(String),

    #[error("step {step_index} failed: {reason}")]
    StepFailed {
        step_index: u32,
        reason: minion_core::TerminationReason,
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
        let executor = Arc::new(SandboxStepExecutor::new(lifecycle.clone()));
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
        }
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
                step_type: "cmd".into(),
                error: "Cancelled".into(),
                duration_ms: 0,
                timestamp: Utc::now(),
                sandboxed: true,
            })
            .await?;
            self.finalise_cancel().await?;
            return Ok(StepOutcome::Cancelled);
        }

        let step = &self.workflow.steps[progress.completed_steps].clone();
        let start = Instant::now();

        // Remember when the workflow actually started (first step).
        if progress.completed_steps == 0 && self.started_at.is_none() {
            self.started_at = Some(start);
            // Emit WorkflowStarted exactly once per session.
            self.emit(Event::WorkflowStarted {
                timestamp: Utc::now(),
            })
            .await?;
        }

        self.emit(Event::StepStarted {
            step_name: step.name.clone(),
            step_type: "cmd".into(),
            timestamp: Utc::now(),
        })
        .await?;

        // If cancel landed between StepStarted and exec, bail now — the
        // step is still in a recoverable place (no partial exec output
        // yet, so the retry path from `resume` after manual uncancel is
        // clean). In practice we treat it as the same as post-exec cancel.
        if self.cancel.is_cancelled() {
            self.emit(Event::StepFailed {
                step_name: step.name.clone(),
                step_type: "cmd".into(),
                error: "Cancelled".into(),
                duration_ms: start.elapsed().as_millis() as u64,
                timestamp: Utc::now(),
                sandboxed: true,
            })
            .await?;
            self.finalise_cancel().await?;
            return Ok(StepOutcome::Cancelled);
        }

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
        let step_index = progress.completed_steps as u32;
        // Snapshot the signal-name slot before the `tokio::select!` borrows
        // `self.shutdown_rx`. Cloning an `Arc` doesn't touch the OnceLock —
        // we just need a reader handle the select arm can read without
        // holding another reference to `self`.
        let signal_slot = self.config.shutdown_signal.clone();
        let selection = {
            let exec_fut = executor.execute(session_uuid, &step_clone);
            let cancel_fut = async {
                while !cancel_token.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            let timeout_fut = async {
                match step_clone.timeout {
                    Some(ms) => tokio::time::sleep(std::time::Duration::from_millis(ms)).await,
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

        // Signal landed first: emit SignalReceived BEFORE tearing the
        // sandbox down (D5 emit-before-IO), then finalise the session as
        // cancelled and return a typed StepFailed error carrying the
        // TerminationReason::SignalReceived taxonomy (D9). `finalise_cancel`
        // already does `lifecycle.destroy(&session_uuid)` tolerantly
        // followed by `session.cancel()` — reuse it rather than duplicate
        // the destroy call. Matches the emit-before-IO shape of the
        // StepTimeoutFired block directly below.
        if let StepSelection::Signal(ref signal) = selection {
            let signal = signal.clone();
            self.emit(Event::SignalReceived {
                signal: signal.clone(),
            })
            .await?;
            self.finalise_cancel().await?;
            return Err(EngineError::StepFailed {
                step_index,
                reason: minion_core::TerminationReason::SignalReceived(signal),
            });
        }

        // Timeout landed first. D5 emit-before-IO ordering: persist both
        // facts to the session log BEFORE tearing the sandbox down.
        //   1. StepTimeoutFired — the structural fact that the timer fired
        //      (step_index + configured_ms). Consumers can correlate against
        //      the configured timeout without replaying the whole log.
        //   2. StepFailed — the terminal fact that this step is done and the
        //      session is now in failure state. progress_from_log() treats
        //      step_failed as the terminal marker; without it a reloaded
        //      session can be advanced past a timed-out step. This matches
        //      architecture.md §D9 + NFR13.
        // Only after both events are appended do we destroy the sandbox
        // (IO) and flip the session row. Return a typed StepFailed error
        // carrying the TerminationReason::StepTimeout taxonomy (D9).
        if let StepSelection::TimedOut = selection {
            let configured_ms = step.timeout.expect("TimedOut requires step.timeout.is_some()");
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
            return Err(EngineError::StepFailed {
                step_index,
                reason: minion_core::TerminationReason::StepTimeout { configured_ms },
            });
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
                return Ok(StepOutcome::Cancelled);
            }
            StepSelection::TimedOut => unreachable!("handled above"),
            StepSelection::Signal(_) => unreachable!("handled above"),
        };

        match exec_result {
            Ok(output) if output.is_success() => {
                self.emit(Event::StepCompleted {
                    step_name: step.name.clone(),
                    step_type: "cmd".into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                    sandboxed: true,
                })
                .await?;
                Ok(StepOutcome::StepCompleted {
                    step_name: step.name.clone(),
                })
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
                Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                })
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
                Ok(StepOutcome::StepFailed {
                    step_name: step.name.clone(),
                    error,
                })
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

    // ── Internals ───────────────────────────────────────────────────────

    async fn emit(&self, event: Event) -> Result<(), EngineError> {
        let payload = serde_json::to_value(&event)
            .map_err(|e| EngineError::InvalidState(format!("serialize: {e}")))?;
        self.session.append(payload).await?;
        Ok(())
    }

    async fn progress_from_log(&self) -> Result<Progress, EngineError> {
        let events = self.session.replay().await?;
        let mut completed = 0usize;
        let mut has_failure = false;
        let mut last_failed: Option<String> = None;

        for evt in events.iter() {
            let Some(tag) = evt.payload.get("event").and_then(|v| v.as_str()) else {
                continue;
            };
            match tag {
                "step_completed" => completed += 1,
                "step_failed" => {
                    has_failure = true;
                    last_failed = evt
                        .payload
                        .get("step_name")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                }
                _ => {}
            }
        }

        Ok(Progress {
            completed_steps: completed,
            has_failure,
            last_failed_step: last_failed,
        })
    }

    async fn finalise_success(&mut self) -> Result<(), EngineError> {
        if self.session.status() == SessionStatus::Running {
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

    async fn finalise_cancel(&mut self) -> Result<(), EngineError> {
        if self.session.status() == SessionStatus::Running {
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

    async fn finalise_fail(&mut self) -> Result<(), EngineError> {
        if self.session.status() == SessionStatus::Running {
            self.session.fail().await?;
        }
        Ok(())
    }
}

struct Progress {
    completed_steps: usize,
    has_failure: bool,
    last_failed_step: Option<String>,
}

/// Which branch of the `step` loop's `tokio::select!` fired first.
enum StepSelection {
    /// The executor returned a result (either `Ok(output)` or `Err(e)`).
    Done(Result<minion_sandbox_orchestrator::ExecOutput, minion_sandbox_orchestrator::SandboxError>),
    /// The cancel token was flipped mid-step.
    Cancelled,
    /// The configured wall-clock timeout elapsed before the executor returned.
    TimedOut,
    /// The per-process shutdown broadcast fired (Story 2.3). Payload is the
    /// lowercase signal name read from `HarnessConfig::shutdown_signal` at
    /// the moment the select arm resolved.
    Signal(String),
}
