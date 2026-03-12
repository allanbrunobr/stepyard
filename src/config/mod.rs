use std::collections::HashMap;
use std::time::Duration;

use crate::workflow::schema::{StepType, WorkflowConfig};

/// Resolved configuration for a specific step (after 4-layer merge)
#[derive(Debug, Clone, Default)]
pub struct StepConfig {
    pub values: HashMap<String, serde_json::Value>,
}

impl StepConfig {
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.values.get(key).and_then(|v| v.as_str())
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.values
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    pub fn get_duration(&self, key: &str) -> Option<Duration> {
        let s = self.get_str(key)?;
        parse_duration(s)
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.values.get(key).and_then(|v| v.as_u64())
    }
}

fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        secs.trim().parse::<u64>().ok().map(Duration::from_secs)
    } else if let Some(ms) = s.strip_suffix("ms") {
        ms.trim().parse::<u64>().ok().map(Duration::from_millis)
    } else if let Some(mins) = s.strip_suffix('m') {
        mins.trim()
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60))
    } else {
        s.parse::<u64>().ok().map(Duration::from_secs)
    }
}

/// Convert serde_yaml::Value → serde_json::Value
fn yaml_to_json(v: &serde_yaml::Value) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

/// Manages 4-layer config resolution
pub struct ConfigManager {
    config: WorkflowConfig,
}

impl ConfigManager {
    pub fn new(config: WorkflowConfig) -> Self {
        Self { config }
    }

    /// Resolve config for a step by merging 4 layers:
    /// global < type < pattern < step inline
    pub fn resolve(
        &self,
        step_name: &str,
        step_type: &StepType,
        step_inline: &HashMap<String, serde_yaml::Value>,
    ) -> StepConfig {
        let mut merged = HashMap::new();

        // Layer 1: global
        for (k, v) in &self.config.global {
            merged.insert(k.clone(), yaml_to_json(v));
        }

        // Layer 2: by step type
        let type_config = match step_type {
            StepType::Agent => &self.config.agent,
            StepType::Cmd => &self.config.cmd,
            StepType::Chat => &self.config.chat,
            StepType::Gate => &self.config.gate,
            _ => &HashMap::new(),
        };
        for (k, v) in type_config {
            merged.insert(k.clone(), yaml_to_json(v));
        }

        // Layer 3: by pattern match on step name
        for (pattern, values) in &self.config.patterns {
            if let Ok(re) = regex::Regex::new(pattern) {
                if re.is_match(step_name) {
                    for (k, v) in values {
                        merged.insert(k.clone(), yaml_to_json(v));
                    }
                }
            }
        }

        // Layer 4: step inline (highest priority)
        for (k, v) in step_inline {
            merged.insert(k.clone(), yaml_to_json(v));
        }

        StepConfig { values: merged }
    }
}
