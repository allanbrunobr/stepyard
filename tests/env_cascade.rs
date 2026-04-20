//! Story 3.4 — cascade resolver tests.
//!
//! Verifies the overlay precedence (`defaults.env` < `workflow.env` <
//! `step.env`), the host `${VAR}` expansion semantics, and the fail-fast
//! error on a missing host variable.
//!
//! The tests drive [`stepyard_harness::resolve_env`] directly rather than
//! constructing a full [`stepyard_harness::Engine`] — the engine needs a
//! Postgres-backed `Session` which would require a live database per test
//! run. The engine method [`Engine::prepare_step`] is a thin delegate over
//! `resolve_env`, so testing the free function covers the merge + expansion
//! logic end-to-end.
//!
//! NFR8 (no secrets in logs) is vacuously satisfied at this layer: the
//! function returns the resolved map and never tracing!-logs values. Event
//! payload NFR8 compliance is asserted by Engine-level tests elsewhere.
//!
//! `#[serial_test::serial]` guards against parallel-test races on
//! `std::env::set_var` — one test's setvar would otherwise be visible to
//! unrelated tests running on other threads.

use std::collections::HashMap;

use stepyard_harness::{resolve_env, Defaults, EngineError};
use serial_test::serial;

fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
#[serial]
fn cascade_overlay_and_host_expansion() {
    // Story 3.4 AC5 fixture: defaults contribute ONLY_DEF; workflow
    // overrides SHARED; step overrides FOO and pulls TOKEN from host.
    // SAFETY: Rust edition 2021 — `set_var` is safe; #[serial] prevents
    // parallel-test contamination.
    std::env::set_var("GITHUB_TOKEN", "abc123");

    let defaults = Defaults::with_env(env_map(&[
        ("SHARED", "def"),
        ("ONLY_DEF", "x"),
    ]));
    let workflow_env = env_map(&[
        ("FOO", "workflow-foo"),
        ("SHARED", "wf"),
    ]);
    let step_env = env_map(&[
        ("FOO", "step-foo"),
        ("TOKEN", "${GITHUB_TOKEN}"),
    ]);

    let resolved = resolve_env(&defaults, &workflow_env, &step_env)
        .expect("cascade with valid ${VAR} must resolve");

    assert_eq!(resolved.get("FOO").map(String::as_str), Some("step-foo"));
    assert_eq!(resolved.get("SHARED").map(String::as_str), Some("wf"));
    assert_eq!(resolved.get("ONLY_DEF").map(String::as_str), Some("x"));
    assert_eq!(resolved.get("TOKEN").map(String::as_str), Some("abc123"));
    assert_eq!(resolved.len(), 4);

    std::env::remove_var("GITHUB_TOKEN");
}

#[test]
#[serial]
fn unresolved_host_var_returns_invalid_state() {
    // Ensure a clean slate so a leaked earlier setvar cannot satisfy the
    // reference.
    std::env::remove_var("MISSING_STORY_3_4");

    let defaults = Defaults::default();
    let workflow_env = HashMap::new();
    let step_env = env_map(&[("TOKEN", "${MISSING_STORY_3_4}")]);

    let err = resolve_env(&defaults, &workflow_env, &step_env)
        .expect_err("missing host var must fail fast");

    match err {
        EngineError::InvalidState(msg) => {
            // AC2 message format is locked: lowercase, no trailing
            // punctuation, key name present verbatim.
            assert_eq!(msg, "host env variable not set: MISSING_STORY_3_4");
        }
        other => panic!("expected InvalidState, got {other:?}"),
    }
}

#[test]
#[serial]
fn inline_var_ref_is_not_expanded() {
    // AC1 scope cut: only `^\$\{[A-Z0-9_]+\}$` (exact-form) is expanded.
    // Inline refs pass through verbatim so users hit a deliberate behavior
    // boundary instead of half-working interpolation.
    std::env::set_var("GITHUB_TOKEN", "abc123");

    let defaults = Defaults::default();
    let workflow_env = HashMap::new();
    let step_env = env_map(&[
        ("INLINE", "prefix-${GITHUB_TOKEN}-suffix"),
        ("LOWER", "${lowercase_var}"),
        ("EMPTY", "${}"),
    ]);

    let resolved = resolve_env(&defaults, &workflow_env, &step_env)
        .expect("non-${VAR} values must pass through, not error");

    assert_eq!(resolved["INLINE"], "prefix-${GITHUB_TOKEN}-suffix");
    assert_eq!(resolved["LOWER"], "${lowercase_var}");
    assert_eq!(resolved["EMPTY"], "${}");

    std::env::remove_var("GITHUB_TOKEN");
}

#[test]
#[serial]
fn step_beats_workflow_beats_defaults() {
    // Focused precedence assertion — each key is present in exactly the
    // layer whose value should win.
    let defaults = Defaults::with_env(env_map(&[
        ("A", "defaults"),
        ("B", "defaults"),
        ("C", "defaults"),
    ]));
    let workflow_env = env_map(&[
        ("B", "workflow"),
        ("C", "workflow"),
    ]);
    let step_env = env_map(&[("C", "step")]);

    let resolved = resolve_env(&defaults, &workflow_env, &step_env)
        .expect("no host expansion — cannot fail");

    assert_eq!(resolved["A"], "defaults");
    assert_eq!(resolved["B"], "workflow");
    assert_eq!(resolved["C"], "step");
}

#[test]
#[serial]
fn resolution_under_10ms_for_20_entries() {
    // NFR4 budget: ≤10ms for ≤20 entries. This is a soft guard against
    // accidental quadratic behavior creeping in — not a production SLO.
    std::env::set_var("HOST_STORY_3_4", "v");

    let mut defaults_map = HashMap::new();
    let mut workflow_map = HashMap::new();
    let mut step_map = HashMap::new();
    for i in 0..7 {
        defaults_map.insert(format!("D_{i}"), format!("d{i}"));
    }
    for i in 0..7 {
        workflow_map.insert(format!("W_{i}"), format!("w{i}"));
    }
    for i in 0..6 {
        step_map.insert(format!("S_{i}"), "${HOST_STORY_3_4}".to_string());
    }

    let defaults = Defaults::with_env(defaults_map);
    let start = std::time::Instant::now();
    let _ = resolve_env(&defaults, &workflow_map, &step_map).expect("resolve");
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(10),
        "cascade took {elapsed:?}, expected <10ms for 20 entries (NFR4)"
    );

    std::env::remove_var("HOST_STORY_3_4");
}
