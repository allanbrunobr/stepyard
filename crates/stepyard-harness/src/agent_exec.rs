//! Agent step executor (PR 5a of Task #31) — argv builder + runtime spawner.
//!
//! Commit 3a landed the typed shape + adapter; commit 3b (this file
//! across two commits) lands the pure [`build_agent_argv`] contract
//! together with the async [`run_agent_step`] spawner that drives the
//! Claude CLI. The pure argv builder is tested with unit tests; the
//! async spawner is exercised end-to-end via the
//! `tests/agent_replay.rs` integration tests against a mock binary.
//!
//! # Contract
//!
//! [`build_agent_argv`] returns the Claude CLI argv *after* the binary
//! path (mirroring v1 `AgentExecutor::build_args` at
//! `src/steps/agent.rs:20-60`, which also returns args-only and lets the
//! spawner supply the binary via the step's `command:` field). The
//! caller wraps it in `Command::new(bin).args(argv)` when spawning.
//!
//! # Session-resolution rules (v1 parity, with one deliberate fix)
//!
//! | Step fields                       | argv extension                                 |
//! |-----------------------------------|-------------------------------------------------|
//! | `resume: <name>` set              | `--resume <captured_id>`                        |
//! | `fork_session: <name>` set        | `--fork-session --resume <captured_id>` **(1)** |
//! | neither set, session=Shared(*)    | `--fork-session --resume <first_captured_id>`   |
//! | neither set, session=Isolated     | *(nothing)*                                     |
//! | neither set, Shared, no capture   | *(nothing)* — first agent step of the workflow  |
//!
//! (*) default when `agent_session` is `None`.
//!
//! **(1)** v1 emits a bare `--resume <id>` for explicit `fork_session`
//! (see `src/steps/agent.rs:47-50`), relying on implicit Claude CLI
//! fork semantics that the field name (`fork_session`) doesn't match.
//! v2 emits `--fork-session --resume <id>` so the argv reflects the
//! semantic intent. This is a **deliberate behavior change** from v1,
//! tested explicitly at [`tests::explicit_fork_session_emits_fork_session_and_resume`]
//! so it is not a hidden parity drift.
//!
//! # Scope
//!
//! * Adapter-layer guards (mutual exclusion of `resume` / `fork_session`,
//!   missing `prompt`, invalid `permissions` string) are enforced at
//!   `src/cli/harness_adapter.rs`. The builder trusts its input was
//!   already through the adapter.
//! * The `prompt` field is *not* part of argv — it's piped to the child
//!   process's stdin by the runtime spawner (next commit on PR 5a).
//! * The CLI binary path is *not* part of argv — it's the first arg to
//!   `Command::new(...)` in the spawner.

use std::collections::HashMap;

use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::workflow::{AgentPermissions, AgentSessionMode, Step};

/// Failure modes for [`build_agent_argv`]. These are pure
/// argv-construction errors; spawn / IO / parse failures are surfaced
/// separately by [`AgentExecError`].
#[derive(Debug, thiserror::Error)]
pub(crate) enum BuildArgvError {
    /// `step.resume: <target>` names an agent step that either did not
    /// run yet or did not capture a session_id. Surfaces the offending
    /// step and target so the terminal event has enough context for
    /// operators to diagnose without re-reading the YAML.
    #[error(
        "agent step `{step}` names `resume: {target}` but no captured \
         session_id for `{target}` was found in the session log"
    )]
    ResumeTargetNotFound { step: String, target: String },

    /// Same shape as [`Self::ResumeTargetNotFound`] but for the
    /// `fork_session:` key. Kept as a distinct variant so tests / logs
    /// can distinguish which YAML key the operator typo'd.
    #[error(
        "agent step `{step}` names `fork_session: {target}` but no captured \
         session_id for `{target}` was found in the session log"
    )]
    ForkSessionTargetNotFound { step: String, target: String },
}

/// Snapshot of the session-log-derived state the argv builder needs.
/// Mirrors the two fields on [`crate::engine::Progress`] that hold
/// captured agent session_ids, but is declared here so argv unit tests
/// don't reach into module-private engine types.
///
/// Held by reference to avoid cloning the whole map for every
/// dispatched agent step.
pub(crate) struct AgentSessionState<'a> {
    /// `step_name → session_id` for every top-level agent step that
    /// completed successfully in the log scan. Explicit
    /// `resume:` / `fork_session:` targets are resolved against this.
    pub agent_session_ids: &'a HashMap<String, String>,
    /// First-wins captured session_id from the forward scan of the log
    /// (v1 `SessionManager::capture` parity). Consumed by the
    /// default-shared path when no explicit target is set.
    pub first_agent_session_id: Option<&'a str>,
}

/// Build the Claude CLI argv for an agent step.
///
/// `step.kind` is expected to be [`crate::workflow::StepKind::Agent`];
/// this is enforced at the engine dispatch site, not here — the
/// builder is generic over any [`Step`] with agent-shaped fields so it
/// stays unit-testable without constructing a full dispatcher.
pub(crate) fn build_agent_argv(
    step: &Step,
    state: &AgentSessionState<'_>,
) -> Result<Vec<String>, BuildArgvError> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--verbose".into(),
        "--output-format".into(),
        "stream-json".into(),
    ];

    if let Some(model) = step.model.as_deref() {
        args.push("--model".into());
        args.push(model.into());
    }
    if let Some(sp) = step.system_prompt_append.as_deref() {
        args.push("--append-system-prompt".into());
        args.push(sp.into());
    }
    if matches!(step.permissions, Some(AgentPermissions::Skip)) {
        args.push("--dangerously-skip-permissions".into());
    }

    // Explicit `resume:` / `fork_session:` short-circuit the workflow
    // session default. The adapter rejects the (Some, Some) combo so
    // the builder doesn't need to defend against it — any breach here
    // is a programming error, not user YAML.
    if let Some(target) = step.resume.as_deref() {
        let session_id = state.agent_session_ids.get(target).ok_or_else(|| {
            BuildArgvError::ResumeTargetNotFound {
                step: step.name.clone(),
                target: target.into(),
            }
        })?;
        args.push("--resume".into());
        args.push(session_id.clone());
        return Ok(args);
    }
    if let Some(target) = step.fork_session.as_deref() {
        let session_id = state.agent_session_ids.get(target).ok_or_else(|| {
            BuildArgvError::ForkSessionTargetNotFound {
                step: step.name.clone(),
                target: target.into(),
            }
        })?;
        // v2 behavior change: explicit fork_session emits both flags.
        // See module docs (§ Session-resolution rules, note 1) for the
        // rationale and the pinned test.
        args.push("--fork-session".into());
        args.push("--resume".into());
        args.push(session_id.clone());
        return Ok(args);
    }

    // No explicit target → workflow-level session default. `None` and
    // `Some(Shared)` are equivalent; `Some(Isolated)` opts out entirely.
    let isolated = matches!(step.agent_session, Some(AgentSessionMode::Isolated));
    if !isolated {
        if let Some(first) = state.first_agent_session_id {
            args.push("--fork-session".into());
            args.push("--resume".into());
            args.push(first.into());
        }
    }

    Ok(args)
}

/// Failure modes for [`run_agent_step`]. Variants split by the stage of
/// the spawn-and-parse pipeline they surface from so downstream event
/// emission and tests can discriminate without string-matching.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AgentExecError {
    /// Argv construction rejected the step — propagated verbatim so the
    /// adapter-layer distinction between `resume`/`fork_session` typos
    /// survives into terminal events.
    #[error(transparent)]
    BuildArgv(#[from] BuildArgvError),

    /// `Command::spawn` failed. Almost always a missing/unexecutable
    /// binary (wrong `agent_command:`, PATH miss, mock not marked +x).
    #[error("failed to spawn agent binary `{command}`: {error}")]
    Spawn { command: String, error: String },

    /// Writing the rendered prompt to the child's stdin failed. Most
    /// common cause is EPIPE when the child exits before draining stdin
    /// (v1 surfaced this as a generic `StepError::Fail`; v2 keeps it
    /// distinct so the integration suite can pin the race explicitly —
    /// see task #51 for the test harness hardening).
    #[error("failed to write prompt to agent stdin: {error}")]
    StdinWrite { error: String },

    /// `child.wait()` failed after the parse loop resolved. Distinct
    /// from [`Self::Spawn`] so operators can tell "never started" from
    /// "started but lost the handle".
    #[error("failed to wait for agent process: {error}")]
    WaitFailed { error: String },

    /// Child exited non-zero AND produced no parseable `result` event.
    /// v1 parity (`src/steps/agent.rs:149`): non-zero *with* a captured
    /// response is still SUCCESS because the Claude CLI sometimes exits
    /// non-zero while still emitting a valid `result` event (tool_use
    /// failure with a fallback response being the common case).
    #[error(
        "agent process exited with status {code} and no response was \
         captured on stdout"
    )]
    NonZeroExit { code: i32 },
}

/// Runtime output of [`run_agent_step`]. Every numeric field is
/// `Option<_>` because the CLI only emits them inside a `"result"`
/// stream-json event, and a truncated / misbehaving child may never
/// emit one — absence is a fact worth preserving instead of papering
/// over with zeros.
#[derive(Debug, Default)]
pub(crate) struct AgentExecOutput {
    /// Final assistant response, keyed on the `result.result` field of
    /// the last `"result"` stream-json event. Becomes
    /// `StepOutputSnapshot.stdout` on the terminal event.
    pub response: String,
    /// `result.session_id`, if the CLI reported one. Lifted to
    /// `Event::StepCompleted.agent_session_id` so replay can reconstruct
    /// the session-id map without widening `StepOutputSnapshot`.
    pub session_id: Option<String>,
    /// `result.usage.input_tokens`. Lifted to the event level per
    /// Option B of PR 5a.
    pub input_tokens: Option<u64>,
    /// `result.usage.output_tokens`.
    pub output_tokens: Option<u64>,
    /// `result.cost_usd`.
    pub cost_usd: Option<f64>,
    /// True when the workflow-level completion signal matched a raw stdout
    /// line. The engine turns this into `Event::CompletionSignaled` and a
    /// successful workflow terminal state.
    pub completion_signaled: bool,
}

/// Parse one `stream-json` line from the Claude CLI into [`AgentExecOutput`].
///
/// v1 parity with `src/steps/agent.rs:63-97`. Behavior:
///
/// * Malformed JSON → silently ignored (matches v1 `if let Ok(..)`).
///   Rationale: the CLI will sometimes emit blank lines or pre-auth
///   handshake noise; surfacing those as errors would flake every run.
/// * `"result"` events update the accumulator fields (last-wins: the CLI
///   may emit multiple results for multi-turn plans and we want the
///   final one, matching v1).
/// * `"assistant"` / `"tool_use"` display events are dropped. v1 forwarded
///   them to `cli::display` for human-readable progress; the harness is
///   headless, so they don't belong here.
/// * All other `"type"` values → ignored (forward-compatible with new
///   event kinds the CLI might add).
fn parse_stream_json_line(line: &str, out: &mut AgentExecOutput) {
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    if msg.get("type").and_then(|t| t.as_str()) != Some("result") {
        return;
    }
    if let Some(r) = msg.get("result").and_then(|r| r.as_str()) {
        out.response = r.to_string();
    }
    if let Some(sid) = msg.get("session_id").and_then(|s| s.as_str()) {
        out.session_id = Some(sid.to_string());
    }
    if let Some(usage) = msg.get("usage") {
        if let Some(v) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
            out.input_tokens = Some(v);
        }
        if let Some(v) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
            out.output_tokens = Some(v);
        }
    }
    if let Some(cost) = msg.get("cost_usd").and_then(|c| c.as_f64()) {
        out.cost_usd = Some(cost);
    }
}

/// Spawn the Claude CLI, pipe the rendered prompt to its stdin, parse
/// the stream-json response from stdout, and return structured output.
///
/// # Binary resolution
///
/// The binary path is `step.agent_command.as_deref().unwrap_or("claude")`
/// — v1 parity with `config.get_str("command")` in
/// `src/steps/agent.rs:24`. `Command::new` inherits `PATH` from the
/// parent so bare `"claude"` resolves through PATH.
///
/// # Cancellation contract
///
/// This function does **not** observe `step.timeout`, the cancel token,
/// or the shutdown broadcast. Those belong to the engine's step-level
/// `tokio::select!` (see [`crate::engine::Engine::run_agent_step`]), so
/// agent lines up with cmd's cancel/signal/timeout model (PR 3 of #31)
/// without duplicating the race here.
///
/// The child is spawned with `.kill_on_drop(true)` so that when the
/// outer `select!` drops this future mid-wait (cancel/signal/timeout
/// branch fired first), Tokio sends `SIGKILL` to the Claude CLI on
/// `Child::drop`. Without this flag Tokio defaults to leak-on-drop and
/// a SIGTERM during a long CLI turn would leave the child running past
/// the session's terminal event.
///
/// Issue #59: `kill_on_drop(true)` only signals the leader. Claude
/// itself spawns helper processes (MCP servers, tool subprocesses) and
/// once the leader dies they reparent to PID 1 and survive. To stop
/// that, the child is also placed in **its own process group** via
/// `process_group(0)`, and a [`ProcessGroupKillOnDrop`] guard is held
/// alongside the child. Reverse-declaration drop order means the guard
/// fires first (`kill(-pgid, SIGKILL)` reaches every descendant in the
/// group), then Tokio's `Child::Drop` sends `SIGKILL` to the leader
/// (idempotent if `killpg` already reaped it).
///
/// # Exit-code rule (v1 parity)
///
/// ```text
/// status.success() && response.is_empty()  → OK (empty response) — v1 parity
/// status.success() && !response.is_empty() → OK (happy path)
/// !status.success() && response.is_empty() → NonZeroExit — hard failure
/// !status.success() && !response.is_empty() → OK — tool_use failed mid-turn
/// ```
///
/// The last row is the one that surprises reviewers: the CLI occasionally
/// exits non-zero while still producing a valid `result` event (tool-use
/// failure, network hiccup mid-stream). v1 treated that as SUCCESS so
/// workflows could salvage the partial response; v2 keeps the rule
/// verbatim.
pub(crate) async fn run_agent_step(
    step: &Step,
    rendered_prompt: &str,
    state: &AgentSessionState<'_>,
    env: &HashMap<String, String>,
    completion_signal: Option<&str>,
) -> Result<AgentExecOutput, AgentExecError> {
    let argv = build_agent_argv(step, state)?;
    let command = step.agent_command.as_deref().unwrap_or("claude");

    let mut command_builder = Command::new(command);
    command_builder
        .args(&argv)
        .envs(env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // Discard stderr: we never read it, and a piped-but-unread stderr
        // deadlocks the child once the pipe buffer fills (~64KB on Linux).
        // v1 left this as piped and accepted the latent hang; v2 drops it.
        .stderr(std::process::Stdio::null())
        // SIGKILL the child if this future is dropped mid-wait. The
        // engine's outer `select!` drops us on cancel/signal/timeout;
        // without this the child would leak past the session's terminal
        // event (Tokio's default is leak-on-drop for spawned processes).
        .kill_on_drop(true);
    // Issue #59: place the child in its own process group so the RAII
    // guard below can `kill(-pgid, SIGKILL)` the leader plus every
    // descendant Claude forked. cfg(unix) gates this because Tokio's
    // `Command::process_group` is only compiled on Unix targets.
    #[cfg(unix)]
    command_builder.process_group(0);
    let mut child = command_builder.spawn().map_err(|e| AgentExecError::Spawn {
        command: command.to_string(),
        error: e.to_string(),
    })?;

    // Issue #59: build the kill-group guard immediately after spawn so
    // the guard's lifetime brackets the same scope as `child`. Reverse
    // declaration order = reverse drop order: when this future is
    // dropped (cancel/signal/timeout race), `_kill_group` drops first
    // and SIGKILLs the whole process group, then Tokio's `Child::Drop`
    // SIGKILLs the leader directly (idempotent if already reaped).
    //
    // `child.id()` is `Some` immediately after `spawn()` returns Ok —
    // it only goes `None` after `wait()` reaps the child. We capture it
    // here, before any `await`, so the unwrap is sound.
    #[cfg(unix)]
    let _kill_group = {
        let pid = child
            .id()
            .expect("Child::id is Some between spawn and wait") as i32;
        crate::process_group::ProcessGroupKillOnDrop::new(pid)
    };

    // Feed the rendered prompt to the child and close the write half so
    // the child sees EOF. Dropping `stdin` is required — without it the
    // child blocks on its own `read` forever and the parse loop hangs.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(rendered_prompt.as_bytes())
            .await
            .map_err(|e| AgentExecError::StdinWrite {
                error: e.to_string(),
            })?;
        drop(stdin);
    }

    // `stdout` was requested as piped on spawn, so the handle is always
    // present. A `None` here would indicate the Tokio runtime itself is
    // misbehaving, which is unrecoverable.
    let stdout = child
        .stdout
        .take()
        .expect("child was spawned with stdout piped");
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    let mut output = AgentExecOutput::default();
    while let Ok(Some(line)) = lines.next_line().await {
        parse_stream_json_line(&line, &mut output);
        if completion_signal
            .map(|signal| line.contains(signal))
            .unwrap_or(false)
        {
            output.completion_signaled = true;
            return Ok(output);
        }
    }

    let status = child.wait().await.map_err(|e| AgentExecError::WaitFailed {
        error: e.to_string(),
    })?;

    if !status.success() && output.response.is_empty() {
        return Err(AgentExecError::NonZeroExit {
            code: status.code().unwrap_or(-1),
        });
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::Step;

    fn empty_state() -> (HashMap<String, String>, Option<String>) {
        (HashMap::new(), None)
    }

    fn state<'a>(
        ids: &'a HashMap<String, String>,
        first: Option<&'a String>,
    ) -> AgentSessionState<'a> {
        AgentSessionState {
            agent_session_ids: ids,
            first_agent_session_id: first.map(String::as_str),
        }
    }

    fn base_args() -> Vec<String> {
        vec![
            "-p".into(),
            "--verbose".into(),
            "--output-format".into(),
            "stream-json".into(),
        ]
    }

    #[test]
    fn minimal_agent_emits_only_base_stream_flags() {
        let step = Step::agent("ask", "Hi");
        let (ids, first) = empty_state();
        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();
        assert_eq!(argv, base_args());
    }

    #[test]
    fn model_system_prompt_and_skip_permissions_are_threaded_through() {
        let mut step = Step::agent("ask", "Hi");
        step.model = Some("claude-sonnet-4-6".into());
        step.system_prompt_append = Some("Be concise.".into());
        step.permissions = Some(AgentPermissions::Skip);
        let (ids, first) = empty_state();

        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();

        let mut expected = base_args();
        expected.extend([
            "--model".into(),
            "claude-sonnet-4-6".into(),
            "--append-system-prompt".into(),
            "Be concise.".into(),
            "--dangerously-skip-permissions".into(),
        ]);
        assert_eq!(argv, expected);
    }

    #[test]
    fn default_permissions_variant_does_not_emit_skip_flag() {
        let mut step = Step::agent("ask", "Hi");
        step.permissions = Some(AgentPermissions::Default);
        let (ids, first) = empty_state();
        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();
        assert!(
            !argv.iter().any(|a| a == "--dangerously-skip-permissions"),
            "permissions: default must not emit skip flag — argv was {argv:?}"
        );
    }

    #[test]
    fn explicit_resume_emits_resume_only() {
        let mut step = Step::agent("refine", "Continue");
        step.resume = Some("plan".into());

        let mut ids = HashMap::new();
        ids.insert("plan".into(), "sid-plan".into());
        let first = Some("sid-plan".to_string());

        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();

        let mut expected = base_args();
        expected.extend(["--resume".into(), "sid-plan".into()]);
        assert_eq!(
            argv, expected,
            "explicit resume must NOT emit --fork-session"
        );
    }

    /// Reviewer-requested pin (PR 5a commit 3b): the v2 semantic fix for
    /// explicit `fork_session:` is to emit `--fork-session --resume <id>`
    /// rather than the v1 bare `--resume <id>`. If this test ever changes
    /// shape, it must do so deliberately (with a release note), not
    /// silently.
    #[test]
    fn explicit_fork_session_emits_fork_session_and_resume() {
        let mut step = Step::agent("branch", "Try alternative");
        step.fork_session = Some("plan".into());

        let mut ids = HashMap::new();
        ids.insert("plan".into(), "sid-plan".into());
        let first = Some("sid-plan".to_string());

        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();

        let mut expected = base_args();
        expected.extend([
            "--fork-session".into(),
            "--resume".into(),
            "sid-plan".into(),
        ]);
        assert_eq!(
            argv, expected,
            "v2 explicit fork_session must emit --fork-session --resume (semantic fix over v1's bare --resume)"
        );
    }

    #[test]
    fn explicit_resume_precludes_workflow_session_args() {
        // Belt-and-suspenders: even when a shared-session first_id is
        // captured, an explicit `resume:` overrides it — the resume arm
        // must early-return so we never leak a second `--resume`.
        let mut step = Step::agent("refine", "Continue");
        step.resume = Some("plan".into());
        step.agent_session = Some(AgentSessionMode::Shared);

        let mut ids = HashMap::new();
        ids.insert("plan".into(), "sid-plan".into());
        ids.insert("earlier".into(), "sid-earlier".into());
        let first = Some("sid-earlier".to_string());

        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();

        let mut expected = base_args();
        expected.extend(["--resume".into(), "sid-plan".into()]);
        assert_eq!(argv, expected);
        assert_eq!(
            argv.iter().filter(|a| *a == "--resume").count(),
            1,
            "explicit resume must not double-emit alongside workflow session"
        );
        assert!(
            !argv.iter().any(|a| a == "--fork-session"),
            "explicit resume must not emit --fork-session from the shared path"
        );
    }

    #[test]
    fn shared_default_with_capture_emits_fork_session_and_resume_for_first() {
        // Workflow-level `session: shared` (or the absent default) +
        // a prior captured session_id = `--fork-session --resume <first>`.
        // v1 parity with `src/engine/context.rs:103-108`.
        let step = Step::agent("later", "Continue");

        let mut ids = HashMap::new();
        ids.insert("earlier".into(), "sid-earlier".into());
        let first = Some("sid-earlier".to_string());

        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();

        let mut expected = base_args();
        expected.extend([
            "--fork-session".into(),
            "--resume".into(),
            "sid-earlier".into(),
        ]);
        assert_eq!(argv, expected);
    }

    #[test]
    fn shared_default_without_capture_emits_no_session_args() {
        // First agent step of the workflow — nothing captured yet, so
        // the CLI just runs fresh.
        let step = Step::agent("first", "Hi");
        let (ids, first) = empty_state();
        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();
        assert_eq!(
            argv,
            base_args(),
            "no capture yet must emit no session args"
        );
    }

    #[test]
    fn isolated_emits_no_session_args_even_with_capture() {
        let mut step = Step::agent("fresh", "Hi");
        step.agent_session = Some(AgentSessionMode::Isolated);

        let mut ids = HashMap::new();
        ids.insert("earlier".into(), "sid-earlier".into());
        let first = Some("sid-earlier".to_string());

        let argv = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap();
        assert_eq!(
            argv,
            base_args(),
            "isolated session must opt out of all --resume threading"
        );
    }

    #[test]
    fn missing_resume_target_returns_structured_error() {
        let mut step = Step::agent("refine", "Continue");
        step.resume = Some("typo".into());
        let (ids, first) = empty_state();

        let err = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap_err();
        match err {
            BuildArgvError::ResumeTargetNotFound { step, target } => {
                assert_eq!(step, "refine");
                assert_eq!(target, "typo");
            }
            other => panic!("expected ResumeTargetNotFound, got {other:?}"),
        }
    }

    #[test]
    fn missing_fork_session_target_returns_structured_error() {
        let mut step = Step::agent("branch", "Try");
        step.fork_session = Some("typo".into());
        let (ids, first) = empty_state();

        let err = build_agent_argv(&step, &state(&ids, first.as_ref())).unwrap_err();
        match err {
            BuildArgvError::ForkSessionTargetNotFound { step, target } => {
                assert_eq!(step, "branch");
                assert_eq!(target, "typo");
            }
            other => panic!("expected ForkSessionTargetNotFound, got {other:?}"),
        }
    }

    // Unit-level coverage for `parse_stream_json_line`. The end-to-end
    // spawn/write/parse path is covered by `tests/agent_replay.rs` in
    // the next commit of PR 5a; here we pin the JSON shape the harness
    // accepts from the Claude CLI so a CLI-side schema drift becomes
    // a compile-time-adjacent failure rather than a mysterious runtime
    // regression.

    #[test]
    fn parses_result_event_populates_all_fields() {
        let line = r#"{"type":"result","result":"hi","session_id":"sid-1","usage":{"input_tokens":11,"output_tokens":22},"cost_usd":0.0042}"#;
        let mut out = AgentExecOutput::default();
        parse_stream_json_line(line, &mut out);
        assert_eq!(out.response, "hi");
        assert_eq!(out.session_id.as_deref(), Some("sid-1"));
        assert_eq!(out.input_tokens, Some(11));
        assert_eq!(out.output_tokens, Some(22));
        assert_eq!(out.cost_usd, Some(0.0042));
    }

    #[test]
    fn non_result_event_types_are_ignored() {
        // `assistant` and `tool_use` are display-only events that v1
        // forwarded to the CLI's progress renderer; the harness MUST
        // drop them and leave the accumulator untouched.
        let mut out = AgentExecOutput::default();
        parse_stream_json_line(
            r#"{"type":"assistant","content":"Processing..."}"#,
            &mut out,
        );
        parse_stream_json_line(r#"{"type":"tool_use","tool":"Read"}"#, &mut out);
        assert!(out.response.is_empty());
        assert!(out.session_id.is_none());
        assert!(out.input_tokens.is_none());
    }

    #[test]
    fn malformed_json_lines_are_ignored() {
        let mut out = AgentExecOutput::default();
        parse_stream_json_line("", &mut out);
        parse_stream_json_line("not json at all", &mut out);
        parse_stream_json_line(r#"{"partial":"#, &mut out);
        assert!(out.response.is_empty());
        assert!(out.session_id.is_none());
    }

    #[test]
    fn last_result_event_wins_on_response_and_session_id() {
        // Multi-turn plans emit multiple result events; the v1 parser
        // kept overwriting the accumulator so the final turn wins.
        let mut out = AgentExecOutput::default();
        parse_stream_json_line(
            r#"{"type":"result","result":"first","session_id":"sid-a","usage":{"input_tokens":1,"output_tokens":2},"cost_usd":0.01}"#,
            &mut out,
        );
        parse_stream_json_line(
            r#"{"type":"result","result":"second","session_id":"sid-b","usage":{"input_tokens":3,"output_tokens":4},"cost_usd":0.02}"#,
            &mut out,
        );
        assert_eq!(out.response, "second");
        assert_eq!(out.session_id.as_deref(), Some("sid-b"));
        assert_eq!(out.input_tokens, Some(3));
        assert_eq!(out.output_tokens, Some(4));
        assert_eq!(out.cost_usd, Some(0.02));
    }

    #[test]
    fn result_event_without_usage_leaves_tokens_as_none() {
        // Token fields are Options — a CLI that omits `usage` must
        // leave them as None rather than defaulting to 0, otherwise
        // "unknown" and "zero tokens" collapse into one representation.
        let mut out = AgentExecOutput::default();
        parse_stream_json_line(
            r#"{"type":"result","result":"hi","session_id":"sid-1"}"#,
            &mut out,
        );
        assert_eq!(out.response, "hi");
        assert_eq!(out.session_id.as_deref(), Some("sid-1"));
        assert!(out.input_tokens.is_none());
        assert!(out.output_tokens.is_none());
        assert!(out.cost_usd.is_none());
    }

    #[test]
    fn result_event_without_session_id_leaves_session_as_none() {
        // AC bullet: "agent_session_id ausente não falha se o CLI não
        // retorna id, mas também não captura sessão." If the CLI answers
        // without a session_id (early exit, older CLI build, custom
        // wrapper), `parse_stream_json_line` must leave `out.session_id`
        // as None so the terminal event records "no capture" rather than
        // defaulting to an empty string — the log scan treats absence
        // and empty string identically, but downstream operators can't.
        let mut out = AgentExecOutput::default();
        parse_stream_json_line(
            r#"{"type":"result","result":"hi","usage":{"input_tokens":1,"output_tokens":2},"cost_usd":0.01}"#,
            &mut out,
        );
        assert_eq!(out.response, "hi");
        assert!(out.session_id.is_none());
        assert_eq!(out.input_tokens, Some(1));
        assert_eq!(out.output_tokens, Some(2));
    }
}
