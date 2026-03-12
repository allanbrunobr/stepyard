// Sandbox config API — some items used only in integration paths
#![allow(dead_code)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Network policy for sandbox (allow/deny domain lists)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Domains/IPs to allow (empty = allow all)
    pub allow: Vec<String>,
    /// Domains/IPs to deny
    pub deny: Vec<String>,
}

/// Resource limits for sandbox container
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceConfig {
    /// Number of CPUs (e.g., 2.0)
    pub cpus: Option<f64>,
    /// Memory limit (e.g., "2g", "512m")
    pub memory: Option<String>,
}

/// Sandbox mode
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SandboxMode {
    /// No sandbox
    #[default]
    Disabled,
    /// CLI --sandbox flag: wrap entire workflow execution
    FullWorkflow,
    /// config.agent.sandbox: true — only agent steps run in sandbox
    AgentOnly,
    /// config.global.sandbox.enabled: true — devbox with full config
    Devbox,
}

/// Full sandbox configuration (parsed from workflow config or CLI flags)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub enabled: bool,
    /// Docker image to use (default: "ubuntu:22.04")
    pub image: Option<String>,
    /// Host path to mount as workspace inside container
    pub workspace: Option<String>,
    pub network: NetworkConfig,
    pub resources: ResourceConfig,
}

impl SandboxConfig {
    /// Default image used when none is specified
    pub const DEFAULT_IMAGE: &'static str = "ubuntu:22.04";

    pub fn image(&self) -> &str {
        self.image.as_deref().unwrap_or(Self::DEFAULT_IMAGE)
    }

    /// Parse SandboxConfig from a global config map (Devbox mode)
    pub fn from_global_config(config: &HashMap<String, serde_yaml::Value>) -> Self {
        let sandbox = match config.get("sandbox") {
            Some(serde_yaml::Value::Mapping(m)) => m,
            _ => return Self::default(),
        };

        let enabled = sandbox
            .get(serde_yaml::Value::String("enabled".into()))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let image = sandbox
            .get(serde_yaml::Value::String("image".into()))
            .and_then(|v| v.as_str())
            .map(String::from);

        let workspace = sandbox
            .get(serde_yaml::Value::String("workspace".into()))
            .and_then(|v| v.as_str())
            .map(String::from);

        let (allow, deny) = match sandbox.get(serde_yaml::Value::String("network".into())) {
            Some(serde_yaml::Value::Mapping(net)) => {
                let allow = net
                    .get(serde_yaml::Value::String("allow".into()))
                    .and_then(|v| v.as_sequence())
                    .map(|seq| {
                        seq.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let deny = net
                    .get(serde_yaml::Value::String("deny".into()))
                    .and_then(|v| v.as_sequence())
                    .map(|seq| {
                        seq.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                (allow, deny)
            }
            _ => (vec![], vec![]),
        };

        let (cpus, memory) = match sandbox.get(serde_yaml::Value::String("resources".into())) {
            Some(serde_yaml::Value::Mapping(res)) => {
                let cpus = res
                    .get(serde_yaml::Value::String("cpus".into()))
                    .and_then(|v| v.as_f64());
                let memory = res
                    .get(serde_yaml::Value::String("memory".into()))
                    .and_then(|v| v.as_str())
                    .map(String::from);
                (cpus, memory)
            }
            _ => (None, None),
        };

        Self {
            enabled,
            image,
            workspace,
            network: NetworkConfig { allow, deny },
            resources: ResourceConfig { cpus, memory },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_image() {
        let cfg = SandboxConfig::default();
        assert_eq!(cfg.image(), "ubuntu:22.04");
    }

    #[test]
    fn custom_image_override() {
        let cfg = SandboxConfig {
            image: Some("node:20".to_string()),
            ..Default::default()
        };
        assert_eq!(cfg.image(), "node:20");
    }

    #[test]
    fn from_global_config_parses_fields() {
        let yaml = r#"
sandbox:
  enabled: true
  image: "rust:1.80"
  workspace: "/app"
  network:
    allow:
      - "api.anthropic.com"
    deny:
      - "0.0.0.0/0"
  resources:
    cpus: 2.0
    memory: "4g"
"#;
        let map: HashMap<String, serde_yaml::Value> = serde_yaml::from_str(yaml).unwrap();
        let cfg = SandboxConfig::from_global_config(&map);

        assert!(cfg.enabled);
        assert_eq!(cfg.image(), "rust:1.80");
        assert_eq!(cfg.workspace.as_deref(), Some("/app"));
        assert_eq!(cfg.network.allow, ["api.anthropic.com"]);
        assert_eq!(cfg.network.deny, ["0.0.0.0/0"]);
        assert_eq!(cfg.resources.cpus, Some(2.0));
        assert_eq!(cfg.resources.memory.as_deref(), Some("4g"));
    }

    #[test]
    fn from_global_config_empty_returns_default() {
        let map: HashMap<String, serde_yaml::Value> = HashMap::new();
        let cfg = SandboxConfig::from_global_config(&map);
        assert!(!cfg.enabled);
        assert!(cfg.image.is_none());
    }
}
