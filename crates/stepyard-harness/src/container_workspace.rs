//! Project workspace inside the session container (v2 Docker/Podman runs).
//!
//! When [`crate::HarnessConfig::container_workspace`] is set, the engine
//! copies the project into the session container before the first step that
//! runs there. `cmd` steps and `agent` steps then run in that copy. When the
//! workflow ends (success or failure), the files that changed are copied
//! back to the project on the host and the container is removed.

use std::path::PathBuf;
use stepyard_sandbox_orchestrator::{ContainerHandle, CopyOutReport};

/// Marker file that records "the project is already in this container", so
/// a resumed run does not copy the project again over the agent's changes.
pub(crate) const READY_MARKER: &str = "/tmp/.stepyard-workspace-ready";

/// Where the project lives on the host and in the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerWorkspace {
    /// Project directory on the host (the copy source and the sync target).
    pub host_dir: PathBuf,
    /// Workspace directory in the container, e.g. `/workspace`.
    pub container_dir: String,
    /// Run agent steps as this user if it exists in the image. The Claude
    /// CLI refuses `--dangerously-skip-permissions` as root.
    pub agent_user: Option<String>,
    /// Paths not copied into the container (tar `--exclude` patterns).
    pub exclude: Vec<String>,
}

impl ContainerWorkspace {
    /// Default layout: `/workspace`, agent user `stepyard`, and the usual
    /// build/dependency directories excluded.
    pub fn new(host_dir: PathBuf) -> Self {
        Self {
            host_dir,
            container_dir: "/workspace".into(),
            agent_user: Some("stepyard".into()),
            exclude: [
                "target",
                "node_modules",
                ".stepyard",
                ".venv",
                "__pycache__",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

/// How to run an agent CLI in the session container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentContainer {
    pub handle: ContainerHandle,
    pub user: Option<String>,
    pub workdir: String,
}

/// Result of copying the container's changes back to the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSync {
    Synced(CopyOutReport),
    Failed(String),
}

/// Shell script run (as the container's default user) after the project is
/// copied in. It makes sure `/workspace` is a git repository, so the end of
/// the run can list changed files with `git status`; gives the agent user
/// ownership of the files; and writes the ready marker.
pub(crate) fn setup_script(container_dir: &str, agent_user: Option<&str>) -> String {
    let dir = shell_quote(container_dir);
    let mut script = format!(
        "set -e; cd {dir}; \
         if [ -f .git ]; then rm -f .git; fi; \
         if [ ! -d .git ]; then \
           git init -q && git add -A && \
           git -c user.name=stepyard -c user.email=stepyard@localhost \
             commit -q --allow-empty -m 'stepyard: workspace baseline'; \
         fi; "
    );
    if let Some(user) = agent_user {
        let user = shell_quote(user);
        script.push_str(&format!(
            "if id -u {user} >/dev/null 2>&1; then chown -R {user}: {dir}; fi; "
        ));
    }
    script.push_str(&format!("touch {READY_MARKER}"));
    script
}

pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_script_creates_baseline_chowns_and_marks_ready() {
        let s = setup_script("/workspace", Some("stepyard"));
        assert!(s.starts_with("set -e; cd '/workspace';"));
        assert!(s.contains("git init -q"));
        assert!(s.contains("chown -R 'stepyard': '/workspace'"));
        assert!(s.ends_with("touch /tmp/.stepyard-workspace-ready"));
    }

    #[test]
    fn setup_script_without_agent_user_skips_chown() {
        assert!(!setup_script("/w", None).contains("chown"));
    }

    #[test]
    fn default_workspace_excludes_build_dirs() {
        let ws = ContainerWorkspace::new(PathBuf::from("/p"));
        assert_eq!(ws.container_dir, "/workspace");
        assert!(ws.exclude.contains(&"target".to_string()));
        assert!(ws.exclude.contains(&"node_modules".to_string()));
    }
}
