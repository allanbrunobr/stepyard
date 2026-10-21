use std::collections::HashMap;

use crate::config::merge::yaml_to_json;
use crate::config::StepConfig;
use crate::workflow::schema::{StepType, WorkflowConfig};

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
        let empty = HashMap::new();
        let type_config = match step_type {
            StepType::Agent => &self.config.agent,
            StepType::Cmd => &self.config.cmd,
            StepType::Chat => &self.config.chat,
            StepType::Gate => &self.config.gate,
            _ => &empty,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn yaml_str(s: &str) -> serde_yaml::Value {
        serde_yaml::Value::String(s.to_string())
    }

    #[test]
    fn test_global_timeout_overridden_by_step_inline() {
        let mut global = HashMap::new();
        global.insert("timeout".to_string(), yaml_str("300s"));

        let config = WorkflowConfig {
            global,
            ..Default::default()
        };
        let manager = ConfigManager::new(config);

        let mut inline = HashMap::new();
        inline.insert("timeout".to_string(), yaml_str("10s"));

        let resolved = manager.resolve("any_step", &StepType::Agent, &inline);
        assert_eq!(
            resolved.get_duration("timeout"),
            Some(Duration::from_secs(10)),
            "step inline timeout=10s should override global timeout=300s"
        );
    }

    #[test]
    fn test_pattern_match_sets_model() {
        let mut pattern_values = HashMap::new();
        pattern_values.insert("model".to_string(), yaml_str("claude-3-haiku"));

        let mut patterns = HashMap::new();
        patterns.insert("lint.*".to_string(), pattern_values);

        let config = WorkflowConfig {
            patterns,
            ..Default::default()
        };
        let manager = ConfigManager::new(config);

        let resolved = manager.resolve("lint_check", &StepType::Agent, &HashMap::new());
        assert_eq!(
            resolved.get_str("model"),
            Some("claude-3-haiku"),
            "pattern 'lint.*' should match step name 'lint_check'"
        );
    }

    #[test]
    fn test_pattern_no_match() {
        let mut pattern_values = HashMap::new();
        pattern_values.insert("model".to_string(), yaml_str("claude-3-haiku"));

        let mut patterns = HashMap::new();
        patterns.insert("lint.*".to_string(), pattern_values);

        let config = WorkflowConfig {
            patterns,
            ..Default::default()
        };
        let manager = ConfigManager::new(config);

        let resolved = manager.resolve("test_run", &StepType::Agent, &HashMap::new());
        assert_eq!(
            resolved.get_str("model"),
            None,
            "pattern 'lint.*' should NOT match step name 'test_run'"
        );
    }
}
