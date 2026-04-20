//! Workflow representation for the harness.
//!
//! Story 2.3 landed a cmd-only shape. PR 1 of Task #31 (v2 migration) widens
//! the data model to represent every step kind plus named scopes so the
//! harness can parse and round-trip the full legacy shape. **Execution
//! remains cmd-only** — the CLI adapter (`src/cli/harness_adapter.rs`)
//! still rejects non-cmd step types before they reach [`crate::Engine`].
//! Executors for the non-cmd kinds land in subsequent PRs.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Every step kind the harness can represent.
///
/// Only [`StepKind::Cmd`] is executable today. The remaining variants exist
/// so workflow YAML carrying them survives deserialization and round-trips
/// intact; the adapter rejects them at the CLI boundary until follow-up
/// PRs wire each executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    #[default]
    Cmd,
    Agent,
    Chat,
    Gate,
    Repeat,
    Map,
    Parallel,
    Call,
    Template,
    Script,
}

impl StepKind {
    fn is_cmd(&self) -> bool {
        matches!(self, StepKind::Cmd)
    }
}

impl std::fmt::Display for StepKind {
    /// Renders the same snake_case label the engine writes to
    /// `step_type` on emitted events, so display/log/CLI can share one
    /// source of truth.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StepKind::Cmd => "cmd",
            StepKind::Agent => "agent",
            StepKind::Chat => "chat",
            StepKind::Gate => "gate",
            StepKind::Repeat => "repeat",
            StepKind::Map => "map",
            StepKind::Parallel => "parallel",
            StepKind::Call => "call",
            StepKind::Template => "template",
            StepKind::Script => "script",
        };
        f.write_str(s)
    }
}

/// A complete workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub steps: Vec<Step>,
    /// Workflow-level env vars. Merged below step-level env and above
    /// `.stepyard/defaults.yaml` in the cascade resolver (Story 3.4).
    /// `#[serde(default)]` preserves backward compat for YAML without an
    /// `env:` field (NFR18, NFR22).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Named step groups referenced by repeat/map/call kinds. The harness
    /// stores and round-trips them; execution wires up in a follow-up PR
    /// of the v2 migration. `#[serde(default)]` keeps existing cmd-only
    /// YAML without a `scopes:` block parseable.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scopes: HashMap<String, Scope>,
}

impl Workflow {
    pub fn new(name: impl Into<String>, steps: Vec<Step>) -> Self {
        Self {
            name: name.into(),
            steps,
            env: HashMap::new(),
            scopes: HashMap::new(),
        }
    }
}

/// A named sub-sequence of steps, referenced by repeat/map/call kinds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    pub steps: Vec<Step>,
}

/// One step in a workflow. Only [`StepKind::Cmd`] is executed today; other
/// kinds round-trip through serde but are rejected by the CLI adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    /// Step kind. Controls which executor the engine dispatches to. YAML
    /// field name is `type` to match the legacy workflow schema. Absent
    /// `type:` defaults to [`StepKind::Cmd`] so existing cmd-only YAML
    /// keeps deserializing unchanged.
    #[serde(
        default,
        rename = "type",
        skip_serializing_if = "StepKind::is_cmd"
    )]
    pub kind: StepKind,
    /// Shell command for [`StepKind::Cmd`] steps. Empty for other kinds
    /// until their executors land.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Wall-clock step timeout in milliseconds. Absent = no timeout.
    /// YAML field name is `timeout` to match the Story 1.4 workflow schema.
    #[serde(rename = "timeout", default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Step-level env vars. Highest precedence in the cascade resolver
    /// (Story 3.4) — overrides workflow, defaults, and host `${VAR}`.
    /// `#[serde(default)]` keeps existing YAML without `env:` parseable
    /// (NFR18, NFR22).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Tera template evaluated by [`StepKind::Gate`] steps to decide the
    /// pass/fail branch. PR 2 of Task #31. Other kinds leave this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Gate action when `condition` evaluates truthy. PR 2 only accepts
    /// `"continue"` and `"fail"`; `"break"` and `"skip"` land with
    /// repeat/map/call semantics in PR 3 and are rejected by the adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_pass: Option<String>,
    /// Gate action when `condition` evaluates falsy. Same value space as
    /// `on_pass`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_fail: Option<String>,
    /// Operator-visible message attached to the gate's terminal outcome
    /// (displayed in Dashboard and CLI). Plain string, no templating in
    /// PR 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Step {
    pub fn cmd(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: StepKind::Cmd,
            command: command.into(),
            timeout: None,
            env: HashMap::new(),
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
        }
    }

    /// Constructor for a [`StepKind::Gate`] step. PR 2 of Task #31.
    pub fn gate(name: impl Into<String>, condition: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: StepKind::Gate,
            command: String::new(),
            timeout: None,
            env: HashMap::new(),
            condition: Some(condition.into()),
            on_pass: None,
            on_fail: None,
            message: None,
        }
    }

    /// Builder variant that attaches a wall-clock timeout (milliseconds).
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout = Some(timeout_ms);
        self
    }

    /// Builder variant that sets step-level env vars (used by Story 3.4's
    /// cascade resolver).
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backward_compat_cmd_only_yaml_still_parses() {
        let yaml = r#"
name: legacy
steps:
  - name: one
    command: "echo 1"
  - name: two
    command: "echo 2"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.name, "legacy");
        assert_eq!(wf.steps.len(), 2);
        assert_eq!(wf.steps[0].kind, StepKind::Cmd);
        assert!(wf.scopes.is_empty());
    }

    #[test]
    fn non_cmd_step_kind_deserializes() {
        let yaml = r#"
name: scaffold
steps:
  - name: check
    type: gate
  - name: loop
    type: repeat
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.steps[0].kind, StepKind::Gate);
        assert_eq!(wf.steps[1].kind, StepKind::Repeat);
    }

    #[test]
    fn scopes_block_deserializes() {
        let yaml = r#"
name: scaffold-scopes
steps:
  - name: enter
    command: "echo enter"
scopes:
  work:
    steps:
      - name: inner
        command: "echo inner"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        let scope = wf.scopes.get("work").expect("scope `work` missing");
        assert_eq!(scope.steps.len(), 1);
        assert_eq!(scope.steps[0].name, "inner");
        assert_eq!(scope.steps[0].kind, StepKind::Cmd);
    }

    #[test]
    fn cmd_kind_omitted_from_serialized_output() {
        let step = Step::cmd("s", "true");
        let yaml = serde_yaml::to_string(&step).unwrap();
        assert!(
            !yaml.contains("type:"),
            "cmd is the default; `type:` should be skipped: {yaml}"
        );
    }

    #[test]
    fn cmd_step_omits_gate_fields_from_serialized_output() {
        // Backward-compat: the four gate fields added in PR 2 of Task #31
        // must stay absent on the wire for cmd steps, otherwise existing
        // workflow round-trips would gain noise.
        let step = Step::cmd("s", "true");
        let yaml = serde_yaml::to_string(&step).unwrap();
        for field in ["condition:", "on_pass:", "on_fail:", "message:"] {
            assert!(
                !yaml.contains(field),
                "cmd step leaked `{field}` onto the wire: {yaml}"
            );
        }
    }

    #[test]
    fn gate_fields_roundtrip_through_yaml() {
        let yaml = r#"
name: with-gate
steps:
  - name: check
    type: gate
    condition: "{{ steps.one.exit_code }} == 0"
    on_pass: continue
    on_fail: fail
    message: "build must pass"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.kind, StepKind::Gate);
        assert_eq!(
            step.condition.as_deref(),
            Some("{{ steps.one.exit_code }} == 0")
        );
        assert_eq!(step.on_pass.as_deref(), Some("continue"));
        assert_eq!(step.on_fail.as_deref(), Some("fail"));
        assert_eq!(step.message.as_deref(), Some("build must pass"));
    }
}
