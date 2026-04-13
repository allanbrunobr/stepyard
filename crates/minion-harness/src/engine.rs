//! The [`Engine`] type — step/resume/cancel loop over a [`Session`] and a
//! [`SandboxLifecycle`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use minion_core::Event;
use minion_sandbox_orchestrator::SandboxLifecycle;
use minion_session::{Session, SessionError, SessionId, SessionStatus};

use crate::executor::{SandboxStepExecutor, StepExecutor};
use crate::workflow::Workflow;

/// Runtime configuration for the harness.
#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Tenant key used for new sessions (maps to `sessions.tenant_id`).
    pub tenant_id: String,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            tenant_id: "default".into(),
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
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("session error: {0}")]
    Session(#[from] SessionError),

    #[error("sandbox error: {0}")]
    Sandbox(#[from] minion_sandbox_orchestrator::SandboxError),

    #[error("invalid state: {0}")]
    InvalidState(String),
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
        Self {
            session,
            executor,
            lifecycle,
            workflow,
            cancel: CancelToken::default(),
            config,
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
        // Fast path: already cancelled — do not emit anything new.
        if self.cancel.is_cancelled() {
            self.finalise_cancel().await?;
            return Ok(StepOutcome::Cancelled);
        }

        // Ask the log how far we are. "Completed" counts only include
        // successful StepCompleted events; a StepFailed means the workflow
        // is stuck and no new step should be executed.
        let progress = self.progress_from_log().await?;
        if progress.has_failure {
            // Workflow is in failed state; do not advance.
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

        let exec_result = self.executor.execute(*self.session.id().as_uuid(), step).await;
        let duration_ms = start.elapsed().as_millis() as u64;

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
            // Tear down the sandbox — cattle, no regrets.
            let _ = self
                .lifecycle
                .destroy(&minion_sandbox_orchestrator::SandboxId::default())
                .await;
            self.session.cancel().await?;
        }
        Ok(())
    }
}

struct Progress {
    completed_steps: usize,
    has_failure: bool,
    last_failed_step: Option<String>,
}
