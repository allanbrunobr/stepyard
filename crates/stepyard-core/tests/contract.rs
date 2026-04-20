//! Contract tests for the public surface of `stepyard-core`. These freeze the
//! externally visible behavior:
//!
//! * Event JSON discriminator and field names are stable (Story 2.1 AC).
//! * Subscribers can ignore unknown variants via `#[serde(other)]` (NFC6).
//! * EngineError exposes no `anyhow::Error` in its public surface.
//! * EventSubscriber is dyn-compatible.

use chrono::TimeZone;
use stepyard_core::{EngineError, Event, EventSubscriber, StepOutputSnapshot, TerminationReason};
use serde::Deserialize;
use serde_json::json;

#[test]
fn step_started_serialization_is_stable() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap();
    let event = Event::StepStarted {
        step_name: "review".into(),
        step_type: "agent".into(),
        timestamp: ts,
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
}

#[test]
fn event_subscriber_is_dyn_compatible() {
    // If this compiles, EventSubscriber can be used as a trait object —
    // which the engine relies on when fanning events out via Box<dyn>.
    fn _accepts_dyn(_: Box<dyn EventSubscriber>) {}
}
