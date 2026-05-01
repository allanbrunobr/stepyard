//! Scope runner — executes `call` / `repeat` / `map` container steps.
//!
//! PR 3 of Task #31 (commit 3b). The v2 engine's top-level `step()`
//! dispatches to [`Engine::run_container_step`] in this module whenever
//! it meets a `call` / `repeat` / `map` kind. This module then walks the
//! named scope body for each iteration, emitting scoped
//! `StepStarted` / `StepCompleted` events with a populated
//! [`stepyard_core::ScopeContext`], and finally emits the **container's**
//! own top-level `StepCompleted` with a synthetic output snapshot so the
//! containing workflow advances by one step — preserving Invariante 11
//! (no per-step memory held on the engine; all state lives in the log).
//!
//! # Non-goals for PR 3
//!
//! * `map.parallel` is **carried through but ignored** — iterations run
//!   sequentially in declaration order. The field stays on the wire for
//!   YAML round-trip fidelity.
//! * `scope.outputs` / `step.outputs` override is honored for `call` and
//!   `repeat`; for `map` the synthetic output is always the per-item
//!   stdout JSON array. Custom reducers/collect live in a later PR.
//! * Nested containers (a `call` / `repeat` / `map` inside a scope body)
//!   are rejected: the containing top-level step emits `StepFailed` and
//!   no scoped events are logged for the inner step (defence-in-depth —
//!   the adapter already blocks this at the CLI boundary).
//!
//! # Replay model
//!
//! [`Engine::run_container_step`] never holds a mutable iteration or
//! position counter between `step()` calls. On every entry it re-scans
//! the session log, finds the last scoped event that belongs to this
//! container, and decides whether to resume inside an iteration, start
//! the next iteration, or emit the container's terminal top-level
//! `StepCompleted`. A process crash anywhere inside a scope body is
//! therefore fully recoverable through [`Engine::resume`].
//!
//! `steps.*` rendering inside an iteration body is **locally scoped** —
//! only scoped completions with `container == self && iteration == N &&
//! position < P` contribute to the outputs map visible to the step at
//! position `P`. Top-level completions are exposed via `target` and
//! `vars`; iteration carry lives on `scope.value` (v1 parity).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use chrono::Utc;
use stepyard_core::{Event, ScopeContext, Signal, StepOutputSnapshot};
use stepyard_sandbox_orchestrator::ExecOutput;
use tokio::task::JoinSet;

use crate::engine::{CmdOutcome, Engine, EngineError, StepOutcome};
use crate::gate::{evaluate_bool, outcome_for, GateAction, GateError, GateOutcome};
use crate::render::{render, RenderContext, ScopeView};
use crate::workflow::{Step, StepKind};

/// Shared label function for scope-body step kinds. Keeps the "cmd" /
/// "gate" strings a single source of truth so the log stays consistent
/// with the top-level dispatch.
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

async fn parallel_task_with_timeout<F>(
    timeout: Option<std::time::Duration>,
    fut: F,
) -> ParallelTaskResult
where
    F: Future<Output = Result<ParallelTaskOutput, String>>,
{
    match timeout {
        Some(timeout) => match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(out)) => ParallelTaskResult::Completed(out),
            Ok(Err(error)) => ParallelTaskResult::Failed(error),
            Err(_) => ParallelTaskResult::TimedOut {
                configured_ms: timeout.as_millis() as u64,
            },
        },
        None => match fut.await {
            Ok(out) => ParallelTaskResult::Completed(out),
            Err(error) => ParallelTaskResult::Failed(error),
        },
    }
}

/// One scope-body step's contribution to the iteration's control flow.
///
/// Internal to [`Engine::run_scope_body`]. Every variant except
/// [`BodyStep::Continue`] means the iteration stops — either because
/// the gate said so (`Skip`/`Break`), the step failed, or the session
/// was cancelled / signalled / timed out. For the terminal variants
/// (everything below `Break`) the terminal events and session
/// finalisation have **already** been persisted; the caller just
/// surfaces the corresponding outcome.
#[derive(Debug)]
enum BodyStep {
    /// Step completed normally. Snapshot is `None` for gate steps
    /// (no exec output) and `Some(...)` for cmd steps.
    Continue(Option<StepOutputSnapshot>),
    /// A gate inside the body fired `skip` — end this iteration, start
    /// the next one (or, for `call`, complete the scope body).
    Skip,
    /// A gate inside the body fired `break` — end the whole container.
    Break,
    /// Body step returned a non-zero exit, parse/render/gate error, or
    /// the sandbox layer itself errored. `StepFailed` + `finalise_fail`
    /// already emitted.
    Failed(String),
    /// Cancel token flipped. `StepFailed("Cancelled")` + `finalise_cancel`
    /// already emitted.
    Cancelled,
    /// Process-wide shutdown broadcast fired. `SignalReceived` +
    /// `StepFailed` + `finalise_cancel` already emitted. String is the
    /// lowercase signal name.
    Signal(String),
    /// Per-step wall-clock timeout. `StepTimeoutFired` + `StepFailed` +
    /// sandbox destroy + `finalise_fail` already emitted.
    TimedOut { configured_ms: u64 },
}

enum ParallelTask {
    Cmd {
        pos: u32,
        name: String,
        step: Step,
        env: HashMap<String, String>,
    },
    Agent {
        pos: u32,
        name: String,
        step: Step,
        prompt: String,
        env: HashMap<String, String>,
        agent_session_ids: HashMap<String, String>,
        first_agent_session_id: Option<String>,
    },
    Chat {
        pos: u32,
        name: String,
        step: Step,
        req: crate::chat_exec::ChatCompletionRequest,
        client: Arc<dyn crate::chat_exec::ChatClient>,
        session: String,
        user_content: String,
    },
}

enum ParallelTaskOutput {
    Cmd(ExecOutput),
    Agent(crate::agent_exec::AgentExecOutput),
    Chat {
        session: String,
        user_content: String,
        output: crate::chat_exec::ChatExecOutput,
    },
}

enum ParallelTaskResult {
    Completed(ParallelTaskOutput),
    Failed(String),
    TimedOut { configured_ms: u64 },
}

enum ParallelDrain {
    Completed,
    Failed(String),
    TimedOut { name: String, configured_ms: u64 },
    Cancelled,
    Signal(String),
}

/// Result of running one full iteration of a scope body.
#[derive(Debug)]
enum IterationOutcome {
    /// Iteration completed — body walked to the last step or a gate
    /// fired `skip`. `last_output` is the most recent cmd snapshot the
    /// body produced (used by `call` / `repeat` as fallback when no
    /// `outputs:` override is set).
    Completed {
        last_output: Option<StepOutputSnapshot>,
    },
    /// A gate fired `break` — the containing loop must exit. Carries
    /// the last-output-so-far for the synthetic container stdout.
    Break {
        last_output: Option<StepOutputSnapshot>,
    },
    /// Body step failed / cancelled / signalled / timed out. Terminal
    /// events + finalisation already emitted; surfaces to the caller
    /// so it can propagate the right outcome without emitting anything
    /// else.
    Failed(String),
    Cancelled,
    Signal(String),
    TimedOut {
        configured_ms: u64,
    },
}

/// State reconstructed from the session log for a single container
/// step. Enables log-only replay: each `run_container_step` call rescans
/// the log and arrives at the same `ContainerReplayState` pre-crash or
/// post-crash.
#[derive(Debug, Default)]
struct ContainerReplayState {
    /// Zero-based index of the iteration to run next. When a prior
    /// iteration has `completed_body == true`, this is that
    /// iteration's index + 1.
    next_iteration: u32,
    /// Furthest (iteration, position) pair with a logged
    /// `step_completed`. Used inside [`Engine::run_scope_body`] to skip
    /// positions that already ran to completion pre-crash.
    last_completed: Option<(u32, u32)>,
    /// `true` when a scoped gate with `gate_outcome == "break"` has
    /// been logged for this container — the loop must not start a
    /// further iteration, only emit the container's top-level
    /// `StepCompleted`.
    broke: bool,
    /// Per-iteration final cmd snapshot (keyed by iteration index).
    /// Maps are small — bounded by the `repeat` count or `map` item
    /// list — so an unbounded `HashMap` is acceptable here.
    last_output_per_iteration: HashMap<u32, StepOutputSnapshot>,
    /// Iterations whose scoped gate emitted `gate_outcome == "skip"`.
    /// These iterations are terminal even though later positions have
    /// no `step_completed` entry — a crash immediately after a skip
    /// event must not resume past the gate.
    ///
    /// Break is intentionally NOT tracked here: non-replay leaves
    /// `state.next_iteration == break_iter` (see `run_repeat` /
    /// `run_map`), so the rebuild must mirror that (capping via
    /// `break_iteration` during the scan) rather than treating the
    /// break iteration as "advance past".
    iteration_terminated_by_gate: HashSet<u32>,
}

impl ContainerReplayState {
    /// Rebuild from the session log by scanning scoped events whose
    /// `scope_context.container == container_name`. `scope_len` is the
    /// number of body steps — used to decide whether an iteration has
    /// fully completed (all positions saw a `step_completed`).
    async fn rebuild(
        engine: &Engine,
        container_name: &str,
        scope_len: u32,
    ) -> Result<Self, EngineError> {
        let events = engine
            .session_handle()
            .replay()
            .await
            .map_err(EngineError::from)?;

        let mut state = ContainerReplayState::default();

        // Track which positions of each iteration have a logged
        // step_completed. A small Vec<bool> per iteration is enough
        // because iterations are bounded by `max_iterations` / item
        // count, and scope bodies are short.
        let mut iteration_positions: HashMap<u32, Vec<bool>> = HashMap::new();

        // Lowest iteration index at which a scoped gate logged
        // `gate_outcome == "break"`. Non-replay behavior parks
        // `state.next_iteration` at that iteration (it does NOT
        // advance), so the rebuild caps next_iteration there.
        let mut break_iteration: Option<u32> = None;

        for evt in events.iter() {
            let Some(tag) = evt.payload.get("event").and_then(|v| v.as_str()) else {
                continue;
            };
            if tag != "step_completed" {
                continue;
            }
            let Some(sc) = evt.payload.get("scope_context") else {
                continue;
            };
            if sc.is_null() {
                continue;
            }
            let Some(container) = sc.get("container").and_then(|v| v.as_str()) else {
                continue;
            };
            if container != container_name {
                continue;
            }
            let Some(iteration) = sc.get("iteration").and_then(|v| v.as_u64()) else {
                continue;
            };
            let Some(position) = sc.get("position").and_then(|v| v.as_u64()) else {
                continue;
            };
            let iteration = iteration as u32;
            let position = position as u32;

            let seen = iteration_positions
                .entry(iteration)
                .or_insert_with(|| vec![false; scope_len as usize]);
            if let Some(slot) = seen.get_mut(position as usize) {
                *slot = true;
            }

            // Track last-output for the iteration — any cmd completion
            // contributes; later positions overwrite earlier ones so
            // the final snapshot lands in the map. Malformed payload
            // must fail loudly (mirrors PR 2 gate_replay hardening).
            if let Some(raw) = evt.payload.get("output").filter(|v| !v.is_null()) {
                let snap: StepOutputSnapshot =
                    serde_json::from_value(raw.clone()).map_err(|e| {
                        EngineError::InvalidState(format!(
                            "scope replay: step_completed log entry has malformed `output` payload (container=`{container_name}`, iteration={iteration}, position={position}): {e}"
                        ))
                    })?;
                state.last_output_per_iteration.insert(iteration, snap);
            }

            // Record furthest (iteration, position) pair.
            state.last_completed = Some(match state.last_completed {
                None => (iteration, position),
                Some((i, p)) if (iteration, position) > (i, p) => (iteration, position),
                Some(prev) => prev,
            });

            // Track terminal scoped gate outcomes. Skip marks the
            // iteration terminal so next_iteration advances past it.
            // Break leaves next_iteration at the break iter — matching
            // non-replay, which also does not advance `state.next_iteration`
            // on break.
            if let Some(go) = evt.payload.get("gate_outcome").and_then(|v| v.as_str()) {
                match go {
                    "skip" => {
                        state.iteration_terminated_by_gate.insert(iteration);
                    }
                    "break" => {
                        state.broke = true;
                        break_iteration = Some(
                            break_iteration
                                .map(|b| b.min(iteration))
                                .unwrap_or(iteration),
                        );
                    }
                    _ => {}
                }
            }
        }

        // Figure out next_iteration: walk the iterations we've seen in
        // order. An iteration is "done" (advance past) when either all
        // positions are logged OR a scoped gate ended it with `skip`.
        // If a break has been logged, next_iteration caps at that
        // iteration index regardless of what the position vectors say.
        let mut next_iteration = 0u32;
        let mut sorted_iters: Vec<u32> = iteration_positions.keys().copied().collect();
        sorted_iters.sort_unstable();
        for iter in sorted_iters {
            if break_iteration == Some(iter) {
                next_iteration = iter;
                break;
            }
            let Some(seen) = iteration_positions.get(&iter) else {
                continue;
            };
            let fully_seen = seen.iter().all(|x| *x);
            let skip_terminal = state.iteration_terminated_by_gate.contains(&iter);
            if fully_seen || skip_terminal {
                next_iteration = iter + 1;
            } else {
                next_iteration = iter;
                break;
            }
        }
        if let Some(bi) = break_iteration {
            // Guard against a configuration where break was logged but
            // no position row exists for that iteration (defensive).
            next_iteration = next_iteration.min(bi);
        }
        state.next_iteration = next_iteration;

        Ok(state)
    }
}

/// Snapshot map visible to templates **inside** a single scope body
/// iteration. Rebuilt on every render call (no caching) to keep
/// Invariante 11: scope-body state lives in the session log, not on
/// the engine.
///
/// Per PR 3 of Task #31 (user-anchored rule 2): scope-body templates
/// see only prior-position scoped completions of the *same* iteration.
/// Top-level completions are exposed via `target` / `vars`; iteration
/// carry lives on `scope.value`.
async fn in_iteration_outputs(
    engine: &Engine,
    container_name: &str,
    iteration: u32,
    before_position: u32,
) -> Result<HashMap<String, StepOutputSnapshot>, EngineError> {
    let events = engine
        .session_handle()
        .replay()
        .await
        .map_err(EngineError::from)?;

    let mut outputs: HashMap<String, StepOutputSnapshot> = HashMap::new();

    for evt in events.iter() {
        let Some(tag) = evt.payload.get("event").and_then(|v| v.as_str()) else {
            continue;
        };
        if tag != "step_completed" {
            continue;
        }
        let Some(sc) = evt.payload.get("scope_context").filter(|v| !v.is_null()) else {
            continue;
        };
        let Some(container) = sc.get("container").and_then(|v| v.as_str()) else {
            continue;
        };
        if container != container_name {
            continue;
        }
        let Some(iter) = sc.get("iteration").and_then(|v| v.as_u64()) else {
            continue;
        };
        if iter as u32 != iteration {
            continue;
        }
        let Some(pos) = sc.get("position").and_then(|v| v.as_u64()) else {
            continue;
        };
        if pos as u32 >= before_position {
            continue;
        }

        let step_name = match evt.payload.get("step_name").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if let Some(raw) = evt.payload.get("output").filter(|v| !v.is_null()) {
            let snap: StepOutputSnapshot =
                serde_json::from_value(raw.clone()).map_err(|e| {
                    EngineError::InvalidState(format!(
                        "scope replay: step_completed log entry has malformed `output` payload (container=`{container_name}`, iteration={iteration}, step=`{step_name}`): {e}"
                    ))
                })?;
            outputs.insert(step_name, snap);
        }
    }

    Ok(outputs)
}

/// v1-parity parser for a rendered `items:` string.
///
/// Mirrors the legacy engine (`src/steps/map.rs:237`): a string starting
/// with `[` is tried as a JSON array (strings pass through, other JSON
/// values render via `to_string`); anything else — including invalid
/// JSON that starts with `[` — falls back to line-splitting, skipping
/// blank lines. The absence of a failure path is intentional: v1 never
/// rejects item shape at parse time (the user-anchored rule 3 for 3b
/// keeps that behavior).
pub(crate) fn parse_items(rendered: &str) -> Vec<String> {
    if rendered.trim_start().starts_with('[') {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(rendered) {
            return arr
                .into_iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                })
                .collect();
        }
    }
    rendered
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

impl Engine {
    /// Top-level entry point for a `call` / `repeat` / `map` step.
    ///
    /// Called from [`Engine::step`] AFTER the container's top-level
    /// `StepStarted` has been emitted. Returns the same
    /// [`StepOutcome`] shape the cmd/gate paths return, so the caller
    /// in `step()` doesn't need to know whether it dispatched to a
    /// scope runner or a cmd path.
    pub(crate) async fn run_container_step(
        &mut self,
        step: &Step,
        start: std::time::Instant,
    ) -> Result<StepOutcome, EngineError> {
        match &step.kind {
            StepKind::Call => self.run_call(step, start).await,
            StepKind::Repeat => self.run_repeat(step, start).await,
            StepKind::Map => self.run_map(step, start).await,
            StepKind::Parallel => self.run_parallel(step, start).await,
            // Unreachable: `step()` dispatches by kind; this matcher
            // exists only so a future kind landing without wiring
            // produces a clean compile error.
            other => Err(EngineError::InvalidState(format!(
                "run_container_step received non-container kind `{other}`"
            ))),
        }
    }

    /// Execute a `parallel` — run every sub-step in `scope` concurrently and
    /// surface the first failure as the container failure (v1 parity:
    /// `src/steps/parallel.rs`).
    ///
    /// PR 6 of Task #31. The adapter synthesises a hidden scope
    /// (`__parallel_<top_level_index>`) from the YAML `steps:` list, so
    /// the harness sees parallel as just another scope-bodied container
    /// — but the runner is bespoke (sub-steps run concurrently, not
    /// sequentially) so it does not share `run_scope_body`.
    ///
    /// Replay model: sub-step completions can land in any order, so we
    /// can't reuse `ContainerReplayState` (which assumes sequential
    /// `0..=p` completion). Instead we walk the log directly via
    /// [`parallel_completed_positions`] and skip every position whose
    /// `step_completed` is already persisted.
    ///
    /// Output synthesis is **position-specific**: the synthetic stdout
    /// is the LAST sub-step by definition order (position `scope_len-1`),
    /// not the chronologically last to finish. v1 parity:
    /// `src/steps/parallel.rs` reads `nested_steps.last()` against the
    /// outputs map keyed by definition order.
    async fn run_parallel(
        &mut self,
        step: &Step,
        start: std::time::Instant,
    ) -> Result<StepOutcome, EngineError> {
        let container_name = step.name.clone();

        let scope_name = match step.scope.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return self
                    .emit_container_failure(
                        step,
                        start,
                        "parallel step missing `scope:` field".into(),
                    )
                    .await;
            }
        };
        let scope_def = match self.workflow().scopes.get(&scope_name).cloned() {
            Some(s) => s,
            None => {
                return self
                    .emit_container_failure(
                        step,
                        start,
                        format!(
                            "scope `{scope_name}` referenced by parallel step `{container_name}` not found in workflow"
                        ),
                    )
                    .await;
            }
        };
        let scope_len = scope_def.steps.len() as u32;

        if scope_len == 0 {
            // Empty parallel: synthesise an empty stdout and succeed.
            // Mirrors v1's vacuous-Ok behavior (`nested_steps.last()` is
            // None → fallback to `StepOutput::Empty`).
            let synthetic = self
                .synthesise_container_output(step, &scope_def.outputs, &container_name, None, 0)
                .await;
            return self.emit_container_success(step, start, synthetic).await;
        }

        let container_top_level_index = top_level_position_of(self, &container_name).await?;

        // Container-level cancel check before spawning.
        if self.is_cancelled() {
            return self.emit_container_cancel(step, start).await;
        }

        // Replay-skip set: positions whose `step_completed` for this
        // container at iteration 0 is already in the log. Critical that
        // we walk the log directly instead of using
        // `ContainerReplayState` — parallel sub-steps complete in
        // arbitrary order, so the latter's "last_completed = (iter, p)
        // means 0..=p complete" invariant doesn't hold here.
        let already_completed = parallel_completed_positions(self, &container_name).await?;

        // Render/resolve every non-completed sub-step serially — this work
        // is cheap and avoids borrowing `&self` inside spawned futures.
        let top_outs = top_level_outputs(self).await?;
        let progress = self.progress_from_log().await?;
        let mut to_run: Vec<ParallelTask> = Vec::new();
        for (pos, sub) in scope_def.steps.iter().enumerate() {
            let pos = pos as u32;
            if already_completed.contains(&pos) {
                continue;
            }
            // Defence-in-depth: #80 wires cmd/agent/chat into parallel.
            // Other non-container kinds stay rejected until their
            // parallel semantics are explicitly implemented.
            if !matches!(sub.kind, StepKind::Cmd | StepKind::Agent | StepKind::Chat) {
                return self
                    .emit_container_failure(
                        step,
                        start,
                        format!(
                            "parallel sub-step `{}` has unsupported kind `{}` (allowed: cmd, agent, chat)",
                            sub.name, sub.kind
                        ),
                    )
                    .await;
            }
            let resolved_env = match self.prepare_step(sub) {
                Ok(e) => e,
                Err(e) => {
                    return self
                        .emit_container_failure(
                            step,
                            start,
                            format!("env resolve for parallel sub-step `{}`: {e}", sub.name),
                        )
                        .await;
                }
            };
            let ctx = RenderContext {
                steps: &top_outs,
                target: &self.run_context().target,
                vars: &self.run_context().vars,
                scope: None,
            };
            match sub.kind {
                StepKind::Cmd => {
                    let rendered = match render(&sub.command, &ctx) {
                        Ok(s) => s,
                        Err(e) => {
                            return self
                                .emit_container_failure(
                                    step,
                                    start,
                                    format!("render parallel sub-step `{}`: {e}", sub.name),
                                )
                                .await;
                        }
                    };
                    let mut rendered_step = sub.clone();
                    rendered_step.command = rendered;
                    to_run.push(ParallelTask::Cmd {
                        pos,
                        name: sub.name.clone(),
                        step: rendered_step,
                        env: resolved_env,
                    });
                }
                StepKind::Agent => {
                    let Some(prompt_template) = sub.prompt.as_deref() else {
                        return self
                            .emit_container_failure(
                                step,
                                start,
                                format!(
                                    "agent step `{}` has no prompt — the adapter should have rejected this at load time",
                                    sub.name
                                ),
                            )
                            .await;
                    };
                    let prompt = match render(prompt_template, &ctx) {
                        Ok(s) => s,
                        Err(e) => {
                            return self
                                .emit_container_failure(
                                    step,
                                    start,
                                    format!("agent prompt render failed: {e}"),
                                )
                                .await;
                        }
                    };
                    to_run.push(ParallelTask::Agent {
                        pos,
                        name: sub.name.clone(),
                        step: sub.clone(),
                        prompt,
                        env: resolved_env,
                        agent_session_ids: progress.agent_session_ids.clone(),
                        first_agent_session_id: progress.first_agent_session_id.clone(),
                    });
                }
                StepKind::Chat => {
                    let Some(prompt_template) = sub.prompt.as_deref() else {
                        return self
                            .emit_container_failure(
                                step,
                                start,
                                format!(
                                    "chat step `{}` has no prompt — the adapter should have rejected this at load time",
                                    sub.name
                                ),
                            )
                            .await;
                    };
                    let prompt = match render(prompt_template, &ctx) {
                        Ok(s) => s,
                        Err(e) => {
                            return self
                                .emit_container_failure(
                                    step,
                                    start,
                                    format!("chat prompt render failed: {e}"),
                                )
                                .await;
                        }
                    };
                    let session = sub.chat_session.clone().unwrap_or_else(|| sub.name.clone());
                    let history = progress
                        .chat_sessions
                        .get(&session)
                        .cloned()
                        .unwrap_or_default();
                    let resolved_api_key = match crate::chat_exec::resolve_api_key(
                        &sub.name,
                        sub.api_key_env.as_deref(),
                        &resolved_env,
                    ) {
                        Ok(v) => v,
                        Err(e) => {
                            return self
                                .emit_container_failure(
                                    step,
                                    start,
                                    format!(
                                        "resolve chat api key for parallel sub-step `{}`: {e}",
                                        sub.name
                                    ),
                                )
                                .await;
                        }
                    };
                    let Some(client) = self.chat_client() else {
                        return self
                            .emit_container_failure(
                                step,
                                start,
                                crate::chat_exec::ChatExecError::NoClientConfigured {
                                    step: sub.name.clone(),
                                }
                                .to_string(),
                            )
                            .await;
                    };
                    let req = crate::chat_exec::build_chat_request(
                        sub,
                        prompt.clone(),
                        history,
                        resolved_api_key,
                    );
                    to_run.push(ParallelTask::Chat {
                        pos,
                        name: sub.name.clone(),
                        step: sub.clone(),
                        req,
                        client,
                        session,
                        user_content: prompt,
                    });
                }
                _ => unreachable!("unsupported kind handled above"),
            }
        }

        // Spawn one task per non-completed sub-step. Cloned executor/client
        // handles let each task own its own `Arc` so the futures don't
        // borrow `self`. Each task owns its own step timeout; the drain
        // loop below races the whole JoinSet against cancel and shutdown.
        let session_uuid = *self.session().id().as_uuid();
        let mut joinset: JoinSet<(u32, String, StepKind, ParallelTaskResult)> = JoinSet::new();
        for task in to_run {
            match task {
                ParallelTask::Cmd {
                    pos,
                    name,
                    step,
                    env,
                } => {
                    let executor = self.executor();
                    let timeout = step.timeout;
                    joinset.spawn(async move {
                        let fut = async {
                            executor
                                .execute_with_env(session_uuid, &step, &env)
                                .await
                                .map(ParallelTaskOutput::Cmd)
                                .map_err(|e| e.to_string())
                        };
                        let res = parallel_task_with_timeout(timeout, fut).await;
                        (pos, name, StepKind::Cmd, res)
                    });
                }
                ParallelTask::Agent {
                    pos,
                    name,
                    step,
                    prompt,
                    env,
                    agent_session_ids,
                    first_agent_session_id,
                } => {
                    let timeout = step.timeout;
                    joinset.spawn(async move {
                        let state = crate::agent_exec::AgentSessionState {
                            agent_session_ids: &agent_session_ids,
                            first_agent_session_id: first_agent_session_id.as_deref(),
                        };
                        let fut = async {
                            crate::agent_exec::run_agent_step(&step, &prompt, &state, &env)
                                .await
                                .map(ParallelTaskOutput::Agent)
                                .map_err(|e| e.to_string())
                        };
                        let res = parallel_task_with_timeout(timeout, fut).await;
                        (pos, name, StepKind::Agent, res)
                    });
                }
                ParallelTask::Chat {
                    pos,
                    name,
                    step,
                    req,
                    client,
                    session,
                    user_content,
                } => {
                    let timeout = step.timeout;
                    joinset.spawn(async move {
                        let fut = async {
                            crate::chat_exec::run_chat_step(&step, req, client.as_ref())
                                .await
                                .map(|output| ParallelTaskOutput::Chat {
                                    session,
                                    user_content,
                                    output,
                                })
                                .map_err(|e| e.to_string())
                        };
                        let res = parallel_task_with_timeout(timeout, fut).await;
                        (pos, name, StepKind::Chat, res)
                    });
                }
            }
        }

        // Drain. First non-zero exit / executor error / per-task timeout
        // wins; cancel and shutdown race the entire JoinSet. Subsequent
        // tasks are aborted but we still drain the JoinSet so child futures
        // are dropped and RAII cleanup can run.
        let mut completions: Vec<(u32, String, StepKind, ParallelTaskOutput)> = Vec::new();
        let mut terminal = ParallelDrain::Completed;
        while !joinset.is_empty() {
            let cancel_token = self.cancel_token();
            let cancel_fut = async {
                while !cancel_token.is_cancelled() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            };
            let signal_slot = self.shutdown_signal();
            let shutdown_rx = self.shutdown_rx_mut();
            let shutdown_fut = shutdown_rx.recv();
            tokio::pin!(cancel_fut);
            tokio::pin!(shutdown_fut);

            let joined = tokio::select! {
                joined = joinset.join_next() => joined,
                _ = &mut cancel_fut => {
                    terminal = ParallelDrain::Cancelled;
                    joinset.abort_all();
                    break;
                }
                _ = &mut shutdown_fut => {
                    terminal = ParallelDrain::Signal(
                        signal_slot
                            .get()
                            .cloned()
                            .unwrap_or_else(|| "unknown".into()),
                    );
                    joinset.abort_all();
                    break;
                }
            };

            let Some(joined) = joined else {
                break;
            };
            match joined {
                Ok((pos, name, kind, ParallelTaskResult::Completed(out))) => {
                    if let ParallelTaskOutput::Cmd(ref cmd_out) = out {
                        if cmd_out.exit_code != 0 {
                            terminal = ParallelDrain::Failed(format!(
                                "parallel sub-step `{name}` failed with exit_code={}",
                                cmd_out.exit_code
                            ));
                            joinset.abort_all();
                        }
                    }
                    completions.push((pos, name, kind, out));
                }
                Ok((_pos, name, _kind, ParallelTaskResult::Failed(error))) => {
                    terminal =
                        ParallelDrain::Failed(format!("parallel sub-step `{name}`: {error}"));
                    joinset.abort_all();
                }
                Ok((_pos, name, _kind, ParallelTaskResult::TimedOut { configured_ms })) => {
                    terminal = ParallelDrain::TimedOut {
                        name,
                        configured_ms,
                    };
                    joinset.abort_all();
                }
                Err(je) if je.is_cancelled() => {
                    // Aborted via `joinset.abort_all()` after another
                    // terminal branch won — expected, drop silently.
                }
                Err(je) => {
                    terminal = ParallelDrain::Failed(format!("parallel sub-step JoinError: {je}"));
                    joinset.abort_all();
                }
            }
            if !matches!(terminal, ParallelDrain::Completed) {
                break;
            }
        }

        if !matches!(terminal, ParallelDrain::Completed) {
            while let Some(joined) = joinset.join_next().await {
                if let Ok((pos, name, kind, ParallelTaskResult::Completed(out))) = joined {
                    completions.push((pos, name, kind, out));
                }
            }
        }

        // Emit scoped events for every completion in **definition** order
        // (position-sorted). v1 parity: a downstream replay walking
        // `step_completed` events sees deterministic ordering.
        completions.sort_by_key(|(pos, _, _, _)| *pos);
        let mut completion_snaps: Vec<(u32, StepOutputSnapshot)> = Vec::new();
        for (pos, name, kind, out) in &completions {
            let scope_ctx = ScopeContext {
                container: container_name.clone(),
                iteration: 0,
                position: *pos,
            };
            self.emit(Event::StepStarted {
                step_name: name.clone(),
                step_type: step_type_label(kind).into(),
                timestamp: Utc::now(),
                scope_context: Some(scope_ctx.clone()),
            })
            .await?;

            match out {
                ParallelTaskOutput::Cmd(cmd_out) => {
                    let snap = StepOutputSnapshot {
                        stdout: cmd_out.stdout.clone(),
                        stderr: cmd_out.stderr.clone(),
                        exit_code: cmd_out.exit_code,
                    };
                    if cmd_out.exit_code == 0 {
                        completion_snaps.push((*pos, snap.clone()));
                        self.emit(Event::StepCompleted {
                            step_name: name.clone(),
                            step_type: "cmd".into(),
                            duration_ms: 0,
                            timestamp: Utc::now(),
                            input_tokens: None,
                            output_tokens: None,
                            cost_usd: None,
                            sandboxed: true,
                            output: Some(snap),
                            scope_context: Some(scope_ctx),
                            gate_outcome: None,
                            agent_session_id: None,
                        })
                        .await?;
                    } else {
                        self.emit(Event::StepFailed {
                            step_name: name.clone(),
                            step_type: "cmd".into(),
                            error: format!("exit_code={}", cmd_out.exit_code),
                            duration_ms: 0,
                            timestamp: Utc::now(),
                            sandboxed: true,
                        })
                        .await?;
                    }
                }
                ParallelTaskOutput::Agent(agent_out) => {
                    let snap = StepOutputSnapshot {
                        stdout: agent_out.response.clone(),
                        stderr: String::new(),
                        exit_code: 0,
                    };
                    completion_snaps.push((*pos, snap.clone()));
                    self.emit(Event::StepCompleted {
                        step_name: name.clone(),
                        step_type: "agent".into(),
                        duration_ms: 0,
                        timestamp: Utc::now(),
                        input_tokens: agent_out.input_tokens,
                        output_tokens: agent_out.output_tokens,
                        cost_usd: agent_out.cost_usd,
                        sandboxed: false,
                        output: Some(snap),
                        scope_context: Some(scope_ctx),
                        gate_outcome: None,
                        agent_session_id: agent_out.session_id.clone(),
                    })
                    .await?;
                }
                ParallelTaskOutput::Chat {
                    session,
                    user_content,
                    output,
                } => {
                    self.emit(Event::ChatMessageAppended {
                        step_name: name.clone(),
                        session: session.clone(),
                        role: stepyard_core::ChatRole::User,
                        content: user_content.clone(),
                        timestamp: Utc::now(),
                    })
                    .await?;
                    self.emit(Event::ChatMessageAppended {
                        step_name: name.clone(),
                        session: session.clone(),
                        role: stepyard_core::ChatRole::Assistant,
                        content: output.response.clone(),
                        timestamp: Utc::now(),
                    })
                    .await?;
                    let snap = StepOutputSnapshot {
                        stdout: output.response.clone(),
                        stderr: String::new(),
                        exit_code: 0,
                    };
                    completion_snaps.push((*pos, snap.clone()));
                    self.emit(Event::StepCompleted {
                        step_name: name.clone(),
                        step_type: "chat".into(),
                        duration_ms: 0,
                        timestamp: Utc::now(),
                        input_tokens: output.input_tokens,
                        output_tokens: output.output_tokens,
                        cost_usd: output.cost_usd,
                        sandboxed: false,
                        output: Some(snap),
                        scope_context: Some(scope_ctx),
                        gate_outcome: None,
                        agent_session_id: None,
                    })
                    .await?;
                }
            }
        }

        match terminal {
            ParallelDrain::Completed => {}
            ParallelDrain::Failed(err) => {
                return self.emit_container_failure(step, start, err).await;
            }
            ParallelDrain::Cancelled => {
                return self.emit_container_cancel(step, start).await;
            }
            ParallelDrain::Signal(signal) => {
                self.emit_container_signal(step, start, signal.clone())
                    .await?;
                return Err(EngineError::StepFailed {
                    step_index: container_top_level_index,
                    reason: stepyard_core::TerminationReason::SignalReceived(
                        stepyard_core::Signal::from(signal),
                    ),
                });
            }
            ParallelDrain::TimedOut {
                name,
                configured_ms,
            } => {
                self.emit_container_timeout(step, start, &name, configured_ms)
                    .await?;
                return Err(EngineError::StepFailed {
                    step_index: container_top_level_index,
                    reason: stepyard_core::TerminationReason::StepTimeout { configured_ms },
                });
            }
        }

        // Position-specific output synthesis: pull the last DEFINITION
        // position's snapshot. Live-run path: in `completions`. Full
        // replay path (no fresh completions because everything ran in a
        // prior session): walk the log via `recover_logged_output`.
        let last_pos = scope_len - 1;
        let last_output =
            if let Some((_, snap)) = completion_snaps.iter().find(|(p, _)| *p == last_pos) {
                Some(snap.clone())
            } else {
                recover_logged_output(self, &container_name, 0, last_pos).await?
            };

        let synthetic = self
            .synthesise_container_output(
                step,
                &scope_def.outputs,
                &container_name,
                last_output.as_ref(),
                0,
            )
            .await;
        self.emit_container_success(step, start, synthetic).await
    }

    /// Execute a `call` — run `scope` exactly once. A scope-body gate's
    /// `break` ends the scope body early; it does **not** fail the
    /// container (v1 parity: `src/steps/call.rs`).
    async fn run_call(
        &mut self,
        step: &Step,
        start: std::time::Instant,
    ) -> Result<StepOutcome, EngineError> {
        let container_name = step.name.clone();
        // Top-level step index of the container. Used to attribute
        // `EngineError::StepFailed { step_index, .. }` correctly when a
        // scope-body cmd is interrupted by signal or step timeout —
        // otherwise the caller sees a zeroed index pointing at the
        // first workflow step rather than the container that owns the
        // scoped cmd.
        let container_top_level_index = top_level_position_of(self, &container_name).await?;
        let scope_name = match step.scope.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return self
                    .emit_container_failure(step, start, "call step missing `scope:` field".into())
                    .await;
            }
        };
        let scope_def = match self.workflow().scopes.get(&scope_name).cloned() {
            Some(s) => s,
            None => {
                return self
                    .emit_container_failure(
                        step,
                        start,
                        format!("scope `{scope_name}` referenced by call step `{container_name}` not found in workflow"),
                    )
                    .await;
            }
        };
        let scope_len = scope_def.steps.len() as u32;

        let state = ContainerReplayState::rebuild(self, &container_name, scope_len).await?;

        // `call` is a single pass. If iteration 0 is already past,
        // jump straight to the container's terminal emit.
        let last_output = if state.next_iteration == 0 && !state.broke {
            let seed = step
                .initial_value
                .clone()
                .unwrap_or(serde_json::Value::Null);
            match self
                .run_scope_body(step, &scope_def.steps, 0, seed, state.last_completed)
                .await?
            {
                IterationOutcome::Completed { last_output } => last_output,
                IterationOutcome::Break { last_output } => last_output,
                IterationOutcome::Failed(e) => {
                    // Body already emitted its scoped StepFailed and
                    // finalised the session. We now also emit a
                    // top-level StepFailed for the container itself so
                    // `progress_from_log().last_failed_step` attributes
                    // the failure to the container (not the scope-body
                    // step that raised it). `emit_container_failure`
                    // calls `finalise_fail` too — idempotent when the
                    // session is already `failed`.
                    return self.emit_container_failure(step, start, e).await;
                }
                IterationOutcome::Cancelled => return Ok(StepOutcome::Cancelled),
                IterationOutcome::Signal(signal) => {
                    return Err(EngineError::StepFailed {
                        step_index: container_top_level_index,
                        reason: stepyard_core::TerminationReason::SignalReceived(
                            stepyard_core::Signal::from(signal),
                        ),
                    });
                }
                IterationOutcome::TimedOut { configured_ms } => {
                    return Err(EngineError::StepFailed {
                        step_index: container_top_level_index,
                        reason: stepyard_core::TerminationReason::StepTimeout { configured_ms },
                    });
                }
            }
        } else {
            // Resume path: iteration 0 is already terminal — either
            // every body position has a `step_completed`, or a scoped
            // gate ended it with `skip`/`break`. Pull whatever last
            // cmd snapshot made it into the log (may be `None` when
            // the skip fired before any cmd ran).
            state.last_output_per_iteration.get(&0).cloned()
        };

        let synthetic = self
            .synthesise_container_output(
                step,
                &scope_def.outputs,
                &container_name,
                last_output.as_ref(),
                0,
            )
            .await;

        self.emit_container_success(step, start, synthetic).await
    }

    /// Execute a `repeat` — run `scope` until a scope-body gate fires
    /// `break`, the cancel token flips, or `max_iterations` fires. A
    /// repeat that exhausts its cap without a break completes
    /// successfully with a warning (v1 parity: `src/steps/repeat.rs`).
    async fn run_repeat(
        &mut self,
        step: &Step,
        start: std::time::Instant,
    ) -> Result<StepOutcome, EngineError> {
        let container_name = step.name.clone();
        // Top-level step index of the container. Used to attribute
        // `EngineError::StepFailed { step_index, .. }` correctly when a
        // scope-body cmd is interrupted by signal or step timeout —
        // otherwise the caller sees a zeroed index pointing at the
        // first workflow step rather than the container that owns the
        // scoped cmd.
        let container_top_level_index = top_level_position_of(self, &container_name).await?;
        let scope_name = match step.scope.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return self
                    .emit_container_failure(
                        step,
                        start,
                        "repeat step missing `scope:` field".into(),
                    )
                    .await;
            }
        };
        let scope_def = match self.workflow().scopes.get(&scope_name).cloned() {
            Some(s) => s,
            None => {
                return self
                    .emit_container_failure(
                        step,
                        start,
                        format!("scope `{scope_name}` referenced by repeat step `{container_name}` not found in workflow"),
                    )
                    .await;
            }
        };
        let scope_len = scope_def.steps.len() as u32;

        let mut state = ContainerReplayState::rebuild(self, &container_name, scope_len).await?;

        // Cap: defaults to 3 when `max_iterations` is absent — matches
        // v1 `src/steps/repeat.rs` (`unwrap_or(3)`), preventing unbounded
        // loops in workflows that forget both `max_iterations` and an
        // internal `break` gate.
        // Seeding `last_output_final` for the synthetic top-level
        // `step_completed`. Two paths matter for replay:
        //
        // * `state.broke == false` — replay landed mid-loop; the next
        //   iteration is `state.next_iteration` and the most recent
        //   completed cmd snap lives at `next_iteration - 1`.
        //
        // * `state.broke == true` — replay landed past a gate `break`; the
        //   container is about to synthesise its top-level output and never
        //   re-enter the loop. Reach into the BREAK iter's snap (any cmd
        //   that ran before the break logged it). When the break fired
        //   before any cmd in the break iter (e.g. body=[gate, ...]), fall
        //   back to the preceding iter's snap to match non-replay, where
        //   `IterationOutcome::Break { last_output: None }` leaves
        //   `last_output_final` at the value the prior `Completed` arm set.
        let mut last_output_final: Option<StepOutputSnapshot> = if state.broke {
            let break_iter = state.next_iteration;
            state
                .last_output_per_iteration
                .get(&break_iter)
                .cloned()
                .or_else(|| {
                    break_iter
                        .checked_sub(1)
                        .and_then(|prev| state.last_output_per_iteration.get(&prev).cloned())
                })
        } else {
            state
                .last_output_per_iteration
                .get(&state.next_iteration.saturating_sub(1))
                .cloned()
        };

        let max_iter = step.max_iterations.unwrap_or(3);

        while !state.broke {
            if self.is_cancelled() {
                return self.emit_container_cancel(step, start).await;
            }
            if state.next_iteration as usize >= max_iter {
                tracing::warn!(
                    container = %container_name,
                    max_iterations = max_iter,
                    "repeat hit max_iterations without break — completing successfully"
                );
                break;
            }

            // Seed scope.value for this iteration:
            // * iteration 0 → step.initial_value (if any)
            // * iteration N>0 → previous iteration's last cmd stdout
            //   serialized as JSON (string when plain; the renderer
            //   still sees it as a `value:`). v1 `repeat.rs` passes the
            //   prior iteration's last_output forward as scope.value.
            let scope_value = if state.next_iteration == 0 {
                step.initial_value
                    .clone()
                    .unwrap_or(serde_json::Value::Null)
            } else {
                state
                    .last_output_per_iteration
                    .get(&(state.next_iteration - 1))
                    .map(|snap| serde_json::Value::String(snap.stdout.clone()))
                    .unwrap_or(serde_json::Value::Null)
            };

            let iteration_start_index = state.next_iteration;
            let last_completed_for_iter = state
                .last_completed
                .filter(|(it, _)| *it == iteration_start_index);

            match self
                .run_scope_body(
                    step,
                    &scope_def.steps,
                    iteration_start_index,
                    scope_value,
                    last_completed_for_iter,
                )
                .await?
            {
                IterationOutcome::Completed { last_output } => {
                    if let Some(ref snap) = last_output {
                        state
                            .last_output_per_iteration
                            .insert(iteration_start_index, snap.clone());
                        last_output_final = Some(snap.clone());
                    }
                    state.next_iteration += 1;
                    state.last_completed =
                        Some((iteration_start_index, scope_len.saturating_sub(1)));
                }
                IterationOutcome::Break { last_output } => {
                    if let Some(ref snap) = last_output {
                        last_output_final = Some(snap.clone());
                    }
                    state.broke = true;
                }
                IterationOutcome::Failed(e) => {
                    // Body already emitted its scoped StepFailed and
                    // finalised the session. We now also emit a
                    // top-level StepFailed for the container itself so
                    // `progress_from_log().last_failed_step` attributes
                    // the failure to the container (not the scope-body
                    // step that raised it). `emit_container_failure`
                    // calls `finalise_fail` too — idempotent when the
                    // session is already `failed`.
                    return self.emit_container_failure(step, start, e).await;
                }
                IterationOutcome::Cancelled => return Ok(StepOutcome::Cancelled),
                IterationOutcome::Signal(signal) => {
                    return Err(EngineError::StepFailed {
                        step_index: container_top_level_index,
                        reason: stepyard_core::TerminationReason::SignalReceived(
                            stepyard_core::Signal::from(signal),
                        ),
                    });
                }
                IterationOutcome::TimedOut { configured_ms } => {
                    return Err(EngineError::StepFailed {
                        step_index: container_top_level_index,
                        reason: stepyard_core::TerminationReason::StepTimeout { configured_ms },
                    });
                }
            }
        }

        let synthetic = self
            .synthesise_container_output(
                step,
                &scope_def.outputs,
                &container_name,
                last_output_final.as_ref(),
                state.next_iteration.saturating_sub(1),
            )
            .await;
        self.emit_container_success(step, start, synthetic).await
    }

    /// Execute a `map` — render `items`, parse, and run `scope` once
    /// per item sequentially (even when `parallel:` is set). v1 parity:
    /// `src/steps/map.rs`. The synthetic container stdout is a JSON
    /// array of per-item last-cmd stdout.
    async fn run_map(
        &mut self,
        step: &Step,
        start: std::time::Instant,
    ) -> Result<StepOutcome, EngineError> {
        let container_name = step.name.clone();
        // Top-level step index of the container. Used to attribute
        // `EngineError::StepFailed { step_index, .. }` correctly when a
        // scope-body cmd is interrupted by signal or step timeout —
        // otherwise the caller sees a zeroed index pointing at the
        // first workflow step rather than the container that owns the
        // scoped cmd.
        let container_top_level_index = top_level_position_of(self, &container_name).await?;
        let scope_name = match step.scope.as_deref() {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => {
                return self
                    .emit_container_failure(step, start, "map step missing `scope:` field".into())
                    .await;
            }
        };
        let scope_def = match self.workflow().scopes.get(&scope_name).cloned() {
            Some(s) => s,
            None => {
                return self
                    .emit_container_failure(
                        step,
                        start,
                        format!("scope `{scope_name}` referenced by map step `{container_name}` not found in workflow"),
                    )
                    .await;
            }
        };
        let Some(items_template) = step.items.as_deref() else {
            return self
                .emit_container_failure(step, start, "map step missing `items:` field".into())
                .await;
        };

        // Rendering items reads from the top-level outputs map — rebuild
        // it from the log for replay-safety. `target` / `vars` come
        // from the engine's run_context.
        let top_level = top_level_outputs(self).await?;
        let ctx = RenderContext {
            steps: &top_level,
            target: &self.run_context().target,
            vars: &self.run_context().vars,
            scope: None,
        };
        let rendered = match render(items_template, &ctx) {
            Ok(s) => s,
            Err(e) => {
                return self
                    .emit_container_failure(step, start, format!("render `items`: {e}"))
                    .await;
            }
        };
        let items = parse_items(&rendered);
        let scope_len = scope_def.steps.len() as u32;

        let mut state = ContainerReplayState::rebuild(self, &container_name, scope_len).await?;

        while !state.broke && (state.next_iteration as usize) < items.len() {
            if self.is_cancelled() {
                return self.emit_container_cancel(step, start).await;
            }
            let iteration_index = state.next_iteration;
            let raw_item = &items[iteration_index as usize];
            // v1: try JSON-parse the item so `scope.value.<field>`
            // works when items are objects; fall back to a String.
            let scope_value = serde_json::from_str::<serde_json::Value>(raw_item)
                .unwrap_or_else(|_| serde_json::Value::String(raw_item.clone()));

            let last_completed_for_iter = state
                .last_completed
                .filter(|(it, _)| *it == iteration_index);

            match self
                .run_scope_body(
                    step,
                    &scope_def.steps,
                    iteration_index,
                    scope_value,
                    last_completed_for_iter,
                )
                .await?
            {
                IterationOutcome::Completed { last_output } => {
                    if let Some(snap) = last_output {
                        state
                            .last_output_per_iteration
                            .insert(iteration_index, snap);
                    }
                    state.next_iteration += 1;
                    state.last_completed = Some((iteration_index, scope_len.saturating_sub(1)));
                }
                IterationOutcome::Break { last_output } => {
                    if let Some(snap) = last_output {
                        state
                            .last_output_per_iteration
                            .insert(iteration_index, snap);
                    }
                    state.broke = true;
                }
                IterationOutcome::Failed(e) => {
                    // Body already emitted its scoped StepFailed and
                    // finalised the session. We now also emit a
                    // top-level StepFailed for the container itself so
                    // `progress_from_log().last_failed_step` attributes
                    // the failure to the container (not the scope-body
                    // step that raised it). `emit_container_failure`
                    // calls `finalise_fail` too — idempotent when the
                    // session is already `failed`.
                    return self.emit_container_failure(step, start, e).await;
                }
                IterationOutcome::Cancelled => return Ok(StepOutcome::Cancelled),
                IterationOutcome::Signal(signal) => {
                    return Err(EngineError::StepFailed {
                        step_index: container_top_level_index,
                        reason: stepyard_core::TerminationReason::SignalReceived(
                            stepyard_core::Signal::from(signal),
                        ),
                    });
                }
                IterationOutcome::TimedOut { configured_ms } => {
                    return Err(EngineError::StepFailed {
                        step_index: container_top_level_index,
                        reason: stepyard_core::TerminationReason::StepTimeout { configured_ms },
                    });
                }
            }
        }

        // Synthesise: JSON array of per-iteration last-cmd stdout in
        // iteration order. Per-iteration semantics:
        //
        // * Fully-run iteration with a last_output snapshot → its stdout.
        // * Skipped iteration (a scope-body gate fired `skip` before any
        //   cmd could emit an output, advancing `next_iteration`) → an
        //   explicit `""` slot. The position must remain present in the
        //   array; otherwise later items collapse into missing indices
        //   and the array becomes misaligned with the source `items`.
        // * Break iteration → array truncates here. If the break iter
        //   had a last_output (break gate fired AFTER an emit), that
        //   output is included and the loop halts on the next gap.
        //
        // The distinction between "skip is empty" and "break truncates"
        // is carried by `state.next_iteration`: skip advances it past
        // the iteration; break leaves it at the iteration.
        let mut collected: Vec<String> = Vec::with_capacity(items.len());
        for i in 0..items.len() as u32 {
            match state.last_output_per_iteration.get(&i) {
                Some(snap) => collected.push(snap.stdout.clone()),
                None if i < state.next_iteration => {
                    // Skipped iteration — no cmd emitted an output, but
                    // the slot is still occupied. Explicit "" keeps
                    // later items aligned and surfaces the semantic to
                    // callers reading the synthetic array.
                    collected.push(String::new());
                }
                None => break, // past progress — break truncated here
            }
        }
        let stdout = serde_json::to_string(&collected).unwrap_or_else(|_| "[]".into());
        let synthetic = StepOutputSnapshot {
            stdout,
            stderr: String::new(),
            exit_code: 0,
        };
        self.emit_container_success(step, start, synthetic).await
    }

    /// Walk a scope body for one iteration.
    async fn run_scope_body(
        &mut self,
        container: &Step,
        body: &[Step],
        iteration: u32,
        scope_value: serde_json::Value,
        last_completed: Option<(u32, u32)>,
    ) -> Result<IterationOutcome, EngineError> {
        let container_name = container.name.clone();
        let start_position = match last_completed {
            // We're resuming inside this iteration: skip positions
            // whose step_completed is already logged. The scoped
            // completion at position P means step at position P ran;
            // the next one to run is P + 1.
            Some((it, p)) if it == iteration => p + 1,
            _ => 0,
        };

        let mut last_output: Option<StepOutputSnapshot> = None;

        for (idx, body_step) in body.iter().enumerate() {
            let position = idx as u32;
            if position < start_position {
                // Replay past this position — pull its output from the
                // log so subsequent templates see it.
                if matches!(body_step.kind, StepKind::Cmd) {
                    if let Some(snap) =
                        recover_logged_output(self, &container_name, iteration, position).await?
                    {
                        last_output = Some(snap);
                    }
                }
                continue;
            }

            if self.is_cancelled() {
                // Emit StepFailed on the upcoming body step and cancel.
                let now = Utc::now();
                self.emit(Event::StepFailed {
                    step_name: body_step.name.clone(),
                    step_type: step_type_label(&body_step.kind).into(),
                    error: "Cancelled".into(),
                    duration_ms: 0,
                    timestamp: now,
                    sandboxed: matches!(body_step.kind, StepKind::Cmd),
                })
                .await?;
                self.finalise_cancel().await?;
                return Ok(IterationOutcome::Cancelled);
            }

            // Defensive nested-container guard. Adapter rejects this at
            // the CLI boundary; but a test-constructed workflow could
            // still slip through, so emit a StepFailed on the *container*
            // (not the nested step) and bail with no scoped events for
            // the inner step — user-anchored rule 4 for commit 3b.
            if matches!(
                body_step.kind,
                StepKind::Call | StepKind::Repeat | StepKind::Map
            ) {
                let err = format!(
                    "nested container step `{inner}` of type `{kind}` inside scope container `{outer}` is not supported in PR 3 of #31",
                    inner = body_step.name,
                    kind = step_type_label(&body_step.kind),
                    outer = container_name,
                );
                self.emit(Event::StepFailed {
                    step_name: container_name.clone(),
                    step_type: step_type_label(&container.kind).into(),
                    error: err.clone(),
                    duration_ms: 0,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(IterationOutcome::Failed(err));
            }

            let scope_ctx = ScopeContext {
                container: container_name.clone(),
                iteration,
                position,
            };

            match self
                .run_scope_body_step(container, body_step, &scope_ctx, &scope_value)
                .await?
            {
                BodyStep::Continue(snap) => {
                    if let Some(snap) = snap {
                        last_output = Some(snap);
                    }
                }
                BodyStep::Skip => return Ok(IterationOutcome::Completed { last_output }),
                BodyStep::Break => return Ok(IterationOutcome::Break { last_output }),
                BodyStep::Failed(e) => return Ok(IterationOutcome::Failed(e)),
                BodyStep::Cancelled => return Ok(IterationOutcome::Cancelled),
                BodyStep::Signal(s) => return Ok(IterationOutcome::Signal(s)),
                BodyStep::TimedOut { configured_ms } => {
                    return Ok(IterationOutcome::TimedOut { configured_ms })
                }
            }
        }

        Ok(IterationOutcome::Completed { last_output })
    }

    /// Dispatch a single scope-body step by kind. Cmd routes through
    /// [`Engine::execute_cmd_with_select`] so timeout/cancel/signal
    /// invariants apply identically to top-level cmd; gate routes
    /// through a scope-aware copy of [`Engine::run_gate_step`] that
    /// accepts `skip`/`break` actions.
    async fn run_scope_body_step(
        &mut self,
        container: &Step,
        body_step: &Step,
        scope_ctx: &ScopeContext,
        scope_value: &serde_json::Value,
    ) -> Result<BodyStep, EngineError> {
        match &body_step.kind {
            StepKind::Cmd => {
                self.run_scope_body_cmd(container, body_step, scope_ctx, scope_value)
                    .await
            }
            StepKind::Gate => {
                self.run_scope_body_gate(body_step, scope_ctx, scope_value)
                    .await
            }
            StepKind::Agent => {
                self.run_scope_body_agent(container, body_step, scope_ctx, scope_value)
                    .await
            }
            StepKind::Chat => {
                self.run_scope_body_chat(container, body_step, scope_ctx, scope_value)
                    .await
            }
            // Nested containers already handled one frame up. Other
            // kinds (script/template/parallel) are rejected by the
            // adapter; a test-constructed Step lands here and falls to
            // StepFailed on the body step.
            other => {
                let err = format!(
                    "step type `{other}` not yet supported inside scope bodies in v2 engine"
                );
                self.emit_scope_started(body_step, scope_ctx).await?;
                self.emit(Event::StepFailed {
                    step_name: body_step.name.clone(),
                    step_type: step_type_label(&body_step.kind).into(),
                    error: err.clone(),
                    duration_ms: 0,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                Ok(BodyStep::Failed(err))
            }
        }
    }

    /// Scope-body cmd — emits scoped `StepStarted`, then delegates to
    /// the same `execute_cmd_with_select` helper the top-level path
    /// uses so timeout/cancel/signal semantics stay identical.
    async fn run_scope_body_cmd(
        &mut self,
        container: &Step,
        body_step: &Step,
        scope_ctx: &ScopeContext,
        scope_value: &serde_json::Value,
    ) -> Result<BodyStep, EngineError> {
        self.emit_scope_started(body_step, scope_ctx).await?;

        // Resolve env — defaults < workflow < step. Identical to the
        // top-level cmd path. Failure here emits StepFailed + finalise.
        let resolved_env = match self.prepare_step(body_step) {
            Ok(e) => e,
            Err(e) => {
                let err = e.to_string();
                self.emit(Event::StepFailed {
                    step_name: body_step.name.clone(),
                    step_type: "cmd".into(),
                    error: err.clone(),
                    duration_ms: 0,
                    timestamp: Utc::now(),
                    sandboxed: true,
                })
                .await?;
                self.finalise_fail().await?;
                return Ok(BodyStep::Failed(err));
            }
        };

        // Render scope-body cmd's `command` field against the in-iteration
        // outputs + scope bindings. v1 supports templated commands inside
        // scopes (e.g. `echo {{ scope.value }}`), so keep parity.
        let rendered_command = {
            let in_scope = in_iteration_outputs(
                self,
                &scope_ctx.container,
                scope_ctx.iteration,
                scope_ctx.position,
            )
            .await?;
            let ctx = RenderContext {
                steps: &in_scope,
                target: &self.run_context().target,
                vars: &self.run_context().vars,
                scope: Some(ScopeView {
                    value: scope_value.clone(),
                    index: scope_ctx.iteration,
                }),
            };
            match render(&body_step.command, &ctx) {
                Ok(s) => s,
                Err(e) => {
                    let err = format!("render scope-body cmd `{}`: {e}", body_step.name);
                    self.emit(Event::StepFailed {
                        step_name: body_step.name.clone(),
                        step_type: "cmd".into(),
                        error: err.clone(),
                        duration_ms: 0,
                        timestamp: Utc::now(),
                        sandboxed: true,
                    })
                    .await?;
                    self.finalise_fail().await?;
                    return Ok(BodyStep::Failed(err));
                }
            }
        };

        let mut rendered_step = body_step.clone();
        rendered_step.command = rendered_command;

        // Top-level step_index of the container is the attribution
        // point for `StepTimeoutFired.step_index` on scoped cmds —
        // v1 has no global scoped index; the engine surfaces scope
        // position via `scope_context` on `StepStarted` / `StepCompleted`.
        let container_top_level_index = top_level_position_of(self, &scope_ctx.container).await?;
        let start = std::time::Instant::now();
        let outcome = self
            .execute_cmd_with_select(
                &rendered_step,
                &resolved_env,
                container_top_level_index,
                Some(scope_ctx.clone()),
                start,
            )
            .await?;
        let _ = container; // silence unused-param warning if no future use
        match outcome {
            CmdOutcome::Success(snap) => Ok(BodyStep::Continue(Some(snap))),
            CmdOutcome::Failed(e) => Ok(BodyStep::Failed(e)),
            CmdOutcome::Cancelled => Ok(BodyStep::Cancelled),
            CmdOutcome::Signal(s) => Ok(BodyStep::Signal(s)),
            CmdOutcome::TimedOut { configured_ms } => Ok(BodyStep::TimedOut { configured_ms }),
        }
    }

    /// Scope-body agent — same runtime as the top-level agent dispatcher,
    /// but prompt rendering sees the in-iteration `steps.*` map and emitted
    /// `StepCompleted` carries `scope_context`.
    async fn run_scope_body_agent(
        &mut self,
        container: &Step,
        body_step: &Step,
        scope_ctx: &ScopeContext,
        scope_value: &serde_json::Value,
    ) -> Result<BodyStep, EngineError> {
        self.emit_scope_started(body_step, scope_ctx).await?;

        let step_type = step_type_label(&body_step.kind);
        let start = std::time::Instant::now();
        let resolved_env = match self.prepare_step(body_step) {
            Ok(e) => e,
            Err(e) => {
                return self
                    .emit_scope_body_failure(body_step, start, e.to_string())
                    .await
            }
        };
        let rendered_prompt =
            match render_scope_prompt(self, body_step, scope_ctx, scope_value).await? {
                Ok(s) => s,
                Err(e) => {
                    return self
                        .emit_scope_body_failure(
                            body_step,
                            start,
                            format!("agent prompt render failed: {e}"),
                        )
                        .await;
                }
            };

        let progress = self.progress_from_log().await?;
        let state = crate::agent_exec::AgentSessionState {
            agent_session_ids: &progress.agent_session_ids,
            first_agent_session_id: progress.first_agent_session_id.as_deref(),
        };
        let container_top_level_index = top_level_position_of(self, &scope_ctx.container).await?;

        let cancel_token = self.cancel_token();
        let signal_slot = self.shutdown_signal();
        let step_timeout = body_step.timeout;
        let selection: crate::engine::StepSelection<
            Result<crate::agent_exec::AgentExecOutput, crate::agent_exec::AgentExecError>,
        > = {
            let exec_fut = crate::agent_exec::run_agent_step(
                body_step,
                &rendered_prompt,
                &state,
                &resolved_env,
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
            let shutdown_rx = self.shutdown_rx_mut();
            let shutdown_fut = shutdown_rx.recv();
            tokio::pin!(exec_fut);
            tokio::pin!(cancel_fut);
            tokio::pin!(timeout_fut);
            tokio::pin!(shutdown_fut);
            tokio::select! {
                r = &mut exec_fut => crate::engine::StepSelection::Done(r),
                _ = &mut cancel_fut => crate::engine::StepSelection::Cancelled,
                _ = &mut timeout_fut => crate::engine::StepSelection::TimedOut,
                _ = &mut shutdown_fut => crate::engine::StepSelection::Signal(
                    signal_slot
                        .get()
                        .cloned()
                        .unwrap_or_else(|| "unknown".into()),
                ),
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        if let crate::engine::StepSelection::Signal(ref signal) = selection {
            let signal = signal.clone();
            self.emit(Event::SignalReceived {
                signal: Signal::from(signal.clone()),
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: body_step.name.clone(),
                step_type: step_type.into(),
                error: format!("Signal: {signal}"),
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_cancel().await?;
            let _ = container;
            let _ = container_top_level_index;
            return Ok(BodyStep::Signal(signal));
        }

        if let crate::engine::StepSelection::TimedOut = selection {
            let configured_ms = body_step
                .timeout
                .expect("TimedOut requires step.timeout.is_some()")
                .as_millis() as u64;
            self.emit(Event::StepTimeoutFired {
                step_index: container_top_level_index,
                configured_ms,
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: body_step.name.clone(),
                step_type: step_type.into(),
                error: format!("step timed out after {configured_ms}ms"),
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_fail().await?;
            let _ = container;
            return Ok(BodyStep::TimedOut { configured_ms });
        }

        let exec_result = match selection {
            crate::engine::StepSelection::Done(r) => r,
            crate::engine::StepSelection::Cancelled => {
                self.emit(Event::StepFailed {
                    step_name: body_step.name.clone(),
                    step_type: step_type.into(),
                    error: "Cancelled".into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_cancel().await?;
                let _ = container;
                return Ok(BodyStep::Cancelled);
            }
            crate::engine::StepSelection::TimedOut => unreachable!("handled above"),
            crate::engine::StepSelection::Signal(_) => unreachable!("handled above"),
        };

        let output = match exec_result {
            Ok(o) => o,
            Err(e) => {
                return self
                    .emit_scope_body_failure(body_step, start, e.to_string())
                    .await
            }
        };
        let snapshot = StepOutputSnapshot {
            stdout: output.response,
            stderr: String::new(),
            exit_code: 0,
        };
        self.emit(Event::StepCompleted {
            step_name: body_step.name.clone(),
            step_type: step_type.into(),
            duration_ms,
            timestamp: Utc::now(),
            input_tokens: output.input_tokens,
            output_tokens: output.output_tokens,
            cost_usd: output.cost_usd,
            sandboxed: false,
            output: Some(snapshot.clone()),
            scope_context: Some(scope_ctx.clone()),
            gate_outcome: None,
            agent_session_id: output.session_id,
        })
        .await?;
        let _ = container;
        Ok(BodyStep::Continue(Some(snapshot)))
    }

    /// Scope-body chat — same provider seam as the top-level chat dispatcher,
    /// but prompt rendering sees the in-iteration scope context and terminal
    /// completion is scoped.
    async fn run_scope_body_chat(
        &mut self,
        container: &Step,
        body_step: &Step,
        scope_ctx: &ScopeContext,
        scope_value: &serde_json::Value,
    ) -> Result<BodyStep, EngineError> {
        self.emit_scope_started(body_step, scope_ctx).await?;

        let step_type = step_type_label(&body_step.kind);
        let start = std::time::Instant::now();
        let resolved_env = match self.prepare_step(body_step) {
            Ok(e) => e,
            Err(e) => {
                return self
                    .emit_scope_body_failure(body_step, start, e.to_string())
                    .await
            }
        };
        let rendered_prompt =
            match render_scope_prompt(self, body_step, scope_ctx, scope_value).await? {
                Ok(s) => s,
                Err(e) => {
                    return self
                        .emit_scope_body_failure(
                            body_step,
                            start,
                            format!("chat prompt render failed: {e}"),
                        )
                        .await;
                }
            };
        let session_bucket = body_step
            .chat_session
            .clone()
            .unwrap_or_else(|| body_step.name.clone());
        let progress = self.progress_from_log().await?;
        let history = progress
            .chat_sessions
            .get(&session_bucket)
            .cloned()
            .unwrap_or_default();
        let resolved_api_key = match crate::chat_exec::resolve_api_key(
            &body_step.name,
            body_step.api_key_env.as_deref(),
            &resolved_env,
        ) {
            Ok(v) => v,
            Err(e) => {
                return self
                    .emit_scope_body_failure(body_step, start, e.to_string())
                    .await
            }
        };
        let Some(client) = self.chat_client() else {
            let error = crate::chat_exec::ChatExecError::NoClientConfigured {
                step: body_step.name.clone(),
            }
            .to_string();
            return self.emit_scope_body_failure(body_step, start, error).await;
        };

        self.emit(Event::ChatMessageAppended {
            step_name: body_step.name.clone(),
            session: session_bucket.clone(),
            role: stepyard_core::ChatRole::User,
            content: rendered_prompt.clone(),
            timestamp: Utc::now(),
        })
        .await?;
        let req = crate::chat_exec::build_chat_request(
            body_step,
            rendered_prompt,
            history,
            resolved_api_key,
        );

        let cancel_token = self.cancel_token();
        let signal_slot = self.shutdown_signal();
        let step_timeout = body_step.timeout;
        let selection: crate::engine::StepSelection<
            Result<crate::chat_exec::ChatExecOutput, crate::chat_exec::ChatExecError>,
        > = {
            let exec_fut = crate::chat_exec::run_chat_step(body_step, req, client.as_ref());
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
            let shutdown_rx = self.shutdown_rx_mut();
            let shutdown_fut = shutdown_rx.recv();
            tokio::pin!(exec_fut);
            tokio::pin!(cancel_fut);
            tokio::pin!(timeout_fut);
            tokio::pin!(shutdown_fut);
            tokio::select! {
                r = &mut exec_fut => crate::engine::StepSelection::Done(r),
                _ = &mut cancel_fut => crate::engine::StepSelection::Cancelled,
                _ = &mut timeout_fut => crate::engine::StepSelection::TimedOut,
                _ = &mut shutdown_fut => crate::engine::StepSelection::Signal(
                    signal_slot
                        .get()
                        .cloned()
                        .unwrap_or_else(|| "unknown".into()),
                ),
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        if let crate::engine::StepSelection::Signal(ref signal) = selection {
            let signal_name = signal.clone();
            let signal = Signal::from(signal_name.clone());
            let error = crate::chat_exec::ChatExecError::SignalReceived {
                step: body_step.name.clone(),
                signal: signal.clone(),
            }
            .to_string();
            self.emit(Event::SignalReceived { signal }).await?;
            self.emit(Event::StepFailed {
                step_name: body_step.name.clone(),
                step_type: step_type.into(),
                error,
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_cancel().await?;
            let _ = container;
            return Ok(BodyStep::Signal(signal_name));
        }

        if let crate::engine::StepSelection::TimedOut = selection {
            let configured_ms = body_step
                .timeout
                .expect("TimedOut requires step.timeout.is_some()")
                .as_millis() as u64;
            let error = crate::chat_exec::ChatExecError::Timeout {
                step: body_step.name.clone(),
                configured_ms,
            }
            .to_string();
            self.emit(Event::StepTimeoutFired {
                step_index: top_level_position_of(self, &scope_ctx.container).await?,
                configured_ms,
            })
            .await?;
            self.emit(Event::StepFailed {
                step_name: body_step.name.clone(),
                step_type: step_type.into(),
                error,
                duration_ms,
                timestamp: Utc::now(),
                sandboxed: false,
            })
            .await?;
            self.finalise_fail().await?;
            let _ = container;
            return Ok(BodyStep::TimedOut { configured_ms });
        }

        let exec_result = match selection {
            crate::engine::StepSelection::Done(r) => r,
            crate::engine::StepSelection::Cancelled => {
                let error = crate::chat_exec::ChatExecError::Cancelled {
                    step: body_step.name.clone(),
                }
                .to_string();
                self.emit(Event::StepFailed {
                    step_name: body_step.name.clone(),
                    step_type: step_type.into(),
                    error,
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_cancel().await?;
                let _ = container;
                return Ok(BodyStep::Cancelled);
            }
            crate::engine::StepSelection::TimedOut => unreachable!("handled above"),
            crate::engine::StepSelection::Signal(_) => unreachable!("handled above"),
        };

        let output = match exec_result {
            Ok(o) => o,
            Err(e) => {
                return self
                    .emit_scope_body_failure(body_step, start, e.to_string())
                    .await
            }
        };
        self.emit(Event::ChatMessageAppended {
            step_name: body_step.name.clone(),
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
            step_name: body_step.name.clone(),
            step_type: step_type.into(),
            duration_ms,
            timestamp: Utc::now(),
            input_tokens: output.input_tokens,
            output_tokens: output.output_tokens,
            cost_usd: output.cost_usd,
            sandboxed: false,
            output: Some(snapshot.clone()),
            scope_context: Some(scope_ctx.clone()),
            gate_outcome: None,
            agent_session_id: None,
        })
        .await?;
        let _ = container;
        Ok(BodyStep::Continue(Some(snapshot)))
    }

    /// Scope-body gate — pure template eval, no sandbox. Accepts the
    /// wider `skip`/`break` action vocabulary (top-level gate does
    /// not; see [`GateAction::parse_scoped`]).
    async fn run_scope_body_gate(
        &mut self,
        body_step: &Step,
        scope_ctx: &ScopeContext,
        scope_value: &serde_json::Value,
    ) -> Result<BodyStep, EngineError> {
        self.emit_scope_started(body_step, scope_ctx).await?;

        let start = std::time::Instant::now();
        let on_pass = match GateAction::parse_scoped(body_step.on_pass.as_deref()) {
            Ok(a) => a,
            Err(e) => {
                return self
                    .emit_scope_gate_failure(body_step, start, e.to_string())
                    .await
            }
        };
        let on_fail = match GateAction::parse_scoped(body_step.on_fail.as_deref()) {
            Ok(a) => a,
            Err(e) => {
                return self
                    .emit_scope_gate_failure(body_step, start, e.to_string())
                    .await
            }
        };
        let Some(condition) = body_step
            .condition
            .as_deref()
            .filter(|c| !c.trim().is_empty())
        else {
            let err = GateError::MissingCondition {
                step: body_step.name.clone(),
            };
            return self
                .emit_scope_gate_failure(body_step, start, err.to_string())
                .await;
        };

        let in_scope = in_iteration_outputs(
            self,
            &scope_ctx.container,
            scope_ctx.iteration,
            scope_ctx.position,
        )
        .await?;
        let ctx = RenderContext {
            steps: &in_scope,
            target: &self.run_context().target,
            vars: &self.run_context().vars,
            scope: Some(ScopeView {
                value: scope_value.clone(),
                index: scope_ctx.iteration,
            }),
        };
        let rendered = match render(condition, &ctx) {
            Ok(s) => s,
            Err(e) => {
                return self
                    .emit_scope_gate_failure(body_step, start, e.to_string())
                    .await
            }
        };
        let passed = match evaluate_bool(&body_step.name, &rendered) {
            Ok(b) => b,
            Err(e) => {
                return self
                    .emit_scope_gate_failure(body_step, start, e.to_string())
                    .await
            }
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        match outcome_for(passed, on_pass, on_fail, body_step.message.as_deref()) {
            GateOutcome::Continue => {
                self.emit(Event::StepCompleted {
                    step_name: body_step.name.clone(),
                    step_type: "gate".into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                    sandboxed: false,
                    output: None,
                    scope_context: Some(scope_ctx.clone()),
                    gate_outcome: Some(stepyard_core::GateOutcome::Continue),
                    agent_session_id: None,
                })
                .await?;
                Ok(BodyStep::Continue(None))
            }
            GateOutcome::Skip => {
                self.emit(Event::StepCompleted {
                    step_name: body_step.name.clone(),
                    step_type: "gate".into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                    sandboxed: false,
                    output: None,
                    scope_context: Some(scope_ctx.clone()),
                    gate_outcome: Some(stepyard_core::GateOutcome::Skip),
                    agent_session_id: None,
                })
                .await?;
                Ok(BodyStep::Skip)
            }
            GateOutcome::Break => {
                self.emit(Event::StepCompleted {
                    step_name: body_step.name.clone(),
                    step_type: "gate".into(),
                    duration_ms,
                    timestamp: Utc::now(),
                    input_tokens: None,
                    output_tokens: None,
                    cost_usd: None,
                    sandboxed: false,
                    output: None,
                    scope_context: Some(scope_ctx.clone()),
                    gate_outcome: Some(stepyard_core::GateOutcome::Break),
                    agent_session_id: None,
                })
                .await?;
                Ok(BodyStep::Break)
            }
            GateOutcome::Fail { message } => {
                let err = GateError::Failed {
                    step: body_step.name.clone(),
                    message,
                }
                .to_string();
                self.emit(Event::StepFailed {
                    step_name: body_step.name.clone(),
                    step_type: "gate".into(),
                    error: err.clone(),
                    duration_ms,
                    timestamp: Utc::now(),
                    sandboxed: false,
                })
                .await?;
                self.finalise_fail().await?;
                Ok(BodyStep::Failed(err))
            }
        }
    }

    async fn emit_scope_started(
        &mut self,
        body_step: &Step,
        scope_ctx: &ScopeContext,
    ) -> Result<(), EngineError> {
        self.emit(Event::StepStarted {
            step_name: body_step.name.clone(),
            step_type: step_type_label(&body_step.kind).into(),
            timestamp: Utc::now(),
            scope_context: Some(scope_ctx.clone()),
        })
        .await
    }

    async fn emit_scope_gate_failure(
        &mut self,
        body_step: &Step,
        start: std::time::Instant,
        error: String,
    ) -> Result<BodyStep, EngineError> {
        let duration_ms = start.elapsed().as_millis() as u64;
        self.emit(Event::StepFailed {
            step_name: body_step.name.clone(),
            step_type: "gate".into(),
            error: error.clone(),
            duration_ms,
            timestamp: Utc::now(),
            sandboxed: false,
        })
        .await?;
        self.finalise_fail().await?;
        Ok(BodyStep::Failed(error))
    }

    async fn emit_scope_body_failure(
        &mut self,
        body_step: &Step,
        start: std::time::Instant,
        error: String,
    ) -> Result<BodyStep, EngineError> {
        let duration_ms = start.elapsed().as_millis() as u64;
        self.emit(Event::StepFailed {
            step_name: body_step.name.clone(),
            step_type: step_type_label(&body_step.kind).into(),
            error: error.clone(),
            duration_ms,
            timestamp: Utc::now(),
            sandboxed: false,
        })
        .await?;
        self.finalise_fail().await?;
        Ok(BodyStep::Failed(error))
    }

    /// Emit the container's terminal top-level `StepCompleted` with a
    /// synthetic `output` snapshot. No `scope_context`, no `gate_outcome`.
    async fn emit_container_success(
        &mut self,
        step: &Step,
        start: std::time::Instant,
        synthetic: StepOutputSnapshot,
    ) -> Result<StepOutcome, EngineError> {
        let duration_ms = start.elapsed().as_millis() as u64;
        self.emit(Event::StepCompleted {
            step_name: step.name.clone(),
            step_type: step_type_label(&step.kind).into(),
            duration_ms,
            timestamp: Utc::now(),
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
            sandboxed: false,
            output: Some(synthetic),
            scope_context: None,
            gate_outcome: None,
            agent_session_id: None,
        })
        .await?;
        Ok(StepOutcome::StepCompleted {
            step_name: step.name.clone(),
        })
    }

    async fn emit_container_failure(
        &mut self,
        step: &Step,
        start: std::time::Instant,
        error: String,
    ) -> Result<StepOutcome, EngineError> {
        let duration_ms = start.elapsed().as_millis() as u64;
        self.emit(Event::StepFailed {
            step_name: step.name.clone(),
            step_type: step_type_label(&step.kind).into(),
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

    async fn emit_container_cancel(
        &mut self,
        step: &Step,
        start: std::time::Instant,
    ) -> Result<StepOutcome, EngineError> {
        let duration_ms = start.elapsed().as_millis() as u64;
        self.emit(Event::StepFailed {
            step_name: step.name.clone(),
            step_type: step_type_label(&step.kind).into(),
            error: "Cancelled".into(),
            duration_ms,
            timestamp: Utc::now(),
            sandboxed: false,
        })
        .await?;
        self.finalise_cancel().await?;
        Ok(StepOutcome::Cancelled)
    }

    async fn emit_container_signal(
        &mut self,
        step: &Step,
        start: std::time::Instant,
        signal: String,
    ) -> Result<(), EngineError> {
        let duration_ms = start.elapsed().as_millis() as u64;
        self.emit(Event::SignalReceived {
            signal: Signal::from(signal.clone()),
        })
        .await?;
        self.emit(Event::StepFailed {
            step_name: step.name.clone(),
            step_type: step_type_label(&step.kind).into(),
            error: format!("Signal: {signal}"),
            duration_ms,
            timestamp: Utc::now(),
            sandboxed: false,
        })
        .await?;
        self.finalise_cancel().await
    }

    async fn emit_container_timeout(
        &mut self,
        step: &Step,
        start: std::time::Instant,
        sub_step_name: &str,
        configured_ms: u64,
    ) -> Result<(), EngineError> {
        let duration_ms = start.elapsed().as_millis() as u64;
        self.emit(Event::StepTimeoutFired {
            step_index: top_level_position_of(self, &step.name).await?,
            configured_ms,
        })
        .await?;
        self.emit(Event::StepFailed {
            step_name: step.name.clone(),
            step_type: step_type_label(&step.kind).into(),
            error: format!("parallel sub-step `{sub_step_name}` timed out after {configured_ms}ms"),
            duration_ms,
            timestamp: Utc::now(),
            sandboxed: false,
        })
        .await?;
        let _ = self
            .lifecycle()
            .destroy_by_session(*self.session().id().as_uuid())
            .await;
        self.finalise_fail().await
    }

    /// Compute the synthetic container stdout. `outputs:` template (step-
    /// level then scope-level) is rendered against the **last
    /// iteration's** in-scope outputs, with `scope.value = last stdout`
    /// as a convenience. `None` → fall back to `last_output.stdout`.
    async fn synthesise_container_output(
        &self,
        step: &Step,
        scope_outputs: &Option<String>,
        container_name: &str,
        last_output: Option<&StepOutputSnapshot>,
        last_iteration: u32,
    ) -> StepOutputSnapshot {
        let template = step.outputs.as_deref().or(scope_outputs.as_deref());
        if let Some(tpl) = template {
            let in_scope = in_iteration_outputs(self, container_name, last_iteration, u32::MAX)
                .await
                .unwrap_or_default();
            let ctx_value = last_output
                .map(|snap| serde_json::Value::String(snap.stdout.clone()))
                .unwrap_or(serde_json::Value::Null);
            let ctx = RenderContext {
                steps: &in_scope,
                target: &self.run_context().target,
                vars: &self.run_context().vars,
                scope: Some(ScopeView {
                    value: ctx_value,
                    index: last_iteration,
                }),
            };
            if let Ok(rendered) = render(tpl, &ctx) {
                return StepOutputSnapshot {
                    stdout: rendered,
                    stderr: String::new(),
                    exit_code: 0,
                };
            }
        }
        StepOutputSnapshot {
            stdout: last_output.map(|s| s.stdout.clone()).unwrap_or_default(),
            stderr: String::new(),
            exit_code: 0,
        }
    }
}

/// Rebuild the top-level cross-step outputs map from the session log.
/// Mirrors the filter applied by [`Engine::progress_from_log`]: only
/// completions with a `null`/absent `scope_context` feed this map.
async fn top_level_outputs(
    engine: &Engine,
) -> Result<HashMap<String, StepOutputSnapshot>, EngineError> {
    let events = engine
        .session_handle()
        .replay()
        .await
        .map_err(EngineError::from)?;
    let mut out: HashMap<String, StepOutputSnapshot> = HashMap::new();
    for evt in events.iter() {
        let Some(tag) = evt.payload.get("event").and_then(|v| v.as_str()) else {
            continue;
        };
        if tag != "step_completed" {
            continue;
        }
        let is_scoped = evt
            .payload
            .get("scope_context")
            .map(|v| !v.is_null())
            .unwrap_or(false);
        if is_scoped {
            continue;
        }
        let Some(name) = evt.payload.get("step_name").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(raw) = evt.payload.get("output").filter(|v| !v.is_null()) {
            let snap: StepOutputSnapshot =
                serde_json::from_value(raw.clone()).map_err(|e| {
                    EngineError::InvalidState(format!(
                        "scope replay: top-level step_completed log entry has malformed `output` payload (step=`{name}`): {e}"
                    ))
                })?;
            out.insert(name.to_string(), snap);
        }
    }
    Ok(out)
}

/// Find the top-level step index for `container_name` by scanning the
/// workflow's top-level step list. Used for `StepTimeoutFired.step_index`
/// attribution when a scope-body cmd times out.
async fn top_level_position_of(engine: &Engine, container_name: &str) -> Result<u32, EngineError> {
    engine
        .workflow()
        .steps
        .iter()
        .position(|s| s.name == container_name)
        .map(|p| p as u32)
        .ok_or_else(|| {
            EngineError::InvalidState(format!(
                "scope runner cannot locate container `{container_name}` in workflow.steps"
            ))
        })
}

async fn render_scope_prompt(
    engine: &Engine,
    body_step: &Step,
    scope_ctx: &ScopeContext,
    scope_value: &serde_json::Value,
) -> Result<Result<String, String>, EngineError> {
    let Some(prompt_template) = body_step.prompt.as_deref() else {
        return Ok(Err(format!(
            "{} step `{}` has no prompt — the adapter should have rejected this at load time",
            step_type_label(&body_step.kind),
            body_step.name
        )));
    };
    let in_scope = in_iteration_outputs(
        engine,
        &scope_ctx.container,
        scope_ctx.iteration,
        scope_ctx.position,
    )
    .await?;
    let ctx = RenderContext {
        steps: &in_scope,
        target: &engine.run_context().target,
        vars: &engine.run_context().vars,
        scope: Some(ScopeView {
            value: scope_value.clone(),
            index: scope_ctx.iteration,
        }),
    };
    Ok(render(prompt_template, &ctx).map_err(|e| e.to_string()))
}

/// Replay-skip set for parallel: every position with a logged
/// `step_completed` for `(container == container_name, iteration == 0)`.
///
/// `ContainerReplayState` is unsuitable for parallel because it assumes
/// sequential completion (a logged `(iter, p)` implies positions
/// `0..=p` are all done). Parallel sub-steps complete in arbitrary
/// order, so we walk the log directly and collect a position set.
async fn parallel_completed_positions(
    engine: &Engine,
    container_name: &str,
) -> Result<HashSet<u32>, EngineError> {
    let events = engine
        .session_handle()
        .replay()
        .await
        .map_err(EngineError::from)?;
    let mut set: HashSet<u32> = HashSet::new();
    for evt in events.iter() {
        let Some(tag) = evt.payload.get("event").and_then(|v| v.as_str()) else {
            continue;
        };
        if tag != "step_completed" {
            continue;
        }
        let Some(sc) = evt.payload.get("scope_context").filter(|v| !v.is_null()) else {
            continue;
        };
        let Some(container) = sc.get("container").and_then(|v| v.as_str()) else {
            continue;
        };
        if container != container_name {
            continue;
        }
        let Some(iter) = sc.get("iteration").and_then(|v| v.as_u64()) else {
            continue;
        };
        if iter as u32 != 0 {
            continue;
        }
        let Some(pos) = sc.get("position").and_then(|v| v.as_u64()) else {
            continue;
        };
        set.insert(pos as u32);
    }
    Ok(set)
}

/// Look up the already-logged output snapshot for a scoped
/// `(container, iteration, position)` triple. Called during
/// post-crash replay to re-populate `last_output` without re-running
/// the step.
async fn recover_logged_output(
    engine: &Engine,
    container_name: &str,
    iteration: u32,
    position: u32,
) -> Result<Option<StepOutputSnapshot>, EngineError> {
    let events = engine
        .session_handle()
        .replay()
        .await
        .map_err(EngineError::from)?;
    for evt in events.iter() {
        let Some(tag) = evt.payload.get("event").and_then(|v| v.as_str()) else {
            continue;
        };
        if tag != "step_completed" {
            continue;
        }
        let Some(sc) = evt.payload.get("scope_context").filter(|v| !v.is_null()) else {
            continue;
        };
        let Some(container) = sc.get("container").and_then(|v| v.as_str()) else {
            continue;
        };
        if container != container_name {
            continue;
        }
        let Some(iter) = sc.get("iteration").and_then(|v| v.as_u64()) else {
            continue;
        };
        let Some(pos) = sc.get("position").and_then(|v| v.as_u64()) else {
            continue;
        };
        if iter as u32 != iteration || pos as u32 != position {
            continue;
        }
        if let Some(raw) = evt.payload.get("output").filter(|v| !v.is_null()) {
            let snap: StepOutputSnapshot =
                serde_json::from_value(raw.clone()).map_err(|e| {
                    EngineError::InvalidState(format!(
                        "scope replay: step_completed log entry has malformed `output` payload (container=`{container_name}`, iteration={iteration}, position={position}): {e}"
                    ))
                })?;
            return Ok(Some(snap));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_items_json_array_of_strings() {
        let out = parse_items(r#"["a", "b", "c"]"#);
        assert_eq!(out, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_items_json_array_coerces_non_strings_via_to_string() {
        let out = parse_items(r#"[1, 2, {"name": "alpha"}]"#);
        // Numbers render as bare digits; the object renders as JSON.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], "1");
        assert_eq!(out[1], "2");
        assert!(out[2].contains("alpha"), "got: {}", out[2]);
    }

    #[test]
    fn parse_items_line_split_fallback() {
        let out = parse_items("alpha\nbeta\n\ngamma\n");
        assert_eq!(out, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn parse_items_invalid_json_starting_with_bracket_falls_back_to_lines() {
        // v1 parity: a malformed JSON prefix does NOT fail — it falls
        // through to line-splitting.
        let out = parse_items("[alpha, beta]");
        assert_eq!(out, vec!["[alpha, beta]"]);
    }

    #[test]
    fn parse_items_empty_input_is_empty() {
        assert!(parse_items("").is_empty());
        assert!(parse_items("   \n  \n").is_empty());
    }
}
