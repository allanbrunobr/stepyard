//! Contract tests for the public surface of `stepyard-core`. These freeze the
//! externally visible behavior:
//!
//! * Event JSON discriminator and field names are stable (Story 2.1 AC).
//! * Subscribers can ignore unknown variants via `#[serde(other)]` (NFC6).
//! * EngineError exposes no `anyhow::Error` in its public surface.
//! * EventSubscriber is dyn-compatible.

use chrono::TimeZone;
use stepyard_core::{
    ChatRole, EngineError, Event, EventSubscriber, GateOutcome, ScopeContext, StepOutputSnapshot,
    TerminationReason,
};
use serde::Deserialize;
use serde_json::json;

#[test]
fn step_started_serialization_is_stable() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap();
    let event = Event::StepStarted {
        step_name: "review".into(),
        step_type: "agent".into(),
        timestamp: ts,
        scope_context: None,
    };
    let value: serde_json::Value = serde_json::to_value(&event).unwrap();
    assert_eq!(
        value,
        json!({
            "event": "step_started",
            "step_name": "review",
            "step_type": "agent",
            "timestamp": "2026-04-13T12:00:00Z"
        })
    );
    // The `scope_context` field added in PR 3 of Task #31 must stay
    // absent on the wire when `None`, so pre-PR-3 log entries and
    // subscribers keep working unchanged.
    assert!(
        !value.as_object().unwrap().contains_key("scope_context"),
        "step_started with scope_context=None must omit the field"
    );
}

#[test]
fn step_completed_omits_optional_fields_when_none() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 1).unwrap();
    let event = Event::StepCompleted {
        step_name: "x".into(),
        step_type: "cmd".into(),
        duration_ms: 42,
        timestamp: ts,
        input_tokens: None,
        output_tokens: None,
        cost_usd: None,
        sandboxed: false,
        output: None,
        scope_context: None,
        gate_outcome: None,
        agent_session_id: None,
    };
    let value: serde_json::Value = serde_json::to_value(&event).unwrap();
    let obj = value.as_object().unwrap();
    assert!(!obj.contains_key("input_tokens"));
    assert!(!obj.contains_key("output_tokens"));
    assert!(!obj.contains_key("cost_usd"));
    // Backward-compat: the `output` field added in PR 2 of Task #31 must stay
    // absent on the wire when None, so old subscribers keep deserializing the
    // same JSON shape they've always seen.
    assert!(!obj.contains_key("output"));
    // Same contract for the PR 3 widenings — both scope_context and
    // gate_outcome must stay absent on the wire when None.
    assert!(!obj.contains_key("scope_context"));
    assert!(!obj.contains_key("gate_outcome"));
    // PR 5a of Task #31 adds `agent_session_id` — same omit-when-None
    // contract so pre-PR-5a subscribers continue to see identical JSON.
    assert!(!obj.contains_key("agent_session_id"));
    assert_eq!(obj["sandboxed"], json!(false));
}

#[test]
fn step_completed_with_output_snapshot_roundtrips() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 2).unwrap();
    let original = Event::StepCompleted {
        step_name: "build".into(),
        step_type: "cmd".into(),
        duration_ms: 17,
        timestamp: ts,
        input_tokens: None,
        output_tokens: None,
        cost_usd: None,
        sandboxed: true,
        output: Some(StepOutputSnapshot {
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_code: 0,
        }),
        scope_context: None,
        gate_outcome: None,
        agent_session_id: None,
    };

    let value = serde_json::to_value(&original).unwrap();
    let obj = value.as_object().unwrap();
    let snapshot = obj.get("output").expect("output must be present when Some");
    assert_eq!(snapshot["stdout"], json!("hello\n"));
    assert_eq!(snapshot["exit_code"], json!(0));
    // Empty stderr is skipped by the per-field `skip_serializing_if` to keep
    // logged JSON compact — still valid input for the roundtrip below.
    assert!(!snapshot.as_object().unwrap().contains_key("stderr"));

    let s = serde_json::to_string(&original).unwrap();
    let back: Event = serde_json::from_str(&s).unwrap();
    match back {
        Event::StepCompleted {
            output: Some(snap),
            ..
        } => {
            assert_eq!(snap.stdout, "hello\n");
            assert_eq!(snap.stderr, "");
            assert_eq!(snap.exit_code, 0);
        }
        other => panic!("roundtrip produced unexpected variant: {other:?}"),
    }
}

// ── PR 3 of #31 — scope_context + gate_outcome wire-shape locks ─────────

#[test]
fn step_started_with_scope_context_roundtrips() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 20, 12, 0, 0).unwrap();
    let original = Event::StepStarted {
        step_name: "inner".into(),
        step_type: "cmd".into(),
        timestamp: ts,
        scope_context: Some(ScopeContext {
            container: "build-each".into(),
            iteration: 2,
            position: 1,
        }),
    };
    let value = serde_json::to_value(&original).unwrap();
    let ctx = value
        .as_object()
        .unwrap()
        .get("scope_context")
        .expect("scope_context must be present when Some");
    assert_eq!(ctx["container"], json!("build-each"));
    assert_eq!(ctx["iteration"], json!(2));
    assert_eq!(ctx["position"], json!(1));

    let back: Event = serde_json::from_value(value).unwrap();
    match back {
        Event::StepStarted {
            scope_context: Some(c),
            ..
        } => {
            assert_eq!(c.container, "build-each");
            assert_eq!(c.iteration, 2);
            assert_eq!(c.position, 1);
        }
        other => panic!("roundtrip produced unexpected variant: {other:?}"),
    }
}

#[test]
fn step_completed_with_scope_context_and_gate_outcome_roundtrips() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 20, 12, 0, 1).unwrap();
    let original = Event::StepCompleted {
        step_name: "check".into(),
        step_type: "gate".into(),
        duration_ms: 3,
        timestamp: ts,
        input_tokens: None,
        output_tokens: None,
        cost_usd: None,
        sandboxed: false,
        output: None,
        scope_context: Some(ScopeContext {
            container: "loop".into(),
            iteration: 0,
            position: 2,
        }),
        gate_outcome: Some(GateOutcome::Skip),
        agent_session_id: None,
    };
    let value = serde_json::to_value(&original).unwrap();
    assert_eq!(value["gate_outcome"], json!("skip"));
    assert_eq!(value["scope_context"]["container"], json!("loop"));

    let back: Event = serde_json::from_value(value).unwrap();
    match back {
        Event::StepCompleted {
            scope_context: Some(c),
            gate_outcome: Some(g),
            ..
        } => {
            assert_eq!(c.position, 2);
            assert_eq!(g, GateOutcome::Skip);
        }
        other => panic!("roundtrip produced unexpected variant: {other:?}"),
    }
}

#[test]
fn gate_outcome_serializes_as_snake_case() {
    // Locking the wire spelling — `continue` / `skip` / `break` — so a
    // future rename of the enum variant can't silently change the JSON
    // shape the Dashboard / subscribers rely on.
    for (variant, expected) in [
        (GateOutcome::Continue, "continue"),
        (GateOutcome::Skip, "skip"),
        (GateOutcome::Break, "break"),
    ] {
        let v = serde_json::to_value(variant).unwrap();
        assert_eq!(v, json!(expected), "wire spelling for {variant:?}");
    }
}

#[test]
fn gate_outcome_rejects_unknown_values_on_deserialize() {
    // `fail` routes through StepFailed, not through gate_outcome, so a
    // log entry claiming `gate_outcome: "fail"` is malformed and must
    // fail to parse. Same for arbitrary strings — the contract test
    // pins the accepted values to the three variants.
    let bad = json!({
        "event": "step_completed",
        "step_name": "g",
        "step_type": "gate",
        "duration_ms": 0,
        "timestamp": "2026-04-20T12:00:00Z",
        "sandboxed": false,
        "gate_outcome": "fail"
    });
    assert!(
        serde_json::from_value::<Event>(bad).is_err(),
        "unknown gate_outcome value must fail deserialization"
    );
    let gibberish = json!({
        "event": "step_completed",
        "step_name": "g",
        "step_type": "gate",
        "duration_ms": 0,
        "timestamp": "2026-04-20T12:00:00Z",
        "sandboxed": false,
        "gate_outcome": "explode"
    });
    assert!(serde_json::from_value::<Event>(gibberish).is_err());
}

#[test]
fn legacy_step_started_without_scope_context_deserializes() {
    // A pre-PR-3 log entry — no `scope_context` field at all — must
    // still round-trip into today's Event::StepStarted so old sessions
    // can be replayed unchanged.
    let legacy = json!({
        "event": "step_started",
        "step_name": "x",
        "step_type": "cmd",
        "timestamp": "2026-04-13T12:00:00Z"
    });
    let back: Event = serde_json::from_value(legacy).unwrap();
    match back {
        Event::StepStarted {
            scope_context: None,
            step_name,
            ..
        } => assert_eq!(step_name, "x"),
        other => panic!("legacy step_started must deserialize: {other:?}"),
    }
}

#[test]
fn legacy_step_completed_without_pr3_fields_deserializes() {
    // A pre-PR-3 log entry — neither `scope_context` nor `gate_outcome`
    // present — must still deserialize so sessions logged before the
    // widening replay cleanly. Also verifies the PR-2 `output` field
    // stays optional for the same reason.
    let legacy = json!({
        "event": "step_completed",
        "step_name": "x",
        "step_type": "cmd",
        "duration_ms": 10,
        "timestamp": "2026-04-13T12:00:00Z",
        "sandboxed": true
    });
    let back: Event = serde_json::from_value(legacy).unwrap();
    match back {
        Event::StepCompleted {
            step_name,
            output: None,
            scope_context: None,
            gate_outcome: None,
            ..
        } => assert_eq!(step_name, "x"),
        other => panic!("legacy step_completed must deserialize: {other:?}"),
    }
}

// ── PR 5a of #31 — agent_session_id wire-shape lock ─────────────────────

#[test]
fn step_completed_with_agent_session_id_roundtrips() {
    // Emitted by an `agent` step that captured a Claude CLI `session_id`
    // from its streaming JSON. The field must surface on the wire so
    // `progress_from_log` can rebuild the shared/isolated resume chain
    // from the session log alone (Invariante 11 — no per-process
    // in-memory state between steps).
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap();
    let original = Event::StepCompleted {
        step_name: "review".into(),
        step_type: "agent".into(),
        duration_ms: 1_234,
        timestamp: ts,
        input_tokens: Some(100),
        output_tokens: Some(250),
        cost_usd: Some(0.0042),
        sandboxed: false,
        output: Some(StepOutputSnapshot {
            stdout: "response text".into(),
            stderr: String::new(),
            exit_code: 0,
        }),
        scope_context: None,
        gate_outcome: None,
        agent_session_id: Some("ses_abc123".into()),
    };

    let value = serde_json::to_value(&original).unwrap();
    assert_eq!(value["agent_session_id"], json!("ses_abc123"));

    let back: Event = serde_json::from_value(value).unwrap();
    match back {
        Event::StepCompleted {
            agent_session_id: Some(id),
            step_type,
            ..
        } => {
            assert_eq!(id, "ses_abc123");
            assert_eq!(step_type, "agent");
        }
        other => panic!("roundtrip produced unexpected variant: {other:?}"),
    }
}

#[test]
fn legacy_step_completed_without_agent_session_id_deserializes() {
    // A pre-PR-5a log entry — no `agent_session_id` key — must still
    // deserialize so sessions logged before the widening (including the
    // PR 2 / PR 3 shapes) replay cleanly. The absent field maps to
    // `None`.
    let legacy = json!({
        "event": "step_completed",
        "step_name": "review",
        "step_type": "agent",
        "duration_ms": 1_000,
        "timestamp": "2026-04-15T12:00:00Z",
        "sandboxed": false,
        "input_tokens": 50,
        "output_tokens": 120
    });
    let back: Event = serde_json::from_value(legacy).unwrap();
    match back {
        Event::StepCompleted {
            step_name,
            agent_session_id: None,
            ..
        } => assert_eq!(step_name, "review"),
        other => panic!("legacy step_completed must deserialize: {other:?}"),
    }
}

// ── PR 5b of #31 — ChatMessageAppended wire-shape lock ──────────────────

#[test]
fn chat_message_appended_roundtrips() {
    // The new variant that lets post-crash replay rebuild
    // `chat_sessions` from the session log alone (Invariante 11 — no
    // in-memory chat history between `step` calls). Freezes the five
    // fields and the `snake_case` role spelling that subscribers /
    // replay scanners rely on.
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 22, 12, 0, 0).unwrap();
    let original = Event::ChatMessageAppended {
        step_name: "draft".into(),
        session: "shared".into(),
        role: ChatRole::Assistant,
        content: "ack".into(),
        timestamp: ts,
    };

    let value = serde_json::to_value(&original).unwrap();
    assert_eq!(value["event"], json!("chat_message_appended"));
    assert_eq!(value["step_name"], json!("draft"));
    assert_eq!(value["session"], json!("shared"));
    assert_eq!(value["role"], json!("assistant"));
    assert_eq!(value["content"], json!("ack"));
    assert_eq!(value["timestamp"], json!("2026-04-22T12:00:00Z"));

    let back: Event = serde_json::from_value(value).unwrap();
    match back {
        Event::ChatMessageAppended {
            step_name,
            session,
            role,
            content,
            ..
        } => {
            assert_eq!(step_name, "draft");
            assert_eq!(session, "shared");
            assert_eq!(role, ChatRole::Assistant);
            assert_eq!(content, "ack");
        }
        other => panic!("roundtrip produced unexpected variant: {other:?}"),
    }
}

#[test]
fn chat_role_rejects_unknown_values_on_deserialize() {
    // The replay gate: a corrupted log row claiming `role: "system"`
    // (or any other value outside user/assistant) must fail to
    // deserialize so `compute_progress` surfaces it as `InvalidState`
    // rather than silently coercing the turn into a default role and
    // changing the prompt the next chat turn sees. Mirrors the
    // `gate_outcome_rejects_unknown_values_on_deserialize` lock.
    let bad = json!({
        "event": "chat_message_appended",
        "step_name": "s",
        "session": "s",
        "role": "system",
        "content": "",
        "timestamp": "2026-04-22T12:00:00Z",
    });
    assert!(
        serde_json::from_value::<Event>(bad).is_err(),
        "unknown role value must fail deserialization"
    );

    let numeric = json!({
        "event": "chat_message_appended",
        "step_name": "s",
        "session": "s",
        "role": 0,
        "content": "",
        "timestamp": "2026-04-22T12:00:00Z",
    });
    assert!(
        serde_json::from_value::<Event>(numeric).is_err(),
        "non-string role must fail deserialization"
    );
}

#[test]
fn unknown_event_variant_can_be_routed_to_other() {
    // A subscriber that chooses to forward-compat by using #[serde(other)]
    // can deserialize unknown variants without failing — this is the NFC6
    // contract that lets the dashboard ship behind the engine.
    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(tag = "event", rename_all = "snake_case")]
    enum SubscriberView {
        StepStarted {
            step_name: String,
        },
        #[serde(other)]
        Unknown,
    }

    // A real Event::WorkflowStarted serialized JSON — known variant.
    let known = json!({"event": "workflow_started", "timestamp": "2026-04-13T12:00:00Z"});
    let view: SubscriberView = serde_json::from_value(known).unwrap();
    assert_eq!(view, SubscriberView::Unknown); // Known to engine, unknown to this view.

    // A fictitious future variant the subscriber has never heard of.
    let future = json!({"event": "router_decided", "route": "fast"});
    let view: SubscriberView = serde_json::from_value(future).unwrap();
    assert_eq!(view, SubscriberView::Unknown);

    // The variant the subscriber DOES care about still parses correctly.
    let mine = json!({
        "event": "step_started",
        "step_name": "x",
        "step_type": "cmd",
        "timestamp": "2026-04-13T12:00:00Z"
    });
    let view: SubscriberView = serde_json::from_value(mine).unwrap();
    assert_eq!(
        view,
        SubscriberView::StepStarted {
            step_name: "x".into()
        }
    );
}

#[test]
fn event_roundtrip_through_serde_json() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap();
    let original = Event::WorkflowCompleted {
        duration_ms: 1500,
        timestamp: ts,
    };
    let s = serde_json::to_string(&original).unwrap();
    let back: Event = serde_json::from_str(&s).unwrap();
    match back {
        Event::WorkflowCompleted { duration_ms, .. } => assert_eq!(duration_ms, 1500),
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn engine_error_display_messages_are_stable() {
    assert_eq!(
        EngineError::InvalidWorkflow("missing field x".into()).to_string(),
        "invalid workflow: missing field x"
    );
    assert_eq!(EngineError::Cancelled.to_string(), "cancelled");
    assert_eq!(
        EngineError::StepFailed {
            step_index: 2,
            reason: TerminationReason::StepTimeout {
                configured_ms: 5000
            },
        }
        .to_string(),
        "step 2 failed: step timeout after 5000ms"
    );
    assert_eq!(
        EngineError::InvalidWorkflowField {
            path: "steps[0].timeout".into(),
            got: "30".into(),
            expected: "duration string (e.g. 30s, 500ms, 1h30m)",
        }
        .to_string(),
        "invalid workflow field at `steps[0].timeout`: got `30`, \
         expected duration string (e.g. 30s, 500ms, 1h30m)"
    );
}

#[test]
fn event_subscriber_is_dyn_compatible() {
    // If this compiles, EventSubscriber can be used as a trait object —
    // which the engine relies on when fanning events out via Box<dyn>.
    fn _accepts_dyn(_: Box<dyn EventSubscriber>) {}
}
