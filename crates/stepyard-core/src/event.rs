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
}
