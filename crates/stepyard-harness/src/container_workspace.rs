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
         fi; \
         git rev-parse -q --verify HEAD > .git/stepyard-base || true; "
    );
    // `.git/stepyard-base` records the commit the copy-in started from, so
    // copy-out can compare against it instead of the container's HEAD: work
    // a step commits inside the container must still reach the host
    // (BUG-MOTOR-01).
    if let Some(user) = agent_user {
        let user = shell_quote(user);
        script.push_str(&format!(
            "if id -u {user} >/dev/null 2>&1; then chown -R {user}: {dir}; fi; "
        ));
    }
    script.push_str(&format!("touch {READY_MARKER}"));
    script
}

/// Drop `Deleted` entries for paths the copy-in never brought into the
/// container.
///
/// Copy-in runs `tar --exclude=<pattern>`, and a pattern without `/` matches
/// any path component, so tracked files such as `.stepyard/defaults.yaml` or
/// `src/target/mod.rs` are absent from /workspace while the copied `.git`
/// still lists them. `git status` then reports them deleted, and copy-out
/// removed them from the host (SEG-RUST-08). The container's view of an
/// excluded path is not authoritative, so its deletions are ignored.
/// Additions and modifications are kept as before.
pub(crate) fn without_excluded_deletions(
    changes: Vec<stepyard_sandbox_orchestrator::ChangedPath>,
    exclude: &[String],
) -> Vec<stepyard_sandbox_orchestrator::ChangedPath> {
    use stepyard_sandbox_orchestrator::ChangedPath;
    changes
        .into_iter()
        .filter(|change| match change {
            ChangedPath::Deleted(path) => !is_excluded(path, exclude),
            ChangedPath::Modified(_) => true,
        })
        .collect()
}

/// Whether `path` (relative, `/`-separated) falls under an exclude pattern:
/// a pattern with `/` matches that path prefix, one without `/` matches any
/// single component (`*` and `?` wildcards supported, as in tar).
fn is_excluded(path: &str, exclude: &[String]) -> bool {
    let path = path.trim_start_matches("./");
    exclude.iter().any(|pattern| {
        let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
        if pattern.contains('/') {
            path == pattern || path.starts_with(&format!("{pattern}/"))
        } else {
            path.split('/')
                .any(|component| wildcard_match(pattern, component))
        }
    })
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &t[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &t[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use stepyard_sandbox_orchestrator::ChangedPath;

    #[test]
    fn deletions_under_excluded_components_are_ignored_but_real_changes_kept() {
        let exclude = ContainerWorkspace::new("/h".into()).exclude;
        let changes = vec![
            ChangedPath::Deleted(".stepyard/defaults.yaml".into()),
            ChangedPath::Deleted("src/target/mod.rs".into()),
            ChangedPath::Deleted("web/node_modules/keep.js".into()),
            ChangedPath::Deleted("app/__pycache__/x.pyc".into()),
            ChangedPath::Deleted("removed-for-real.txt".into()),
            ChangedPath::Deleted("targets/not-excluded.rs".into()),
            ChangedPath::Modified("target/new-artifact".into()),
            ChangedPath::Modified("README.md".into()),
        ];
        assert_eq!(
            without_excluded_deletions(changes, &exclude),
            vec![
                ChangedPath::Deleted("removed-for-real.txt".into()),
                ChangedPath::Deleted("targets/not-excluded.rs".into()),
                ChangedPath::Modified("target/new-artifact".into()),
                ChangedPath::Modified("README.md".into()),
            ]
        );
    }

    #[test]
    fn exclude_patterns_support_paths_and_wildcards_like_tar() {
        let exclude = vec![
            "build/out".to_string(),
            "*.log".to_string(),
            "cache?".to_string(),
        ];
        assert!(is_excluded("build/out/a.o", &exclude));
        assert!(!is_excluded("build/other/a.o", &exclude));
        assert!(is_excluded("deep/dir/run.log", &exclude));
        assert!(is_excluded("x/cache1/y", &exclude));
        assert!(!is_excluded("x/cache12/y", &exclude));
    }

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
