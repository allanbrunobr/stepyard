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
    /// Docker image to use (default: "minion-sandbox:latest")
    pub image: Option<String>,
    /// Host path to mount as workspace inside container
    pub workspace: Option<String>,
    pub network: NetworkConfig,
    pub resources: ResourceConfig,
    /// Host environment variables to forward into the container.
    /// Each entry is a variable name (e.g. "GH_TOKEN"); the value is read
    /// from the host environment at container-creation time.
    pub env: Vec<String>,
    /// Extra read-only volume mounts (host_path:container_path or host_path:container_path:mode).
    /// Tilde (~) is expanded to $HOME on the host.
    pub volumes: Vec<String>,
    /// Glob patterns of files/dirs to exclude when copying workspace into the
    /// container (e.g. "node_modules", "target").
    pub exclude: Vec<String>,
    /// DNS servers to use inside the container (e.g. "8.8.8.8").
    /// Ensures name resolution works even with restricted networks.
    pub dns: Vec<String>,
}

impl SandboxConfig {
    /// Default image used when none is specified
    pub const DEFAULT_IMAGE: &'static str = "minion-sandbox:latest";

    /// Well-known env vars that are auto-forwarded when the user does NOT
    /// specify an explicit `env:` list. This covers the most common
    /// credentials needed by workflows.
    pub const AUTO_ENV: &'static [&'static str] = &[
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        // Elasticsearch (Whitebook and other knowledge-base workflows)
        "ELASTICSEARCH_URL",
        "ELASTICSEARCH_API_KEY",
        "ELASTICSEARCH_INDEX",
    ];

    /// Well-known directories to exclude when copying workspace into the
    /// sandbox container. These are typically large build/cache directories
    /// that would make the copy prohibitively slow and are not needed for
    /// workflow execution.
    pub const AUTO_EXCLUDE: &'static [&'static str] = &[
        "target",
        "node_modules",
        "dist",
        "build",
        "__pycache__",
        ".next",
        ".nuxt",
        "vendor",
        ".tox",
        ".venv",
        "venv",
    ];

    /// Well-known host directories that are auto-mounted when the
    /// user does NOT specify an explicit `volumes:` list.
    /// Note: ~/.claude needs read-write access because Claude CLI writes session data.
    /// Note: ~/.gitconfig is NOT mounted because the host gitconfig often
    /// contains macOS-specific paths (e.g. credential helpers pointing to
    /// /usr/local/bin/gh) and missing safe.directory entries. The sandbox
    /// configures its own gitconfig after workspace copy.
    pub const AUTO_VOLUMES: &'static [&'static str] = &[
        // Root mounts (for cmd steps)
        "~/.config/gh:/root/.config/gh:ro",
        "~/.claude:/root/.claude:rw",
        "~/.ssh:/root/.ssh:ro",
        // Minion user mounts (for agent steps — Claude CLI runs as minion)
        "~/.config/gh:/home/minion/.config/gh:ro",
        "~/.claude:/home/minion/.claude:rw",
        "~/.claude.json:/home/minion/.claude.json:ro",
    ];

    pub fn image(&self) -> &str {
        self.image.as_deref().unwrap_or(Self::DEFAULT_IMAGE)
    }

    /// Secrets that are proxied and should NOT be passed as env vars into the container.
    pub const PROXIED_SECRETS: &'static [&'static str] = &["ANTHROPIC_API_KEY"];

    /// Return the effective env-var list: explicit config overrides auto-env.
    pub fn effective_env(&self) -> Vec<String> {
        if self.env.is_empty() {
            Self::AUTO_ENV.iter().map(|s| (*s).to_string()).collect()
        } else {
            self.env.clone()
        }
    }

    /// Return env vars to forward when the API proxy is active.
    /// Excludes secrets that are handled by the proxy (e.g. ANTHROPIC_API_KEY).
    pub fn effective_env_with_proxy(&self) -> Vec<String> {
        self.effective_env()
            .into_iter()
            .filter(|k| !Self::PROXIED_SECRETS.contains(&k.as_str()))
            .collect()
    }

    /// Return the effective volume list: explicit config overrides auto-volumes.
    /// Tilde (~) is expanded to $HOME on the host.
    pub fn effective_volumes(&self) -> Vec<String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let raw = if self.volumes.is_empty() {
            Self::AUTO_VOLUMES.iter().map(|s| (*s).to_string()).collect::<Vec<_>>()
        } else {
            self.volumes.clone()
        };
        raw.into_iter()
            .map(|v| v.replace('~', &home))
            .filter(|v| {
                // Only mount if the host path actually exists
                let host_path = v.split(':').next().unwrap_or("");
                std::path::Path::new(host_path).exists()
            })
            .collect()
    }

    /// Return the effective exclude list: explicit config overrides auto-exclude.
    pub fn effective_exclude(&self) -> Vec<String> {
        if self.exclude.is_empty() {
            Self::AUTO_EXCLUDE.iter().map(|s| (*s).to_string()).collect()
        } else {
            self.exclude.clone()
        }
    }

    /// Helper: parse a YAML string-list from a mapping key.
    fn parse_string_list(
        mapping: &serde_yaml::Mapping,
        key: &str,
    ) -> Vec<String> {
        mapping
            .get(serde_yaml::Value::String(key.into()))
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
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
                (Self::parse_string_list(net, "allow"), Self::parse_string_list(net, "deny"))
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

        let env = Self::parse_string_list(sandbox, "env");
        let volumes = Self::parse_string_list(sandbox, "volumes");
        let exclude = Self::parse_string_list(sandbox, "exclude");
        let dns = Self::parse_string_list(sandbox, "dns");

        Self {
            enabled,
            image,
            workspace,
            network: NetworkConfig { allow, deny },
            resources: ResourceConfig { cpus, memory },
            env,
            volumes,
            exclude,
            dns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_image() {
        let cfg = SandboxConfig::default();
        assert_eq!(cfg.image(), "minion-sandbox:latest");
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
    fn from_global_config_parses_all_fields() {
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
  env:
    - ANTHROPIC_API_KEY
    - GH_TOKEN
    - CUSTOM_SECRET
  volumes:
    - "~/.config/gh:/root/.config/gh:ro"
    - "~/.claude:/root/.claude:ro"
  exclude:
    - node_modules
    - target
    - .git/objects
  dns:
    - "8.8.8.8"
    - "1.1.1.1"
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
        assert_eq!(cfg.env, ["ANTHROPIC_API_KEY", "GH_TOKEN", "CUSTOM_SECRET"]);
        assert_eq!(cfg.volumes.len(), 2);
        assert_eq!(cfg.exclude, ["node_modules", "target", ".git/objects"]);
        assert_eq!(cfg.dns, ["8.8.8.8", "1.1.1.1"]);
    }

    #[test]
    fn from_global_config_empty_returns_default() {
        let map: HashMap<String, serde_yaml::Value> = HashMap::new();
        let cfg = SandboxConfig::from_global_config(&map);
        assert!(!cfg.enabled);
        assert!(cfg.image.is_none());
    }

    #[test]
    fn effective_env_uses_auto_when_empty() {
        let cfg = SandboxConfig::default();
        let env = cfg.effective_env();
        assert!(env.contains(&"ANTHROPIC_API_KEY".to_string()));
        assert!(env.contains(&"GH_TOKEN".to_string()));
    }

    #[test]
    fn effective_env_uses_explicit_when_set() {
        let cfg = SandboxConfig {
            env: vec!["MY_CUSTOM_KEY".to_string()],
            ..Default::default()
        };
        let env = cfg.effective_env();
        assert_eq!(env, vec!["MY_CUSTOM_KEY"]);
        assert!(!env.contains(&"ANTHROPIC_API_KEY".to_string()));
    }

    #[test]
    fn effective_volumes_filters_nonexistent_paths() {
        let cfg = SandboxConfig {
            volumes: vec![
                "/nonexistent/path/abc123:/container/path:ro".to_string(),
            ],
            ..Default::default()
        };
        let vols = cfg.effective_volumes();
        assert!(vols.is_empty(), "should filter out non-existent host paths");
    }
}
