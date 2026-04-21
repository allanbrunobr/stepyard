//! Workflow representation for the harness.
//!
//! Story 2.3 landed a cmd-only shape. PR 1 of Task #31 (v2 migration) widened
//! the data model to represent every step kind plus named scopes so the
//! harness can parse and round-trip the full legacy shape. PR 2 added
//! gate-specific fields (`condition` / `on_pass` / `on_fail` / `message`);
//! PR 3 adds the container fields (`scope` / `max_iterations` / `items` /
//! `parallel` / `initial_value` / `outputs`) that `call` / `repeat` / `map`
//! need, plus the scope runner that executes them. **Executable kinds
//! today:** `cmd`, `gate`, `call`, `repeat`, `map`, `template`, `script`.
//! Other kinds (`agent` / `chat` / `parallel`) still round-trip through
//! serde but are rejected by the CLI adapter
//! (`src/cli/harness_adapter.rs`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Every step kind the harness can represent.
///
/// Executable today: [`StepKind::Cmd`], [`StepKind::Gate`], [`StepKind::Call`],
/// [`StepKind::Repeat`], [`StepKind::Map`], [`StepKind::Template`],
/// [`StepKind::Script`]. The remaining variants (`Agent` / `Chat` /
/// `Parallel`) round-trip through serde but are rejected at the CLI
/// adapter boundary until follow-up PRs wire each executor.
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
    /// Directory where `template` steps look up `<name>.md.tera` files.
    /// Interpreted relative to the harness process's working directory
    /// when it's not absolute. Absent = fall back to `"prompts"` (v1
    /// parity with `src/engine/context.rs:58`). PR 4 of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts_dir: Option<String>,
}

impl Workflow {
    pub fn new(name: impl Into<String>, steps: Vec<Step>) -> Self {
        Self {
            name: name.into(),
            steps,
            env: HashMap::new(),
            scopes: HashMap::new(),
            prompts_dir: None,
        }
    }
}

/// A named sub-sequence of steps, referenced by repeat/map/call kinds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    pub steps: Vec<Step>,
    /// Tera expression evaluated at scope completion to produce the
    /// container step's output snapshot (PR 3 of Task #31). Absent = the
    /// container falls back to the last step's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<String>,
}

/// One step in a workflow. Executable kinds are dispatched by
/// `crates/stepyard-harness/src/engine.rs`; unsupported kinds still
/// round-trip through serde but are rejected by the CLI adapter.
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
    /// Shell command for [`StepKind::Cmd`] steps and Rhai source for
    /// [`StepKind::Script`] steps — both map to the same legacy YAML field
    /// (`run:`). Empty for other kinds.
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

    /// Named scope referenced by `call` / `repeat` / `map` (PR 3 of Task
    /// #31). Required at the adapter boundary for those kinds; `None` for
    /// every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Safety cap for `repeat`: once the counter reaches this value the
    /// container completes cleanly with a warning (v1 parity, see
    /// v1 `repeat.rs:143`). `None` = no explicit cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
    /// Seed value exposed as `scope.value` for the first iteration/pass
    /// of `call` / `repeat` / `map` (v1 parity). Persisted as JSON so
    /// the harness stays YAML-agnostic — the adapter converts the YAML
    /// value once at parse time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_value: Option<serde_json::Value>,
    /// Tera expression rendered by `map` to produce the iteration
    /// collection. Required at the adapter boundary for `map`; `None`
    /// for every other kind. Shape of the rendered result is *not*
    /// enforced at adapt time — the scope runner preserves v1's
    /// "JSON array or line-split" heuristic (v1 `map.rs:237`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    /// Parallelism budget for `map`. Carried through for legacy-YAML
    /// round-trip fidelity; the scope runner in PR 3 executes
    /// sequentially even when this is `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<usize>,
    /// Override for the containing [`Scope::outputs`] template. `None`
    /// falls back to the scope-level value (if any) or to the last
    /// scope-body step's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<String>,
    /// [`StepKind::Template`] only: Tera expression rendered once to
    /// produce the prompt-file basename. Absent → fall back to
    /// `step.name`. Two-pass render matches v1
    /// `src/steps/template_step.rs:32-36`. PR 4 of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

impl Step {
    pub fn cmd(name: impl Into<String>, command: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Cmd);
        step.command = command.into();
        step
    }

    /// Constructor for a [`StepKind::Gate`] step. PR 2 of Task #31.
    pub fn gate(name: impl Into<String>, condition: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Gate);
        step.condition = Some(condition.into());
        step
    }

    /// Constructor for a [`StepKind::Call`] step — runs `scope` once.
    /// PR 3 of Task #31.
    pub fn call(name: impl Into<String>, scope: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Call);
        step.scope = Some(scope.into());
        step
    }

    /// Constructor for a [`StepKind::Repeat`] step — runs `scope` until a
    /// scope-body gate emits `break`, or `max_iterations` fires (v1
    /// parity with a completion warning).
    pub fn repeat(name: impl Into<String>, scope: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Repeat);
        step.scope = Some(scope.into());
        step
    }

    /// Constructor for a [`StepKind::Map`] step — runs `scope` once per
    /// item rendered from the `items` Tera expression.
    pub fn map(
        name: impl Into<String>,
        scope: impl Into<String>,
        items: impl Into<String>,
    ) -> Self {
        let mut step = Step::empty(name, StepKind::Map);
        step.scope = Some(scope.into());
        step.items = Some(items.into());
        step
    }

    /// Constructor for a [`StepKind::Template`] step — renders the
    /// `{prompts_dir}/{prompt or name}.md.tera` file against the current
    /// render context. PR 4 of Task #31.
    pub fn template(name: impl Into<String>, prompt: Option<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Template);
        step.prompt = prompt;
        step
    }

    /// Constructor for a [`StepKind::Script`] step — evaluates the Rhai
    /// `source` against a flat snapshot of the harness render context and
    /// emits the return value as `stdout`. PR 4 of Task #31.
    pub fn script(name: impl Into<String>, source: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Script);
        step.command = source.into();
        step
    }

    fn empty(name: impl Into<String>, kind: StepKind) -> Self {
        Self {
            name: name.into(),
            kind,
            command: String::new(),
            timeout: None,
            env: HashMap::new(),
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: None,
            max_iterations: None,
            initial_value: None,
            items: None,
            parallel: None,
            outputs: None,
            prompt: None,
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

    #[test]
    fn container_fields_roundtrip_through_yaml() {
        let yaml = r#"
name: containers
steps:
  - name: once
    type: call
    scope: setup
  - name: loop
    type: repeat
    scope: work
    max_iterations: 5
    initial_value: 0
  - name: fan
    type: map
    scope: per_file
    items: "{{ steps.list.stdout }}"
    parallel: 4
    initial_value:
      seed: ok
      tries: 3
scopes:
  setup:
    steps:
      - name: seed
        command: "echo seed"
    outputs: "{{ steps.seed.stdout }}"
  work:
    steps:
      - name: tick
        command: "echo tick"
  per_file:
    steps:
      - name: touch
        command: "echo {{ item }}"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();

        let call = &wf.steps[0];
        assert_eq!(call.kind, StepKind::Call);
        assert_eq!(call.scope.as_deref(), Some("setup"));

        let rep = &wf.steps[1];
        assert_eq!(rep.kind, StepKind::Repeat);
        assert_eq!(rep.max_iterations, Some(5));
        assert_eq!(rep.initial_value, Some(serde_json::json!(0)));

        let map = &wf.steps[2];
        assert_eq!(map.kind, StepKind::Map);
        assert_eq!(map.items.as_deref(), Some("{{ steps.list.stdout }}"));
        assert_eq!(map.parallel, Some(4));
        assert_eq!(
            map.initial_value,
            Some(serde_json::json!({ "seed": "ok", "tries": 3 }))
        );

        let setup = wf.scopes.get("setup").unwrap();
        assert_eq!(setup.outputs.as_deref(), Some("{{ steps.seed.stdout }}"));
        assert!(wf.scopes.get("work").unwrap().outputs.is_none());
    }

    #[test]
    fn cmd_step_omits_container_fields_from_serialized_output() {
        // Backward-compat: the six container fields added in PR 3 of #31
        // must stay absent on the wire for cmd and gate steps, otherwise
        // existing workflow round-trips would gain noise.
        let step = Step::cmd("s", "true");
        let yaml = serde_yaml::to_string(&step).unwrap();
        for field in [
            "scope:",
            "max_iterations:",
            "initial_value:",
            "items:",
            "parallel:",
            "outputs:",
        ] {
            assert!(
                !yaml.contains(field),
                "cmd step leaked `{field}` onto the wire: {yaml}"
            );
        }
    }

    #[test]
    fn container_constructors_roundtrip_through_serde() {
        let call = Step::call("once", "setup");
        let rep = Step::repeat("loop", "work");
        let map = Step::map("fan", "per_file", "{{ steps.list.stdout }}");

        let wf = Workflow::new("ctor", vec![call, rep, map]);
        let yaml = serde_yaml::to_string(&wf).unwrap();
        let back: Workflow = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(back.steps[0].kind, StepKind::Call);
        assert_eq!(back.steps[0].scope.as_deref(), Some("setup"));
        assert_eq!(back.steps[1].kind, StepKind::Repeat);
        assert_eq!(back.steps[1].scope.as_deref(), Some("work"));
        assert_eq!(back.steps[2].kind, StepKind::Map);
        assert_eq!(
            back.steps[2].items.as_deref(),
            Some("{{ steps.list.stdout }}")
        );
    }
}
