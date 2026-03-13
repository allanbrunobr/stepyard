pub mod agent;
pub mod call;
pub mod chat;
pub mod cmd;
pub mod gate;
pub mod map;
pub mod parallel;
pub mod repeat;
pub mod template_step;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::sandbox::docker::DockerSandbox;
use crate::workflow::schema::StepDef;

/// Shared reference to a Docker sandbox (None when sandbox is disabled)
pub type SharedSandbox = Option<Arc<Mutex<DockerSandbox>>>;

/// Trait that each step type implements
#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn execute(
        &self,
        step_def: &StepDef,
        config: &StepConfig,
        context: &Context,
    ) -> Result<StepOutput, StepError>;
}

/// Extended trait for executors that can run inside a sandbox
#[async_trait]
pub trait SandboxAwareExecutor: Send + Sync {
    async fn execute_sandboxed(
        &self,
        step_def: &StepDef,
        config: &StepConfig,
        context: &Context,
        sandbox: &SharedSandbox,
    ) -> Result<StepOutput, StepError>;
}

/// Result of any executed step
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepOutput {
    Cmd(CmdOutput),
    Agent(AgentOutput),
    Chat(ChatOutput),
    Gate(GateOutput),
    Scope(ScopeOutput),
    Empty,
}

impl StepOutput {
    /// Main text of the output
    pub fn text(&self) -> &str {
        match self {
            StepOutput::Cmd(o) => &o.stdout,
            StepOutput::Agent(o) => &o.response,
            StepOutput::Chat(o) => &o.response,
            StepOutput::Gate(o) => o.message.as_deref().unwrap_or(""),
            StepOutput::Scope(o) => o
                .final_value
                .as_ref()
                .map(|v| v.text())
                .unwrap_or(""),
            StepOutput::Empty => "",
        }
    }

    /// Exit code (only meaningful for cmd, 0 for others)
    pub fn exit_code(&self) -> i32 {
        match self {
            StepOutput::Cmd(o) => o.exit_code,
            _ => 0,
        }
    }

    /// Whether the step succeeded
    pub fn success(&self) -> bool {
        match self {
            StepOutput::Cmd(o) => o.exit_code == 0,
            StepOutput::Gate(o) => o.passed,
            _ => true,
        }
    }

    /// Split text into lines
    pub fn lines(&self) -> Vec<&str> {
        self.text()
            .lines()
            .filter(|l| !l.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    #[serde(
        serialize_with = "serialize_duration",
        deserialize_with = "deserialize_duration"
    )]
    pub duration: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutput {
    pub response: String,
    pub session_id: Option<String>,
    pub stats: AgentStats,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentStats {
    #[serde(
        serialize_with = "serialize_duration",
        deserialize_with = "deserialize_duration"
    )]
    pub duration: Duration,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatOutput {
    pub response: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateOutput {
    pub passed: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeOutput {
    pub iterations: Vec<IterationOutput>,
    pub final_value: Option<Box<StepOutput>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationOutput {
    pub index: usize,
    pub output: StepOutput,
}

fn serialize_duration<S>(d: &Duration, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_f64(d.as_secs_f64())
}

fn deserialize_duration<'de, D>(d: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let secs = f64::deserialize(d)?;
    Ok(Duration::from_secs_f64(secs))
}
