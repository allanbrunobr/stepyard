pub mod registry;

use std::collections::HashMap;

use async_trait::async_trait;

use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::steps::{StepOutput};
use crate::workflow::schema::StepDef;

/// Schema describing a plugin's configuration requirements
#[derive(Debug, Clone, Default)]
pub struct PluginConfigSchema {
    /// Fields that must be present in the step config
    pub required_fields: Vec<String>,
    /// Optional fields with their default values
    pub optional_fields: HashMap<String, serde_json::Value>,
}

/// Trait that all plugins must implement
#[async_trait]
pub trait PluginStep: Send + Sync {
    /// Unique name identifying this plugin (used as step type in YAML)
    fn name(&self) -> &str;

    /// Execute the plugin step
    async fn execute(
        &self,
        step_def: &StepDef,
        config: &StepConfig,
        context: &Context,
    ) -> Result<StepOutput, StepError>;

    /// Validate that the step config satisfies this plugin's requirements
    fn validate(&self, config: &StepConfig) -> Result<(), StepError>;

    /// Return the configuration schema for this plugin
    fn config_schema(&self) -> PluginConfigSchema;
}
