//! Round 3 Story 1 — workflow YAML parse boundary.
//!
//! AC6: a bare integer `timeout:` value fails at parse time with
//! [`EngineError::InvalidWorkflowField`], carrying the serde path and
//! the canonical duration-grammar hint. The operator learns about a
//! typed-field mismatch the moment `try_from_yaml` runs, not after a
//! surprising millisecond interpretation mid-run.
//!
//! AC8: a duration string round-trips through
//! [`Workflow::try_from_yaml`] and `serde_yaml::to_string` with
//! normalization — `90s` decomposes to `1m30s` on the wire. The
//! canonical serializer in `stepyard_core::duration` is what lets
//! downstream tooling (session dump, CLI inspectors) quote a single
//! spelling per duration value rather than whatever the operator
//! originally typed.
//!
//! Neither test needs a database — parsing is pure serde work.

use std::time::Duration;

use stepyard_core::EngineError;
use stepyard_harness::Workflow;

// ---------------------------------------------------------------------------
// AC6 — bare integer timeout fails before execution.
// ---------------------------------------------------------------------------

#[test]
fn bare_integer_timeout_fails_with_invalid_workflow_field() {
    let yaml = r#"
name: bad
steps:
  - name: oops
    command: "true"
    timeout: 30
"#;

    let err = Workflow::try_from_yaml(yaml).expect_err("bare integer must be rejected");

    match err {
        EngineError::InvalidWorkflowField {
            path,
            got,
            expected,
        } => {
            assert!(
                path.contains("timeout"),
                "path should mention the offending field, got {path:?}"
            );
            // serde_yaml reports the offending integer inside backticks.
            assert_eq!(got, "30", "got should be the raw YAML value");
            assert!(
                expected.contains("duration string"),
                "expected hint should reference the duration grammar, got {expected:?}"
            );
        }
        other => panic!("expected InvalidWorkflowField, got {other:?}"),
    }
}

#[test]
fn valid_duration_string_timeout_parses_to_duration() {
    let yaml = r#"
name: ok
steps:
  - name: fine
    command: "true"
    timeout: 60ms
"#;

    let wf = Workflow::try_from_yaml(yaml).expect("valid duration string parses");
    assert_eq!(wf.steps[0].timeout, Some(Duration::from_millis(60)));
}

#[test]
fn absent_timeout_deserializes_as_none() {
    // Backward-compat: existing workflows without a `timeout:` field must
    // keep parsing cleanly after the wire-type tightening.
    let yaml = r#"
name: ok
steps:
  - name: fine
    command: "true"
"#;

    let wf = Workflow::try_from_yaml(yaml).expect("no timeout parses");
    assert_eq!(wf.steps[0].timeout, None);
}

#[test]
fn malformed_duration_string_fails_with_invalid_workflow_field() {
    // The strict grammar rejects uppercase, whitespace, decimals, mixed
    // order. All of them surface through the same error path as the
    // bare-integer case — no silent fallback, no opaque serde message.
    let yaml = r#"
name: bad
steps:
  - name: oops
    command: "true"
    timeout: "30 s"
"#;

    let err = Workflow::try_from_yaml(yaml).expect_err("whitespace must be rejected");
    match err {
        EngineError::InvalidWorkflowField {
            path, expected, ..
        } => {
            assert!(path.contains("timeout"));
            assert!(expected.contains("duration string"));
        }
        other => panic!("expected InvalidWorkflowField, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AC8 — round-trip with canonical normalization.
// ---------------------------------------------------------------------------

#[test]
fn duration_roundtrip_normalizes_to_canonical_form() {
    // `90s` parses to 90 000 ms; the canonical serializer decomposes that
    // back into the biggest non-zero segments, so the emitted YAML holds
    // `1m30s`. Asserting on the re-serialized form pins the invariant
    // that downstream consumers see one spelling per duration value.
    let yaml = r#"
name: round
steps:
  - name: one
    command: "true"
    timeout: 90s
"#;

    let wf = Workflow::try_from_yaml(yaml).expect("90s parses");
    assert_eq!(wf.steps[0].timeout, Some(Duration::from_secs(90)));

    let emitted = serde_yaml::to_string(&wf).expect("round-trip serialize");
    // serde_yaml may quote scalars; substring match stays stable across
    // its version-to-version formatting choices.
    assert!(
        emitted.contains("1m30s"),
        "canonical output should carry `1m30s`, got:\n{emitted}"
    );
    assert!(
        !emitted.contains("90s"),
        "canonical output should not keep the original `90s`, got:\n{emitted}"
    );
}

#[test]
fn duration_zero_roundtrip_emits_zero_seconds() {
    // `Duration::ZERO` is the one edge case where greedy high-to-low
    // decomposition would emit the empty string. The serializer hard-codes
    // `0s` instead — locks the invariant that every valid timeout round-trips
    // through at least one non-empty segment.
    let yaml = r#"
name: zero
steps:
  - name: one
    command: "true"
    timeout: 0s
"#;

    let wf = Workflow::try_from_yaml(yaml).expect("0s parses");
    assert_eq!(wf.steps[0].timeout, Some(Duration::ZERO));

    let emitted = serde_yaml::to_string(&wf).expect("round-trip serialize");
    assert!(
        emitted.contains("0s"),
        "Duration::ZERO should round-trip as `0s`, got:\n{emitted}"
    );
}
