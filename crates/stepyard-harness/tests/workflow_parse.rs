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

use std::collections::HashMap;
use std::time::Duration;

use stepyard_core::env::{EXPECTED_KEY, EXPECTED_VALUE, REDACTED_NUL_VALUE};
use stepyard_core::EngineError;
use stepyard_harness::{Step, Workflow};

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
            // Substring match, not equality: serde_yaml 0.9 quotes the raw
            // scalar inside backticks, but its exact Display wording is not
            // part of the contract — the invariant is that `got` carries
            // the offending value somewhere inside it.
            assert!(
                got.contains("30"),
                "got should carry the raw YAML value, got {got:?}"
            );
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
fn valid_idle_timeout_duration_string_parses_to_duration() {
    let yaml = r#"
name: idle
steps:
  - name: fine
    command: "true"
    idle_timeout: 2m500ms
"#;

    let wf = Workflow::try_from_yaml(yaml).expect("valid idle_timeout parses");
    assert_eq!(
        wf.steps[0].idle_timeout,
        Some(Duration::from_millis(120_500))
    );
}

#[test]
fn bare_integer_idle_timeout_fails_with_invalid_workflow_field() {
    let yaml = r#"
name: bad-idle
steps:
  - name: oops
    command: "true"
    idle_timeout: 30
"#;

    let err = Workflow::try_from_yaml(yaml).expect_err("bare integer must be rejected");
    match err {
        EngineError::InvalidWorkflowField {
            path,
            got,
            expected,
        } => {
            assert!(path.contains("idle_timeout"));
            assert!(got.contains("30"));
            assert!(expected.contains("duration string"));
        }
        other => panic!("expected InvalidWorkflowField, got {other:?}"),
    }
}

#[test]
fn idle_timeout_roundtrip_normalizes_to_canonical_form() {
    let yaml = r#"
name: idle-round
steps:
  - name: one
    command: "true"
    idle_timeout: 90s
"#;

    let wf = Workflow::try_from_yaml(yaml).expect("90s parses");
    assert_eq!(wf.steps[0].idle_timeout, Some(Duration::from_secs(90)));

    let emitted = serde_yaml::to_string(&wf).expect("round-trip serialize");
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
        EngineError::InvalidWorkflowField { path, expected, .. } => {
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

// ---------------------------------------------------------------------------
// Round 3 Story 3 — env key/value validation at the workflow boundary.
// `Workflow::try_from_yaml` runs `Workflow::validate` post-deserialize so
// every env entry (workflow-level, per-step, per-scope-step) satisfies the
// `^[A-Za-z_][A-Za-z0-9_]*$` key grammar and the "UTF-8 without NUL" value
// grammar before the document leaves the parse boundary.
// ---------------------------------------------------------------------------

#[test]
fn bad_env_key_top_level_fails_with_invalid_workflow_field() {
    let yaml = r#"
name: bad
env:
  BAD-KEY: ok
steps:
  - name: one
    command: "true"
"#;

    let err = Workflow::try_from_yaml(yaml).expect_err("hyphen in key must be rejected");
    match err {
        EngineError::InvalidWorkflowField {
            path,
            got,
            expected,
        } => {
            assert_eq!(path, "env.BAD-KEY", "path must point at the offending key");
            assert!(
                got.contains("BAD-KEY"),
                "got must surface the bad key, got {got:?}"
            );
            assert_eq!(
                expected, EXPECTED_KEY,
                "expected must reuse the stable EXPECTED_KEY constant"
            );
        }
        other => panic!("expected InvalidWorkflowField, got {other:?}"),
    }
}

#[test]
fn bad_env_key_inside_step_carries_steps_index_path() {
    // Three steps; the third (index 2) carries the bad key. Path must
    // pin the index so the operator finds the offender in long
    // workflows without grepping.
    let yaml = r#"
name: bad
steps:
  - name: zero
    command: "true"
  - name: one
    command: "true"
  - name: two
    command: "true"
    env:
      BAD-KEY: ok
"#;

    let err = Workflow::try_from_yaml(yaml).expect_err("bad key in steps[2].env");
    match err {
        EngineError::InvalidWorkflowField { path, expected, .. } => {
            assert_eq!(
                path, "steps[2].env.BAD-KEY",
                "path must encode the step index"
            );
            assert_eq!(expected, EXPECTED_KEY);
        }
        other => panic!("expected InvalidWorkflowField, got {other:?}"),
    }
}

#[test]
fn bad_env_key_inside_scope_carries_scopes_path() {
    // Scope `work` has two steps; the second (index 1) holds the bad
    // key. Scoped path format is `scopes.<name>.steps[<idx>].env.<KEY>`.
    let yaml = r#"
name: bad
steps:
  - name: drive
    command: "true"
scopes:
  work:
    steps:
      - name: alpha
        command: "true"
      - name: beta
        command: "true"
        env:
          BAD-KEY: ok
"#;

    let err = Workflow::try_from_yaml(yaml).expect_err("bad key in scopes.work.steps[1].env");
    match err {
        EngineError::InvalidWorkflowField { path, .. } => {
            assert_eq!(path, "scopes.work.steps[1].env.BAD-KEY");
        }
        other => panic!("expected InvalidWorkflowField, got {other:?}"),
    }
}

#[test]
fn valid_env_passes_through_try_from_yaml() {
    // Every grammar-compliant entry — leading underscore, mixed case,
    // empty value, value with shell metachars — must round-trip cleanly.
    let yaml = r#"
name: ok
env:
  FOO: bar
  _LEADING: ""
  MIXED_123: "spaces and = signs"
steps:
  - name: one
    command: "true"
    env:
      STEP_ENV: "value"
scopes:
  work:
    steps:
      - name: scoped
        command: "true"
        env:
          SCOPE_ENV: "✓ utf8"
"#;

    let wf = Workflow::try_from_yaml(yaml).expect("clean env must pass");
    assert_eq!(wf.env.get("FOO"), Some(&"bar".to_string()));
    assert_eq!(wf.env.get("_LEADING"), Some(&"".to_string()));
    assert_eq!(wf.steps[0].env.get("STEP_ENV"), Some(&"value".to_string()));
    assert_eq!(
        wf.scopes["work"].steps[0].env.get("SCOPE_ENV"),
        Some(&"✓ utf8".to_string())
    );
}

#[test]
fn empty_env_value_passes() {
    // An empty value is UTF-8 without NUL — explicitly allowed (operators
    // sometimes set `FOO: ""` to clear an inherited host var).
    let yaml = r#"
name: ok
env:
  FOO: ""
steps:
  - name: one
    command: "true"
"#;

    let wf = Workflow::try_from_yaml(yaml).expect("empty value must pass");
    assert_eq!(wf.env.get("FOO"), Some(&"".to_string()));
}

#[test]
fn nul_in_env_value_redacts_got_and_does_not_leak_length() {
    // YAML embeds the NUL via the `\x00` escape inside a double-quoted
    // scalar. `got` MUST be the fixed redacted string — no length, no
    // index, no prefix, no suffix. This is the env-value
    // confidentiality rule from architecture.md §G env hardening: a
    // value-shaped failure must NOT fingerprint the secret.
    let yaml = "
name: bad
env:
  API_TOKEN: \"sk_live_x\\0LEAK\"
steps:
  - name: one
    command: \"true\"
";

    let err = Workflow::try_from_yaml(yaml).expect_err("NUL in value must be rejected");
    match err {
        EngineError::InvalidWorkflowField {
            path,
            got,
            expected,
        } => {
            assert_eq!(
                path, "env.API_TOKEN",
                "path must embed the (valid) key so operator can locate the offender"
            );
            assert_eq!(
                got, REDACTED_NUL_VALUE,
                "got MUST be the fixed redacted string, got {got:?}"
            );
            assert!(
                !got.contains("sk_live"),
                "redacted got must not leak any prefix of the value"
            );
            assert!(
                !got.contains("LEAK"),
                "redacted got must not leak any suffix of the value"
            );
            assert!(
                !got.chars().any(|c| c.is_ascii_digit()),
                "redacted got must not encode the byte length or NUL index"
            );
            assert_eq!(
                expected, EXPECTED_VALUE,
                "expected must reuse the stable EXPECTED_VALUE constant"
            );
        }
        other => panic!("expected InvalidWorkflowField, got {other:?}"),
    }
}

#[test]
fn programmatic_workflow_validate_fails_on_bad_env() {
    // Adapter path: `harness_adapter::adapt` builds a Workflow via
    // `Workflow::new` + field assignment, NOT YAML parse. The
    // boundary check must be reachable that way too — `Workflow::new`
    // itself stays trusting (so tests like this can build invalid
    // workflows for negative-path coverage), and the explicit
    // `validate()` call is the contract every programmatic caller
    // honors before handing the workflow to the engine.
    let mut bad_env: HashMap<String, String> = HashMap::new();
    bad_env.insert("BAD-KEY".into(), "ok".into());
    let step = Step::cmd("only", "true").with_env(bad_env);

    let mut wf = Workflow::new("prog", vec![step]);
    // Workflow::new MUST stay trusting — programmatic construction
    // alone does not surface the validation error.
    assert!(
        wf.scopes.is_empty(),
        "Workflow::new must not auto-populate scopes"
    );

    let err = wf.validate().expect_err("validate() catches bad step env");
    match err {
        EngineError::InvalidWorkflowField { path, expected, .. } => {
            assert_eq!(path, "steps[0].env.BAD-KEY");
            assert_eq!(expected, EXPECTED_KEY);
        }
        other => panic!("expected InvalidWorkflowField, got {other:?}"),
    }

    // The negative case is the contract; the positive case pins that
    // a freshly-constructed clean workflow passes the same call.
    wf.steps[0].env.clear();
    wf.steps[0].env.insert("GOOD_KEY".into(), "value".into());
    wf.validate().expect("clean workflow must pass validate()");
}
