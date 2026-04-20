//! Bridge between the legacy [`WorkflowDef`] (with all 10 step types) and the
//! narrower [`stepyard_harness::Workflow`] exposed to the v2 engine.
//!
//! PR 2 of Task #31 accepted `cmd` + `gate` only. PR 3 widens the accept list
//! to the three container kinds (`call` / `repeat` / `map`) and threads their
//! named scope bodies through the harness. Shape validation stays in the CLI
//! adapter so `stepyard-harness` never learns about legacy YAML:
//!
//! * `call` / `repeat` / `map` must reference a named scope that exists in
//!   `scopes:`; missing field → [`AdapterError::ContainerMissingScope`],
//!   unknown ref → [`AdapterError::UnknownScope`].
//! * `map` must declare an `items:` expression, but its rendered shape
//!   stays unenforced here — the scope runner preserves v1's
//!   "JSON array or line-split fallback" heuristic (v1 `map.rs:237`).
//! * Nested containers (a `call` / `repeat` / `map` inside another
//!   container's scope body) are rejected outright
//!   ([`AdapterError::NestedScopesNotSupported`]). A later PR that lifts
//!   this restriction can add a `scope_path` frame without reshaping the
//!   event log; see `stepyard_core::event::ScopeContext`.
//! * Gate actions split by position: `break` and `skip` only make sense
//!   inside a scope body, so a top-level gate that declares them fails
//!   with [`AdapterError::TopLevelGateUnsupportedAction`]; a scoped
//!   gate that declares an unknown action fails with
//!   [`AdapterError::ScopedGateUnsupportedAction`].
//!
//! Executors for `call` / `repeat` / `map` land in the scope-runner commit.
//! Kinds not yet executable (`agent` / `chat` / `template` / `script` /
//! `parallel`) continue to fail with [`AdapterError::UnsupportedStepType`].

use std::collections::{HashMap, HashSet};

use stepyard_harness::{Scope, Step, Workflow};

use crate::workflow::schema::{ScopeDef, StepDef, StepType, WorkflowDef};

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error(
        "step type `{step_type}` not yet supported by v2 engine — use --engine v1 \
         or migrate the workflow to a supported kind (cmd, gate, call, repeat, map)"
    )]
    UnsupportedStepType { step_type: StepType },

    #[error("step `{name}` has type cmd but no `run:` field")]
    CmdMissingRun { name: String },

    #[error("gate step `{name}` is missing `condition:`")]
    GateMissingCondition { name: String },

    #[error(
        "top-level gate step `{name}` action `{action}=\"{value}\"` is not \
         supported — `break` / `skip` only apply inside a call/repeat/map \
         scope body (allowed at top level: continue, fail)"
    )]
    TopLevelGateUnsupportedAction {
        name: String,
        action: &'static str,
        value: String,
    },

    #[error(
        "scoped gate step `{name}` action `{action}=\"{value}\"` is not a \
         recognized scope outcome (allowed inside a scope: continue, skip, \
         break, fail)"
    )]
    ScopedGateUnsupportedAction {
        name: String,
        action: &'static str,
        value: String,
    },

    #[error("{kind} step `{name}` is missing `scope:`")]
    ContainerMissingScope { name: String, kind: &'static str },

    #[error("map step `{name}` is missing `items:`")]
    MapMissingItems { name: String },

    #[error(
        "{kind} step `{name}` references scope `{scope}` which is not \
         declared in the workflow's `scopes:` block"
    )]
    UnknownScope {
        name: String,
        kind: &'static str,
        scope: String,
    },

    #[error(
        "nested containers are not supported in PR 3 of #31 — {kind} step \
         `{inner_name}` inside scope `{scope}` must be a non-container kind \
         (cmd, gate)"
    )]
    NestedScopesNotSupported {
        scope: String,
        inner_name: String,
        kind: &'static str,
    },

    #[error(
        "step `{name}` field `{field}:` value is not representable as JSON \
         (harness stores container seeds as JSON): {error}"
    )]
    InitialValueNotJson {
        name: String,
        field: &'static str,
        error: String,
    },
}

#[derive(Clone, Copy)]
enum StepPosition {
    TopLevel,
    Scoped,
}

/// Convert a parsed [`WorkflowDef`] into the harness-facing [`Workflow`].
///
/// Walks the top-level step list and every declared scope body. Executable
/// kinds (`cmd` / `gate` / `call` / `repeat` / `map`) are adapted; scoped
/// bodies additionally reject nested containers. Env maps (workflow-level
/// and step-level) are threaded through so the cascade resolver (Story 3.4)
/// has the values the v2 engine expects.
pub fn adapt(def: &WorkflowDef) -> Result<Workflow, AdapterError> {
    let scope_names: HashSet<&str> = def.scopes.keys().map(String::as_str).collect();

    let mut steps = Vec::with_capacity(def.steps.len());
    for s in &def.steps {
        steps.push(adapt_step(s, &scope_names, StepPosition::TopLevel)?);
    }

    let mut scopes: HashMap<String, Scope> = HashMap::with_capacity(def.scopes.len());
    for (scope_name, scope_def) in &def.scopes {
        scopes.insert(scope_name.clone(), adapt_scope(scope_name, scope_def, &scope_names)?);
    }

    let mut wf = Workflow::new(def.name.clone(), steps);
    wf.env = def.env.clone();
    wf.scopes = scopes;
    Ok(wf)
}

fn adapt_scope(
    scope_name: &str,
    scope_def: &ScopeDef,
    scope_names: &HashSet<&str>,
) -> Result<Scope, AdapterError> {
    let mut body = Vec::with_capacity(scope_def.steps.len());
    for inner in &scope_def.steps {
        // Guardrail 1: nested containers — caught here before adapt_step so
        // the inner kind can't be silently adapted as if top-level.
        if let Some(kind) = container_kind(&inner.step_type) {
            return Err(AdapterError::NestedScopesNotSupported {
                scope: scope_name.to_string(),
                inner_name: inner.name.clone(),
                kind,
            });
        }
        body.push(adapt_step(inner, scope_names, StepPosition::Scoped)?);
    }
    Ok(Scope {
        steps: body,
        outputs: scope_def.outputs.clone(),
    })
}

fn adapt_step(
    s: &StepDef,
    scope_names: &HashSet<&str>,
    position: StepPosition,
) -> Result<Step, AdapterError> {
    match &s.step_type {
        StepType::Cmd => adapt_cmd(s),
        StepType::Gate => adapt_gate(s, position),
        StepType::Call => adapt_call(s, scope_names),
        StepType::Repeat => adapt_repeat(s, scope_names),
        StepType::Map => adapt_map(s, scope_names),
        other => Err(AdapterError::UnsupportedStepType {
            step_type: other.clone(),
        }),
    }
}

fn adapt_cmd(s: &StepDef) -> Result<Step, AdapterError> {
    let cmd = s
        .run
        .clone()
        .ok_or_else(|| AdapterError::CmdMissingRun {
            name: s.name.clone(),
        })?;
    Ok(Step::cmd(s.name.clone(), cmd).with_env(s.env.clone()))
}

fn adapt_gate(s: &StepDef, position: StepPosition) -> Result<Step, AdapterError> {
    let condition = s
        .condition
        .clone()
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| AdapterError::GateMissingCondition {
            name: s.name.clone(),
        })?;

    // Guardrail 2: top-level gates reject break/skip; scoped gates accept
    // the full {continue, fail, skip, break} set.
    validate_gate_action(&s.name, "on_pass", s.on_pass.as_deref(), position)?;
    validate_gate_action(&s.name, "on_fail", s.on_fail.as_deref(), position)?;

    let mut step = Step::gate(s.name.clone(), condition);
    step.on_pass = s.on_pass.clone();
    step.on_fail = s.on_fail.clone();
    step.message = s.message.clone();
    step.env = s.env.clone();
    Ok(step)
}

fn adapt_call(s: &StepDef, scope_names: &HashSet<&str>) -> Result<Step, AdapterError> {
    let scope = require_scope_ref(s, "call", scope_names)?;
    let mut step = Step::call(s.name.clone(), scope);
    apply_common_container_fields(&mut step, s)?;
    Ok(step)
}

fn adapt_repeat(s: &StepDef, scope_names: &HashSet<&str>) -> Result<Step, AdapterError> {
    let scope = require_scope_ref(s, "repeat", scope_names)?;
    let mut step = Step::repeat(s.name.clone(), scope);
    step.max_iterations = s.max_iterations;
    apply_common_container_fields(&mut step, s)?;
    Ok(step)
}

fn adapt_map(s: &StepDef, scope_names: &HashSet<&str>) -> Result<Step, AdapterError> {
    let scope = require_scope_ref(s, "map", scope_names)?;
    // Guardrail 4: presence only. The render-time shape check stays with the
    // scope runner (v1 heuristic, map.rs:237).
    let items = s
        .items
        .clone()
        .filter(|i| !i.trim().is_empty())
        .ok_or_else(|| AdapterError::MapMissingItems {
            name: s.name.clone(),
        })?;
    let mut step = Step::map(s.name.clone(), scope, items);
    step.parallel = s.parallel;
    apply_common_container_fields(&mut step, s)?;
    Ok(step)
}

fn apply_common_container_fields(step: &mut Step, s: &StepDef) -> Result<(), AdapterError> {
    step.env = s.env.clone();
    step.outputs = s.outputs.clone();
    if let Some(v) = &s.initial_value {
        step.initial_value = Some(yaml_to_json(v, &s.name, "initial_value")?);
    }
    Ok(())
}

fn container_kind(step_type: &StepType) -> Option<&'static str> {
    match step_type {
        StepType::Call => Some("call"),
        StepType::Repeat => Some("repeat"),
        StepType::Map => Some("map"),
        _ => None,
    }
}

fn require_scope_ref(
    s: &StepDef,
    kind: &'static str,
    scope_names: &HashSet<&str>,
) -> Result<String, AdapterError> {
    // Guardrail 3: call/repeat/map need both a non-empty `scope:` value AND
    // a matching entry in `scopes:`. Two distinct errors so the message
    // points at the actual mistake.
    let scope = s
        .scope
        .clone()
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| AdapterError::ContainerMissingScope {
            name: s.name.clone(),
            kind,
        })?;
    if !scope_names.contains(scope.as_str()) {
        return Err(AdapterError::UnknownScope {
            name: s.name.clone(),
            kind,
            scope,
        });
    }
    Ok(scope)
}

fn validate_gate_action(
    step_name: &str,
    field: &'static str,
    raw: Option<&str>,
    position: StepPosition,
) -> Result<(), AdapterError> {
    let trimmed = raw.map(str::trim).unwrap_or("");
    if trimmed.is_empty() {
        return Ok(());
    }
    let allowed: &[&str] = match position {
        StepPosition::TopLevel => &["continue", "fail"],
        StepPosition::Scoped => &["continue", "fail", "skip", "break"],
    };
    if allowed.contains(&trimmed) {
        return Ok(());
    }
    Err(match position {
        StepPosition::TopLevel => AdapterError::TopLevelGateUnsupportedAction {
            name: step_name.to_string(),
            action: field,
            value: trimmed.to_string(),
        },
        StepPosition::Scoped => AdapterError::ScopedGateUnsupportedAction {
            name: step_name.to_string(),
            action: field,
            value: trimmed.to_string(),
        },
    })
}

fn yaml_to_json(
    v: &serde_yaml::Value,
    step_name: &str,
    field: &'static str,
) -> Result<serde_json::Value, AdapterError> {
    serde_json::to_value(v).map_err(|e| AdapterError::InitialValueNotJson {
        name: step_name.to_string(),
        field,
        error: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::parser;
    use std::io::Write;

    fn write_tmp(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(".yaml")
            .tempfile()
            .unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn adapts_cmd_only_workflow() {
        let yaml = r#"
name: adapter-smoke
steps:
  - name: one
    type: cmd
    run: "echo 1"
  - name: two
    type: cmd
    run: "echo 2"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        assert_eq!(wf.name, "adapter-smoke");
        assert_eq!(wf.steps.len(), 2);
        assert_eq!(wf.steps[0].name, "one");
        assert_eq!(wf.steps[1].command, "echo 2");
    }

    #[test]
    fn carries_env_at_workflow_and_step_scopes() {
        let yaml = r#"
name: adapter-env
env:
  WF_VAR: workflow_value
  SHARED: from_workflow
steps:
  - name: one
    type: cmd
    run: "echo 1"
    env:
      STEP_VAR: step_value
      SHARED: from_step
  - name: two
    type: cmd
    run: "echo 2"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();

        assert_eq!(wf.env.get("WF_VAR").map(String::as_str), Some("workflow_value"));
        assert_eq!(wf.env.get("SHARED").map(String::as_str), Some("from_workflow"));

        let step_one_env = &wf.steps[0].env;
        assert_eq!(step_one_env.get("STEP_VAR").map(String::as_str), Some("step_value"));
        assert_eq!(step_one_env.get("SHARED").map(String::as_str), Some("from_step"));

        assert!(
            wf.steps[1].env.is_empty(),
            "step without env: should have empty map, got {:?}",
            wf.steps[1].env
        );
    }

    #[test]
    fn accepts_gate_step() {
        let yaml = r#"
name: adapter-gate
steps:
  - name: check
    type: gate
    condition: "{{ steps.build.exit_code }} == 0"
    on_pass: continue
    on_fail: fail
    message: "build must be green"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        assert_eq!(wf.steps.len(), 1);
        let step = &wf.steps[0];
        assert_eq!(step.name, "check");
        assert_eq!(
            step.condition.as_deref(),
            Some("{{ steps.build.exit_code }} == 0")
        );
        assert_eq!(step.on_pass.as_deref(), Some("continue"));
        assert_eq!(step.on_fail.as_deref(), Some("fail"));
        assert_eq!(step.message.as_deref(), Some("build must be green"));
    }

    #[test]
    fn rejects_gate_without_condition() {
        let yaml = r#"
name: adapter-gate-no-cond
steps:
  - name: naked
    type: gate
    on_pass: continue
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing `condition:`"), "msg={msg}");
        assert!(msg.contains("naked"), "msg={msg}");
    }

    #[test]
    fn top_level_gate_rejects_break_and_skip() {
        for (field, value) in [
            ("on_pass", "break"),
            ("on_pass", "skip"),
            ("on_fail", "break"),
            ("on_fail", "skip"),
        ] {
            let yaml = format!(
                r#"
name: adapter-top-gate-bad-action
steps:
  - name: g
    type: gate
    condition: "true"
    {field}: {value}
"#
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let err = adapt(&def).unwrap_err();
            assert!(
                matches!(err, AdapterError::TopLevelGateUnsupportedAction { .. }),
                "expected TopLevelGateUnsupportedAction, got {err:?}"
            );
            let msg = err.to_string();
            assert!(msg.contains(value), "msg={msg}");
            assert!(msg.contains(field), "msg={msg}");
            assert!(msg.contains("top-level gate"), "msg={msg}");
        }
    }

    #[test]
    fn rejects_still_unsupported_step_type() {
        // `repeat` / `map` / `call` now adapt; pick a kind that still has
        // no executor in PR 3 of #31.
        let yaml = r#"
name: adapter-reject-agent
steps:
  - name: think
    type: agent
    prompt: "hello"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not yet supported"), "msg={msg}");
        assert!(msg.contains("agent"), "msg={msg}");
    }

    #[test]
    fn accepts_call_repeat_map_with_valid_scopes() {
        let yaml = r#"
name: adapter-containers
steps:
  - name: once
    type: call
    scope: setup
  - name: loop
    type: repeat
    scope: work
    max_iterations: 3
    initial_value: 0
  - name: fan
    type: map
    scope: per_item
    items: "{{ steps.list.stdout }}"
    parallel: 2
scopes:
  setup:
    steps:
      - name: seed
        type: cmd
        run: "echo seed"
    outputs: "{{ steps.seed.stdout }}"
  work:
    steps:
      - name: tick
        type: cmd
        run: "echo tick"
      - name: maybe_stop
        type: gate
        condition: "false"
        on_pass: break
  per_item:
    steps:
      - name: touch
        type: cmd
        run: "echo {{ item }}"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();

        assert_eq!(wf.steps.len(), 3);

        let call = &wf.steps[0];
        assert_eq!(call.kind, stepyard_harness::StepKind::Call);
        assert_eq!(call.scope.as_deref(), Some("setup"));

        let rep = &wf.steps[1];
        assert_eq!(rep.kind, stepyard_harness::StepKind::Repeat);
        assert_eq!(rep.max_iterations, Some(3));
        assert_eq!(rep.initial_value, Some(serde_json::json!(0)));

        let map = &wf.steps[2];
        assert_eq!(map.kind, stepyard_harness::StepKind::Map);
        assert_eq!(map.items.as_deref(), Some("{{ steps.list.stdout }}"));
        assert_eq!(map.parallel, Some(2));

        let setup = wf.scopes.get("setup").unwrap();
        assert_eq!(setup.outputs.as_deref(), Some("{{ steps.seed.stdout }}"));

        // Scoped gate accepted `break` on on_pass.
        let work = wf.scopes.get("work").unwrap();
        let scoped_gate = &work.steps[1];
        assert_eq!(scoped_gate.kind, stepyard_harness::StepKind::Gate);
        assert_eq!(scoped_gate.on_pass.as_deref(), Some("break"));
    }

    #[test]
    fn container_requires_scope_field() {
        for kind in ["call", "repeat", "map"] {
            let yaml = format!(
                r#"
name: no-scope-{kind}
steps:
  - name: c
    type: {kind}
{extra}
"#,
                extra = if kind == "map" {
                    "    items: \"a,b\""
                } else {
                    ""
                }
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let err = adapt(&def).unwrap_err();
            assert!(
                matches!(err, AdapterError::ContainerMissingScope { kind: k, .. } if k == kind),
                "kind={kind} got {err:?}"
            );
        }
    }

    #[test]
    fn container_with_unknown_scope_is_rejected() {
        let yaml = r#"
name: unknown-scope
steps:
  - name: c
    type: call
    scope: ghost
scopes:
  real:
    steps:
      - name: s
        type: cmd
        run: "true"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::UnknownScope { kind: "call", ref scope, .. } if scope == "ghost"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn map_without_items_is_rejected() {
        let yaml = r#"
name: map-no-items
steps:
  - name: fan
    type: map
    scope: body
scopes:
  body:
    steps:
      - name: s
        type: cmd
        run: "true"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(err, AdapterError::MapMissingItems { ref name } if name == "fan"),
            "got {err:?}"
        );
    }

    #[test]
    fn nested_container_inside_scope_is_rejected() {
        for nested in ["call", "repeat", "map"] {
            let yaml = format!(
                r#"
name: nested-{nested}
steps:
  - name: outer
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: inner
        type: {nested}
        scope: other
{extra}
  other:
    steps:
      - name: s
        type: cmd
        run: "true"
"#,
                extra = if nested == "map" {
                    "        items: \"a\""
                } else {
                    ""
                }
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let err = adapt(&def).unwrap_err();
            assert!(
                matches!(
                    err,
                    AdapterError::NestedScopesNotSupported { kind: k, .. } if k == nested
                ),
                "nested={nested} got {err:?}"
            );
        }
    }

    #[test]
    fn scoped_gate_accepts_skip_and_break() {
        for (field, value) in [
            ("on_pass", "skip"),
            ("on_pass", "break"),
            ("on_fail", "skip"),
            ("on_fail", "break"),
        ] {
            let yaml = format!(
                r#"
name: scoped-gate-{value}-{field}
steps:
  - name: run
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: g
        type: gate
        condition: "true"
        {field}: {value}
"#
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let wf = adapt(&def).expect("scoped gate with skip/break should adapt");
            let scope = wf.scopes.get("body").unwrap();
            let g = &scope.steps[0];
            match field {
                "on_pass" => assert_eq!(g.on_pass.as_deref(), Some(value)),
                "on_fail" => assert_eq!(g.on_fail.as_deref(), Some(value)),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn scoped_gate_rejects_unknown_action() {
        let yaml = r#"
name: scoped-gate-garbage
steps:
  - name: run
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: g
        type: gate
        condition: "true"
        on_pass: explode
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ScopedGateUnsupportedAction {
                    action: "on_pass",
                    ref value,
                    ..
                } if value == "explode"
            ),
            "got {err:?}"
        );
    }
}
