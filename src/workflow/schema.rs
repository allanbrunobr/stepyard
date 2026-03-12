use std::collections::HashMap;

use serde::Deserialize;

/// Top-level workflow definition
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowDef {
    pub name: String,
    #[serde(default)]
    pub version: u32,
    pub description: Option<String>,
    #[serde(default)]
    pub config: WorkflowConfig,
    pub prompts_dir: Option<String>,
    #[serde(default)]
    pub scopes: HashMap<String, ScopeDef>,
    pub steps: Vec<StepDef>,
}

/// Config block with 4 layers
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkflowConfig {
    #[serde(default)]
    pub global: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub agent: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub cmd: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub chat: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub gate: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub patterns: HashMap<String, HashMap<String, serde_yaml::Value>>,
}

/// Named scope (sub-workflow)
#[derive(Debug, Clone, Deserialize)]
pub struct ScopeDef {
    pub steps: Vec<StepDef>,
    pub outputs: Option<String>,
}

/// Individual step definition
#[derive(Debug, Clone, Deserialize)]
pub struct StepDef {
    pub name: String,
    #[serde(rename = "type")]
    pub step_type: StepType,

    // cmd fields
    pub run: Option<String>,

    // agent/chat fields
    pub prompt: Option<String>,

    // gate fields
    pub condition: Option<String>,
    pub on_pass: Option<String>,
    pub on_fail: Option<String>,
    pub message: Option<String>,

    // repeat/map/call fields
    pub scope: Option<String>,
    pub max_iterations: Option<usize>,
    pub initial_value: Option<serde_yaml::Value>,

    // map fields
    pub items: Option<String>,
    pub parallel: Option<usize>,

    // parallel step fields (nested steps)
    pub steps: Option<Vec<StepDef>>,

    // per-step config override
    #[serde(default)]
    pub config: HashMap<String, serde_yaml::Value>,

    // scope output
    pub outputs: Option<String>,
}

/// All supported step types in a workflow.
///
/// Each variant corresponds to a `type:` value in the YAML step definition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    /// Execute a shell command via `/bin/sh -c`.
    Cmd,
    /// Invoke the Claude Code CLI and parse streaming JSON output.
    Agent,
    /// Call the Anthropic (or OpenAI-compatible) API directly.
    Chat,
    /// Evaluate a Tera boolean expression and branch control flow.
    Gate,
    /// Run a named scope repeatedly until break or max_iterations.
    Repeat,
    /// Run a named scope once per item in a comma-separated list.
    Map,
    /// Run nested steps concurrently.
    Parallel,
    /// Invoke a named scope once (no looping).
    Call,
    /// Render a prompt template file and store the result.
    Template,
}

impl std::fmt::Display for StepType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepType::Cmd => write!(f, "cmd"),
            StepType::Agent => write!(f, "agent"),
            StepType::Chat => write!(f, "chat"),
            StepType::Gate => write!(f, "gate"),
            StepType::Repeat => write!(f, "repeat"),
            StepType::Map => write!(f, "map"),
            StepType::Parallel => write!(f, "parallel"),
            StepType::Call => write!(f, "call"),
            StepType::Template => write!(f, "template"),
        }
    }
}
