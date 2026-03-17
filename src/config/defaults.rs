use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::workflow::schema::WorkflowConfig;

/// Embedded defaults — compiled into the binary.
/// Works on every OS, no external files needed.
/// A new user doing `cargo install minion-engine` gets these automatically.
const EMBEDDED_DEFAULTS_YAML: &str = r#"
global:
  timeout: 300s
  working_directory: "."

agent:
  command: claude
  model: claude-sonnet-4-20250514
  flags:
    - "-p"
    - "--output-format"
    - "stream-json"
  permissions: skip

chat:
  provider: anthropic
  model: claude-sonnet-4-20250514
  api_key_env: ANTHROPIC_API_KEY
  temperature: 0.2
  max_tokens: 4096

cmd:
  fail_on_error: true
  timeout: 60s
"#;

/// Parse embedded defaults once (cached via OnceLock for performance).
fn embedded_defaults() -> &'static WorkflowConfig {
    static DEFAULTS: OnceLock<WorkflowConfig> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        serde_yaml::from_str(EMBEDDED_DEFAULTS_YAML)
            .expect("BUG: embedded defaults YAML is invalid — this is a compile-time error")
    })
}

/// Config file search paths, in priority order (lowest → highest).
///
/// ```text
/// [embedded in binary]          ← always available (lowest)
/// ~/.minion/defaults.yaml       ← user-level overrides (optional)
/// .minion/config.yaml           ← project-level overrides (optional)
/// workflow.yaml config:         ← workflow-level
/// step inline config:           ← highest priority
/// ```
fn override_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. User-level: ~/.minion/defaults.yaml
    //    On Windows: C:\Users\<user>\.minion\defaults.yaml
    //    On macOS:   /Users/<user>/.minion/defaults.yaml
    //    On Linux:   /home/<user>/.minion/defaults.yaml
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".minion").join("defaults.yaml"));
    }

    // 2. Project-level: .minion/config.yaml (relative to CWD)
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".minion").join("config.yaml"));
    }

    paths
}

/// Load and parse a config YAML file.
/// Returns None if file doesn't exist or can't be parsed.
fn load_config_file(path: &Path) -> Option<WorkflowConfig> {
    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(path).ok()?;
    let config: WorkflowConfig = serde_yaml::from_str(&content)
        .map_err(|e| {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "Failed to parse config file — skipping"
            );
            e
        })
        .ok()?;

    tracing::info!(path = %path.display(), "Loaded config overrides");
    Some(config)
}

/// Merge two HashMaps: base values are overridden by overlay values.
fn merge_map(
    base: &HashMap<String, serde_yaml::Value>,
    overlay: &HashMap<String, serde_yaml::Value>,
) -> HashMap<String, serde_yaml::Value> {
    let mut merged = base.clone();
    for (k, v) in overlay {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// Merge two pattern maps: for each pattern key, merge inner values.
fn merge_patterns(
    base: &HashMap<String, HashMap<String, serde_yaml::Value>>,
    overlay: &HashMap<String, HashMap<String, serde_yaml::Value>>,
) -> HashMap<String, HashMap<String, serde_yaml::Value>> {
    let mut merged = base.clone();
    for (pattern, values) in overlay {
        let entry = merged.entry(pattern.clone()).or_default();
        for (k, v) in values {
            entry.insert(k.clone(), v.clone());
        }
    }
    merged
}

/// Merge two WorkflowConfigs: `overlay` values override `base` values.
/// Plugins and events from overlay replace base entirely (not merged).
pub fn merge_workflow_config(base: &WorkflowConfig, overlay: &WorkflowConfig) -> WorkflowConfig {
    WorkflowConfig {
        global: merge_map(&base.global, &overlay.global),
        agent: merge_map(&base.agent, &overlay.agent),
        cmd: merge_map(&base.cmd, &overlay.cmd),
        chat: merge_map(&base.chat, &overlay.chat),
        gate: merge_map(&base.gate, &overlay.gate),
        patterns: merge_patterns(&base.patterns, &overlay.patterns),
        plugins: if overlay.plugins.is_empty() {
            base.plugins.clone()
        } else {
            overlay.plugins.clone()
        },
        events: overlay.events.clone().or_else(|| base.events.clone()),
    }
}

/// Build the full defaults chain:
///   embedded → ~/.minion/defaults.yaml → .minion/config.yaml
///
/// Each layer overrides the previous one.
pub fn load_defaults() -> WorkflowConfig {
    let mut merged = embedded_defaults().clone();

    for path in override_config_paths() {
        if let Some(config) = load_config_file(&path) {
            merged = merge_workflow_config(&merged, &config);
        }
    }

    merged
}

/// Apply the full defaults chain under a workflow's config.
///
/// Priority (lowest → highest):
///   embedded → ~/.minion/defaults.yaml → .minion/config.yaml → workflow YAML
///
/// The workflow config always wins over defaults.
pub fn apply_defaults(workflow_config: &WorkflowConfig) -> WorkflowConfig {
    let defaults = load_defaults();
    merge_workflow_config(&defaults, workflow_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml_str(s: &str) -> serde_yaml::Value {
        serde_yaml::Value::String(s.to_string())
    }

    #[test]
    fn embedded_defaults_parse_correctly() {
        let defaults = embedded_defaults();
        // Chat model is set
        assert_eq!(
            defaults.chat.get("model").unwrap(),
            &yaml_str("claude-sonnet-4-20250514")
        );
        // Agent model is set
        assert_eq!(
            defaults.agent.get("model").unwrap(),
            &yaml_str("claude-sonnet-4-20250514")
        );
        // Provider is anthropic
        assert_eq!(
            defaults.chat.get("provider").unwrap(),
            &yaml_str("anthropic")
        );
        // Global timeout is set
        assert_eq!(
            defaults.global.get("timeout").unwrap(),
            &yaml_str("300s")
        );
        // Cmd fail_on_error is set
        assert!(defaults.cmd.contains_key("fail_on_error"));
    }

    #[test]
    fn merge_map_overlay_wins() {
        let mut base = HashMap::new();
        base.insert("model".into(), yaml_str("claude-3-haiku"));
        base.insert("timeout".into(), yaml_str("300s"));

        let mut overlay = HashMap::new();
        overlay.insert("model".into(), yaml_str("claude-sonnet-4-20250514"));

        let merged = merge_map(&base, &overlay);
        assert_eq!(
            merged.get("model").unwrap(),
            &yaml_str("claude-sonnet-4-20250514")
        );
        assert_eq!(merged.get("timeout").unwrap(), &yaml_str("300s"));
    }

    #[test]
    fn merge_map_preserves_base_when_no_overlay() {
        let mut base = HashMap::new();
        base.insert("key".into(), yaml_str("val"));

        let overlay = HashMap::new();
        let merged = merge_map(&base, &overlay);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged.get("key").unwrap(), &yaml_str("val"));
    }

    #[test]
    fn merge_patterns_combines() {
        let mut base_patterns = HashMap::new();
        let mut base_inner = HashMap::new();
        base_inner.insert("model".into(), yaml_str("haiku"));
        base_patterns.insert("lint.*".into(), base_inner);

        let mut overlay_patterns = HashMap::new();
        let mut overlay_inner = HashMap::new();
        overlay_inner.insert("timeout".into(), yaml_str("10s"));
        overlay_patterns.insert("test.*".into(), overlay_inner);

        let merged = merge_patterns(&base_patterns, &overlay_patterns);
        assert!(merged.contains_key("lint.*"));
        assert!(merged.contains_key("test.*"));
    }

    #[test]
    fn merge_workflow_config_full() {
        let base = WorkflowConfig {
            global: {
                let mut m = HashMap::new();
                m.insert("timeout".into(), yaml_str("300s"));
                m
            },
            chat: {
                let mut m = HashMap::new();
                m.insert("model".into(), yaml_str("claude-3-haiku"));
                m.insert("provider".into(), yaml_str("anthropic"));
                m
            },
            ..Default::default()
        };

        let overlay = WorkflowConfig {
            chat: {
                let mut m = HashMap::new();
                m.insert("model".into(), yaml_str("claude-sonnet-4-20250514"));
                m.insert("temperature".into(), yaml_str("0.1"));
                m
            },
            ..Default::default()
        };

        let merged = merge_workflow_config(&base, &overlay);
        assert_eq!(merged.global.get("timeout").unwrap(), &yaml_str("300s"));
        assert_eq!(
            merged.chat.get("model").unwrap(),
            &yaml_str("claude-sonnet-4-20250514")
        );
        assert_eq!(merged.chat.get("provider").unwrap(), &yaml_str("anthropic"));
        assert_eq!(merged.chat.get("temperature").unwrap(), &yaml_str("0.1"));
    }

    #[test]
    fn apply_defaults_always_provides_embedded_values() {
        // Even with an empty workflow config, embedded defaults apply
        let config = WorkflowConfig::default();
        let result = apply_defaults(&config);

        // Model comes from embedded defaults
        assert_eq!(
            result.chat.get("model").unwrap(),
            &yaml_str("claude-sonnet-4-20250514")
        );
        assert_eq!(
            result.chat.get("provider").unwrap(),
            &yaml_str("anthropic")
        );
        assert_eq!(
            result.agent.get("model").unwrap(),
            &yaml_str("claude-sonnet-4-20250514")
        );
    }

    #[test]
    fn workflow_config_overrides_embedded_defaults() {
        let config = WorkflowConfig {
            chat: {
                let mut m = HashMap::new();
                m.insert("model".into(), yaml_str("claude-3-haiku-20240307"));
                m.insert("temperature".into(), yaml_str("0.9"));
                m
            },
            ..Default::default()
        };

        let result = apply_defaults(&config);

        // Workflow model overrides embedded default
        assert_eq!(
            result.chat.get("model").unwrap(),
            &yaml_str("claude-3-haiku-20240307")
        );
        // Workflow temperature overrides embedded default
        assert_eq!(
            result.chat.get("temperature").unwrap(),
            &yaml_str("0.9")
        );
        // Provider still comes from embedded (workflow didn't set it)
        assert_eq!(
            result.chat.get("provider").unwrap(),
            &yaml_str("anthropic")
        );
    }

    #[test]
    fn parse_user_override_yaml() {
        let yaml = r#"
chat:
  model: claude-opus-4-20250514
  temperature: 0.0
"#;
        let config: WorkflowConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.chat.get("model").unwrap(),
            &yaml_str("claude-opus-4-20250514")
        );
        // Only chat is set — other sections empty
        assert!(config.agent.is_empty());
        assert!(config.cmd.is_empty());
    }
}
