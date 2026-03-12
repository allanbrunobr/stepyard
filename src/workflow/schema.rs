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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepType {
    Cmd,
    Agent,
    Chat,
    Gate,
    Repeat,
    Map,
    Parallel,
    Call,
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
