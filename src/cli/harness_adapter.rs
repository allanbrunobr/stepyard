//! Bridge between the legacy [`WorkflowDef`] (with all 9 step types) and the
//! narrower [`stepyard_harness::Workflow`] (cmd + gate for PR 2 of #31).
//!
//! Story 2.4 shipped the v2 engine path behind `--engine v2` with a cmd-only
//! adapter. PR 2 of Task #31 widens the accept list to include `gate`,
//! carrying `condition` / `on_pass` / `on_fail` / `message` through to the
//! harness; the remaining 7 kinds still reject. Keeping this adapter in the
//! CLI crate (instead of inside `stepyard-harness`) preserves the invariant
//! that the harness knows nothing about legacy YAML shapes.

use stepyard_harness::{Step, Workflow};

use crate::workflow::schema::{StepType, WorkflowDef};

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error(
        "step type `{step_type}` not yet supported by v2 engine — use --engine v1 \
         or migrate the workflow to cmd-only steps"
    )]
    UnsupportedStepType { step_type: StepType },

    #[error("step `{name}` has type cmd but no `run:` field")]
    CmdMissingRun { name: String },

    #[error("gate step `{name}` is missing `condition:`")]
    GateMissingCondition { name: String },

    #[error(
        "gate step `{name}` action `{action}=\"{value}\"` is not supported in \
         PR 2 of #31 (allowed: continue, fail; `break` / `skip` land with \
         repeat/map/call in PR 3)"
    )]
    GateUnsupportedAction {
        name: String,
        action: &'static str,
        value: String,
    },
}

/// Convert a parsed [`WorkflowDef`] into the harness-facing [`Workflow`].
///
/// Carries both workflow-level (`def.env`) and step-level (`s.env`) env maps
/// through to the harness types so the v2 engine's cascade resolver (Story 3.4)
/// can merge them. Without this the YAML `env:` fields parse but never reach
/// the engine — a silent drop.
pub fn adapt(def: &WorkflowDef) -> Result<Workflow, AdapterError> {
    let mut steps = Vec::with_capacity(def.steps.len());
    for s in &def.steps {
        match &s.step_type {
            StepType::Cmd => {
                let cmd = s
                    .run
                    .clone()
                    .ok_or_else(|| AdapterError::CmdMissingRun {
                        name: s.name.clone(),
                    })?;
                steps.push(Step::cmd(s.name.clone(), cmd).with_env(s.env.clone()));
            }
            StepType::Gate => {
                let condition = s
                    .condition
                    .clone()
                    .filter(|c| !c.trim().is_empty())
                    .ok_or_else(|| AdapterError::GateMissingCondition {
                        name: s.name.clone(),
                    })?;
                // Reject `break` / `skip` here, at parse time, rather than
                // deep inside the engine's eval path — keeps the contract
                // honest: the CLI refuses gate actions that have no
                // executor behind them in PR 2 of #31.
                validate_gate_action(&s.name, "on_pass", s.on_pass.as_deref())?;
                validate_gate_action(&s.name, "on_fail", s.on_fail.as_deref())?;

                let mut step = Step::gate(s.name.clone(), condition);
                step.on_pass = s.on_pass.clone();
                step.on_fail = s.on_fail.clone();
                step.message = s.message.clone();
                step.env = s.env.clone();
                steps.push(step);
            }
            other => {
                return Err(AdapterError::UnsupportedStepType {
                    step_type: other.clone(),
                });
            }
        }
    }
    let mut wf = Workflow::new(def.name.clone(), steps);
    wf.env = def.env.clone();
    Ok(wf)
}

fn validate_gate_action(
    step_name: &str,
    field: &'static str,
    raw: Option<&str>,
) -> Result<(), AdapterError> {
    match raw.map(str::trim) {
        None | Some("") | Some("continue") | Some("fail") => Ok(()),
        Some(other) => Err(AdapterError::GateUnsupportedAction {
            name: step_name.to_string(),
            action: field,
            value: other.to_string(),
        }),
    }
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
    fn rejects_gate_break_and_skip_on_pass_and_on_fail() {
        // Explicit coverage per PR 2 scope: the contract must refuse any
        // gate action string that has no executor yet, on either branch.
        for (field, value) in [
            ("on_pass", "break"),
            ("on_pass", "skip"),
            ("on_fail", "break"),
            ("on_fail", "skip"),
        ] {
            let yaml = format!(
                r#"
name: adapter-gate-bad-action
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
            let msg = err.to_string();
            assert!(msg.contains(value), "msg={msg}");
            assert!(msg.contains(field), "msg={msg}");
            assert!(msg.contains("PR 3"), "msg={msg}");
        }
    }

    #[test]
    fn rejects_still_unsupported_step_type() {
        let yaml = r#"
name: adapter-reject-repeat
steps:
  - name: looper
    type: repeat
    scope: inner
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not yet supported"), "msg={msg}");
        assert!(msg.contains("repeat"), "msg={msg}");
    }
}
