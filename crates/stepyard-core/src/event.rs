//! The lifecycle [`Event`] enum emitted by the engine and consumed by every
//! [`EventSubscriber`](crate::EventSubscriber).
//!
//! # Forward-compatibility (NFC6)
//!
//! `Event` is `#[non_exhaustive]` and serializes via the `event` discriminator
//! tag in `snake_case`. Subscribers using `serde(other)` on their consumer
//! side can ignore unknown variants instead of failing — this is the contract
//! that keeps the Dashboard, Slack and webhook subscribers working when
//! the engine ships new variants.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Every observable lifecycle event in the engine.
///
/// Variants are stable. New variants may be added in minor versions.
/// Existing variant fields may gain `Option<T>` additions but never lose
/// fields without a major bump.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A workflow execution started. Always the first event in a session.
    WorkflowStarted {
        timestamp: DateTime<Utc>,
    },
    /// A workflow execution finished successfully (status = `completed`).
    WorkflowCompleted {
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    /// A step started executing.
    StepStarted {
        step_name: String,
        step_type: String,
        timestamp: DateTime<Utc>,
        /// Non-`None` when the step runs inside a container scope
        /// (`call` / `repeat` / `map` — PR 3 of Task #31). Absent for
        /// top-level steps and for legacy log entries written before
        /// the widening, so existing JSON deserializes unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope_context: Option<ScopeContext>,
    },
    /// A step finished successfully.
    StepCompleted {
        step_name: String,
        step_type: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
        #[serde(skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
        sandboxed: bool,
        /// Output snapshot persisted to the session log so cross-step template
        /// references (`{{ steps.X.stdout }}` etc.) survive a process crash and
        /// reload — the harness rebuilds its output map from the log in
        /// `progress_from_log` rather than holding in-memory state (PR 2 of
        /// Task #31). `None` for non-cmd step kinds that don't produce exec
        /// output (e.g. `gate`). Redaction / size caps are deliberately out of
        /// this PR's scope.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<StepOutputSnapshot>,
        /// Non-`None` when the step ran inside a container scope
        /// (`call` / `repeat` / `map` — PR 3 of Task #31). Mirrors
        /// [`Self::StepStarted`]; absent for top-level steps.
        ///
        /// The harness's `progress_from_log` counts only completions
        /// with `scope_context: None` toward the top-level step index
        /// — scoped completions feed container-internal replay state.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope_context: Option<ScopeContext>,
        /// Which branch a `gate` step's `condition` resolved to.
        /// Persisting the decision in the log (rather than re-evaluating
        /// the `condition` during replay) keeps the log as the single
        /// source of truth for scope control flow. `None` for non-gate
        /// steps. Gate failures route through [`Self::StepFailed`]
        /// instead, so `fail` is not a valid `gate_outcome` value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gate_outcome: Option<GateOutcome>,
        /// Claude CLI `session_id` captured from an `agent` step's
        /// streaming JSON — persisted so a follow-up agent step with
        /// `session: shared` / `isolated` can derive its `--resume` /
        /// `--fork-session` argv from the session log alone (PR 5a of
        /// Task #31). `None` for every other step kind and for every
        /// log entry written before PR 5a, so existing JSON still
        /// deserializes unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_session_id: Option<String>,
    },
    /// A step finished with an error.
    StepFailed {
        step_name: String,
        step_type: String,
        error: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
        sandboxed: bool,
    },
    /// A Docker sandbox container was created for this session.
    SandboxCreated {
        sandbox_id: String,
        timestamp: DateTime<Utc>,
    },
    /// A Docker sandbox container was destroyed.
    SandboxDestroyed {
        sandbox_id: String,
        timestamp: DateTime<Utc>,
    },
    /// A step hit its configured wall-clock timeout. Emitted immediately
    /// before the engine tears the sandbox down (Story 1.4 emit-before-IO
    /// ordering). `timestamp` is intentionally absent — D5's
    /// timeout-fired family carries only the structural facts the engine
    /// knows at firing time; wall-clock attribution happens via the
    /// surrounding `StepFailed` event (Story 1.4) or the session log.
    StepTimeoutFired {
        step_index: u32,
        configured_ms: u64,
    },
    /// A SIGINT / SIGTERM (or startup reconcile — Story 2.4) interrupted
    /// the engine. Emitted synchronously **before** the sandbox is destroyed
    /// (D5 emit-before-IO). `signal` is lowercase snake_case: `"sigint"`,
    /// `"sigterm"`, or `"crash_recovery"`.
    SignalReceived {
        signal: String,
    },
    /// One turn of an `agent` / `chat` step's conversation was persisted
    /// to the session log. Emitted once per role turn so a post-crash
    /// replay can reconstruct the full `chat_sessions` map from the log
    /// alone (PR 5b of Task #31 — Invariante 11 means the runtime has no
    /// in-memory chat history between `step` calls).
    ///
    /// `session` is the chat-session bucket key — not necessarily the
    /// step name. Steps running under `session: shared` all emit with
    /// the same `session`; steps under `session: isolated` emit with
    /// their own step name. Replay groups turns by `session` and
    /// preserves log append order inside each bucket.
    ///
    /// `role` is the strongly-typed [`ChatRole`] — serde will reject
    /// any value outside `user` / `assistant` at deserialize time, so a
    /// corrupted log entry surfaces as a scan error rather than a
    /// silent drop.
    ChatMessageAppended {
        step_name: String,
        session: String,
        role: ChatRole,
        content: String,
        timestamp: DateTime<Utc>,
    },
}

/// Frozen exec output attached to [`Event::StepCompleted`] so the harness can
/// rebuild a cross-step reference map from the session log alone (no in-memory
/// state between `step` calls — preserves Invariante 11 under post-crash
/// replay). Added in PR 2 of Task #31.
///
/// Fields are persisted verbatim. Redaction and size caps are deferred to a
/// later PR; the harness writes whatever the executor produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepOutputSnapshot {
    /// Standard output of the step, verbatim.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    /// Standard error of the step, verbatim.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    /// Process exit code (0 on success for cmd steps).
    pub exit_code: i32,
}

/// Position of a step inside a container scope (`call` / `repeat` / `map`).
/// Attached to [`Event::StepStarted`] and [`Event::StepCompleted`] so the
/// harness can rebuild container-internal state from the session log
/// alone (PR 3 of Task #31).
///
/// Nested containers are rejected at the adapter layer in PR 3, so a flat
/// `{ container, iteration, position }` is sufficient. A later PR that
/// lifts that restriction can add a `scope_path: Vec<ScopeFrame>` field
/// without breaking this shape (absent = legacy top-level scope frame).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeContext {
    /// Name of the container step (e.g. the `call` / `repeat` / `map`
    /// step that wraps this scope body).
    pub container: String,
    /// Zero-based iteration index within the container.
    /// `0` for `call` (single pass), `0..N` for `repeat` / `map`.
    pub iteration: u32,
    /// Zero-based position of this step within the scope body, counting
    /// in declaration order regardless of whether earlier scope steps
    /// were skipped. Lets replay locate the current step inside the
    /// scope without having to replay gate decisions.
    pub position: u32,
}

/// Which branch of a `gate` step's `condition` fired. Persisted on the
/// gate's [`Event::StepCompleted`] so replay never has to re-evaluate
/// the condition to figure out the scope's next step — the log is the
/// single source of truth for control flow.
///
/// Gate failures route through [`Event::StepFailed`], so `Fail` is
/// deliberately not a variant. `#[non_exhaustive]` lets a later PR add
/// variants (e.g. scope-aware short-circuits) without a major bump.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    /// Execution continues with the next scope step (or the next
    /// top-level step for a top-level gate).
    Continue,
    /// End the current scope iteration early; the containing
    /// `repeat` / `map` advances to the next iteration, and `call`
    /// completes the scope body. Rejected at the adapter boundary for
    /// top-level gates (PR 3 of Task #31).
    Skip,
    /// End the containing scope entirely. `repeat` / `map` exit the
    /// loop and the container step completes successfully; `call`
    /// treats this as end-of-scope (v1 parity). Rejected at the
    /// adapter boundary for top-level gates.
    Break,
}

/// Role of a turn inside a chat session. Serialized as `snake_case`
/// strings (`user`, `assistant`) so unknown roles (`system`, `tool_use`,
/// ...) fail loud at deserialize time instead of being silently coerced
/// to a default. PR 5b of Task #31 ships only the two v1 roles; a
/// follow-up PR that surfaces system prompts can add a variant without a
/// major bump thanks to `#[non_exhaustive]`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatRole {
    /// A user-authored turn (the rendered prompt template, plus any
    /// prior user turns if the workflow is running a multi-turn chat).
    User,
    /// An assistant-authored turn (the provider's completion response).
    Assistant,
}

/// One turn of chat history, reconstructed from the session log by the
/// harness so a `session: shared` chat step landing after a crash sees
/// the same conversation the pre-crash run did (PR 5b of Task #31).
///
/// The wire type mirrors v1's `ChatMessage` except `role` is typed —
/// serde rejects values outside [`ChatRole`] at deserialize time, which
/// is the replay gate for "a corrupted log row must fail the scan, not
/// silently change the prompt the next turn sees."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_timeout_fired_serializes_with_event_tag_and_snake_case_discriminator() {
        let event = Event::StepTimeoutFired {
            step_index: 2,
            configured_ms: 300_000,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "event": "step_timeout_fired",
                "step_index": 2,
                "configured_ms": 300_000,
            })
        );
    }

    #[test]
    fn step_timeout_fired_roundtrips_through_json() {
        let original = Event::StepTimeoutFired {
            step_index: 7,
            configured_ms: 5_000,
        };
        let s = serde_json::to_string(&original).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        match back {
            Event::StepTimeoutFired {
                step_index,
                configured_ms,
            } => {
                assert_eq!(step_index, 7);
                assert_eq!(configured_ms, 5_000);
            }
            other => panic!("roundtrip produced unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn signal_received_serializes_with_event_tag_and_snake_case_discriminator() {
        let event = Event::SignalReceived {
            signal: "sigterm".into(),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "event": "signal_received",
                "signal": "sigterm",
            })
        );
    }

    #[test]
    fn signal_received_roundtrips_through_json() {
        let original = Event::SignalReceived {
            signal: "sigint".into(),
        };
        let s = serde_json::to_string(&original).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        match back {
            Event::SignalReceived { signal } => {
                assert_eq!(signal, "sigint");
            }
            other => panic!("roundtrip produced unexpected variant: {other:?}"),
        }
    }

    fn chat_timestamp() -> DateTime<Utc> {
        "2026-04-22T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn chat_message_appended_serializes_with_event_tag_and_snake_case_role() {
        let event = Event::ChatMessageAppended {
            step_name: "draft".into(),
            session: "shared".into(),
            role: ChatRole::User,
            content: "hello".into(),
            timestamp: chat_timestamp(),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "event": "chat_message_appended",
                "step_name": "draft",
                "session": "shared",
                "role": "user",
                "content": "hello",
                "timestamp": "2026-04-22T12:00:00Z",
            })
        );
    }

    #[test]
    fn chat_message_appended_roundtrips_through_json() {
        let original = Event::ChatMessageAppended {
            step_name: "review".into(),
            session: "review".into(),
            role: ChatRole::Assistant,
            content: "ack".into(),
            timestamp: chat_timestamp(),
        };
        let s = serde_json::to_string(&original).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        match back {
            Event::ChatMessageAppended {
                step_name,
                session,
                role,
                content,
                timestamp,
            } => {
                assert_eq!(step_name, "review");
                assert_eq!(session, "review");
                assert_eq!(role, ChatRole::Assistant);
                assert_eq!(content, "ack");
                assert_eq!(timestamp, chat_timestamp());
            }
            other => panic!("roundtrip produced unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn chat_role_unknown_variants_fail_to_deserialize() {
        // The replay gate: a log row carrying `role: "system"` (or any
        // other value outside user/assistant) must error at the serde
        // layer so `compute_progress` surfaces it as `InvalidState`
        // instead of silently coercing the turn into a default role.
        let payload = serde_json::json!({
            "event": "chat_message_appended",
            "step_name": "s",
            "session": "s",
            "role": "system",
            "content": "",
            "timestamp": "2026-04-22T12:00:00Z",
        });
        let err = serde_json::from_value::<Event>(payload)
            .expect_err("unknown role must fail to deserialize");
        let msg = err.to_string();
        assert!(
            msg.contains("role") || msg.contains("variant"),
            "error must reference the role field, got: {msg}"
        );
    }
}
