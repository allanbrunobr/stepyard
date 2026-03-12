/// Docker Sandbox Integration
///
/// Provides three sandbox modes:
///
/// **Mode 1 — Full Workflow** (`--sandbox` CLI flag):
///   Creates a Docker container, copies workspace, executes all steps inside,
///   copies results back, and destroys the container.
///
/// **Mode 2 — Agent Only** (`config.agent.sandbox: true`):
///   Only `agent` type steps run inside Docker; all other steps run on the host.
///
/// **Mode 3 — Devbox** (`config.global.sandbox.enabled: true`):
///   Full configuration with custom image, workspace path, network allow/deny
///   lists, and resource limits (CPUs, memory). Equivalent to Mode 1 with
///   explicit configuration.
///
/// **Requirements**: Docker Desktop 4.40+ must be installed and running.
pub mod config;
pub mod docker;

pub use config::{SandboxConfig, SandboxMode};
pub use docker::DockerSandbox;

use anyhow::{bail, Result};

/// Determine the sandbox mode from workflow config and CLI flags.
pub fn resolve_mode(
    sandbox_flag: bool,
    global_config: &std::collections::HashMap<String, serde_yaml::Value>,
    agent_config: &std::collections::HashMap<String, serde_yaml::Value>,
) -> SandboxMode {
    // CLI --sandbox flag takes priority → Mode 1
    if sandbox_flag {
        return SandboxMode::FullWorkflow;
    }

    // config.global.sandbox.enabled: true → Mode 3 (Devbox)
    if let Some(serde_yaml::Value::Mapping(m)) = global_config.get("sandbox") {
        if m.get(serde_yaml::Value::String("enabled".into()))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return SandboxMode::Devbox;
        }
    }

    // config.agent.sandbox: true → Mode 2
    if agent_config
        .get("sandbox")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return SandboxMode::AgentOnly;
    }

    SandboxMode::Disabled
}

/// Validate that Docker is available; return a friendly error if not.
pub async fn require_docker() -> Result<()> {
    if !DockerSandbox::is_sandbox_available().await {
        bail!(
            "Docker Sandbox is not available.\n\
             \n\
             Requirements:\n\
             • Docker Desktop 4.40 or later (https://www.docker.com/products/docker-desktop/)\n\
             • Docker daemon must be running\n\
             \n\
             To disable the sandbox, remove --sandbox flag or set `config.agent.sandbox: false`."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_global_sandbox_enabled() -> HashMap<String, serde_yaml::Value> {
        serde_yaml::from_str(
            r#"
sandbox:
  enabled: true
  image: "ubuntu:22.04"
"#,
        )
        .unwrap()
    }

    fn make_agent_sandbox_true() -> HashMap<String, serde_yaml::Value> {
        serde_yaml::from_str("sandbox: true").unwrap()
    }

    #[test]
    fn cli_flag_wins_over_config() {
        let mode = resolve_mode(true, &HashMap::new(), &HashMap::new());
        assert_eq!(mode, SandboxMode::FullWorkflow);
    }

    #[test]
    fn global_config_enabled_gives_devbox_mode() {
        let global = make_global_sandbox_enabled();
        let mode = resolve_mode(false, &global, &HashMap::new());
        assert_eq!(mode, SandboxMode::Devbox);
    }

    #[test]
    fn agent_config_sandbox_true_gives_agent_only() {
        let agent = make_agent_sandbox_true();
        let mode = resolve_mode(false, &HashMap::new(), &agent);
        assert_eq!(mode, SandboxMode::AgentOnly);
    }

    #[test]
    fn no_config_gives_disabled() {
        let mode = resolve_mode(false, &HashMap::new(), &HashMap::new());
        assert_eq!(mode, SandboxMode::Disabled);
    }

    #[test]
    fn cli_flag_overrides_agent_config() {
        let agent = make_agent_sandbox_true();
        let mode = resolve_mode(true, &HashMap::new(), &agent);
        assert_eq!(mode, SandboxMode::FullWorkflow);
    }
}
