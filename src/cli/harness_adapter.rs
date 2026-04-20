//! Bridge between the legacy [`WorkflowDef`] (with all 9 step types) and the
//! narrower [`stepyard_harness::Workflow`] (cmd-only for Story 2.4).
//!
//! Story 2.4 ships the v2 engine path behind `--engine v2`. This adapter
//! accepts cmd-type steps and rejects everything else; the 9-type port is
//! Stories 2.6+. Keeping the adapter in the CLI crate (instead of inside
//! `stepyard-harness`) preserves the invariant that the harness knows nothing
//! about legacy YAML shapes.

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
    fn rejects_gate_step() {
        let yaml = r#"
name: adapter-reject
steps:
  - name: g
    type: gate
    condition: "true"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not yet supported"), "msg={msg}");
        assert!(msg.contains("gate"), "msg={msg}");
    }
}
