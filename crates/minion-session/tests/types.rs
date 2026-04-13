//! Unit tests for types that do not require a live PostgreSQL.
//!
//! Integration tests covering append/replay/load against a real database live
//! in `tests/integration.rs` and are gated behind the `MINION_SESSION_DATABASE_URL`
//! environment variable.

use minion_session::{Session, SessionEvent, SessionId, SessionStatus};
use std::str::FromStr;
use uuid::Uuid;

#[test]
fn session_id_roundtrips_via_display_and_fromstr() {
    let uuid = Uuid::new_v4();
    let id = SessionId(uuid);

    let rendered = id.to_string();
    let parsed = SessionId::from_str(&rendered).expect("round-trip must succeed");

    assert_eq!(id, parsed);
    assert_eq!(*parsed.as_uuid(), uuid);
}

#[test]
fn session_id_from_uuid_and_back() {
    let uuid = Uuid::new_v4();
    let id: SessionId = uuid.into();
    let back: Uuid = id.into();
    assert_eq!(uuid, back);
}

#[test]
fn session_id_new_is_unique_v4() {
    let a = SessionId::new();
    let b = SessionId::new();
    assert_ne!(a, b, "random ids must differ");
}

#[test]
fn session_id_invalid_string_errors() {
    assert!(SessionId::from_str("not-a-uuid").is_err());
    assert!(SessionId::from_str("").is_err());
}

#[test]
fn session_status_as_str_matches_db_check_constraint() {
    // The CHECK constraint in the migration only allows these four values.
    assert_eq!(SessionStatus::Running.as_str(), "running");
    assert_eq!(SessionStatus::Completed.as_str(), "completed");
    assert_eq!(SessionStatus::Failed.as_str(), "failed");
    assert_eq!(SessionStatus::Cancelled.as_str(), "cancelled");
}

#[test]
fn session_status_serde_is_lowercase() {
    let status = SessionStatus::Running;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"running\"");

    let back: SessionStatus = serde_json::from_str("\"completed\"").unwrap();
    assert_eq!(back, SessionStatus::Completed);
}

#[test]
fn session_event_is_serde_roundtrip() {
    let event = SessionEvent {
        id: Uuid::new_v4(),
        session_id: SessionId::new(),
        seq: 42,
        created_at: chrono::Utc::now(),
        payload: serde_json::json!({"type": "step_started", "name": "x"}),
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: SessionEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event.id, back.id);
    assert_eq!(event.session_id, back.session_id);
    assert_eq!(event.seq, back.seq);
    assert_eq!(event.payload, back.payload);
}

// --- Compile-time trait assertions --------------------------------------
// These don't execute; they only fail to compile if trait bounds regress.
// The harness requires Session to be Clone + Send + Sync so it can be
// shared across tokio tasks without wrapping in Arc<Mutex<...>>.

#[allow(dead_code)]
fn assert_clone_send_sync<T: Clone + Send + Sync>() {}

#[test]
fn session_type_bounds() {
    // Will fail to compile (and thus fail the test run) if Session stops
    // being Clone + Send + Sync.
    assert_clone_send_sync::<Session>();
    assert_clone_send_sync::<SessionEvent>();
    assert_clone_send_sync::<SessionId>();
    assert_clone_send_sync::<SessionStatus>();
}
