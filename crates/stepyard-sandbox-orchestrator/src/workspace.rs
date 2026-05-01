//! Git worktree lifecycle contract for Stepyard workspace isolation.
//!
//! The workspace manager sits at the same IO boundary as sandbox lifecycle
//! management: it coordinates external processes and filesystem state for the
//! harness, while `stepyard-core` remains an IO-free contract crate.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use stepyard_session::SessionId;
use thiserror::Error;

/// Strategy for choosing which branch backs a prepared worktree.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchStrategy {
    /// Use the current HEAD without creating or checking out a named branch.
    Head,
    /// Prepare work on a temporary branch and merge it back to `target`.
    MergeToHead { target: String },
    /// Prepare work on the explicitly named branch.
    NamedBranch { name: String },
}

/// Prepared workspace metadata passed from `prepare` to `finalize`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub session_id: SessionId,
}

/// Final workflow outcome used by workspace finalization policy.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowOutcome {
    Success,
    Failure,
}

/// Summary of work performed by `finalize`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizeReport {
    pub timestamp: DateTime<Utc>,
    pub branches_merged: u32,
    pub conflicts: u32,
}

/// Summary of work performed by stale workspace pruning.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneReport {
    pub timestamp: DateTime<Utc>,
    pub worktrees_pruned: u32,
    pub worktrees_preserved: u32,
}

/// Errors raised while managing git worktrees.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("git command failed during {op}: {stderr}")]
    GitCommand { op: String, stderr: String },
    #[error("worktree already exists at {path:?}")]
    WorktreeExists { path: PathBuf },
    #[error("uncommitted changes at {path:?}")]
    UncommittedChanges { path: PathBuf },
    #[error("target branch not found: {target}")]
    TargetBranchNotFound { target: String },
    #[error("merge conflict in files: {files:?}")]
    MergeConflict { files: Vec<String> },
    #[error("workspace io failed: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
}

/// Contract for preparing, finalizing, and pruning isolated git worktrees.
#[async_trait]
pub trait WorkspaceManager: Send + Sync + 'static {
    async fn prepare(
        &self,
        session_id: &SessionId,
        strategy: &BranchStrategy,
    ) -> Result<Workspace, WorkspaceError>;

    async fn finalize(
        &self,
        workspace: &Workspace,
        outcome: WorkflowOutcome,
    ) -> Result<FinalizeReport, WorkspaceError>;

    async fn prune(&self) -> Result<PruneReport, WorkspaceError>;
}

/// Git-backed workspace manager. Story 4.1 establishes the stable contract;
/// later stories fill in the concrete git subprocess behavior.
#[derive(Debug, Clone)]
pub struct GitWorktreeManager {
    repo_root: PathBuf,
    workspaces_dir: PathBuf,
    retention_hours: u64,
}

impl GitWorktreeManager {
    pub fn new(repo_root: PathBuf, workspaces_dir: PathBuf, retention_hours: u64) -> Self {
        Self {
            repo_root,
            workspaces_dir,
            retention_hours,
        }
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn workspaces_dir(&self) -> &Path {
        &self.workspaces_dir
    }

    pub fn retention_hours(&self) -> u64 {
        self.retention_hours
    }
}

#[async_trait]
impl WorkspaceManager for GitWorktreeManager {
    async fn prepare(
        &self,
        _session_id: &SessionId,
        _strategy: &BranchStrategy,
    ) -> Result<Workspace, WorkspaceError> {
        unimplemented!("Story 4.2")
    }

    async fn finalize(
        &self,
        _workspace: &Workspace,
        _outcome: WorkflowOutcome,
    ) -> Result<FinalizeReport, WorkspaceError> {
        unimplemented!("Story 4.4")
    }

    async fn prune(&self) -> Result<PruneReport, WorkspaceError> {
        unimplemented!("Story 4.5")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn assert_send_future<F: std::future::Future + Send>(_: F) {}

    #[test]
    fn git_worktree_manager_records_constructor_inputs() {
        let manager = GitWorktreeManager::new(
            PathBuf::from("/repo"),
            PathBuf::from("/repo/.stepyard/workspaces"),
            24,
        );

        assert_eq!(manager.repo_root(), Path::new("/repo"));
        assert_eq!(
            manager.workspaces_dir(),
            Path::new("/repo/.stepyard/workspaces")
        );
        assert_eq!(manager.retention_hours(), 24);
    }

    #[test]
    fn branch_strategy_serializes_as_snake_case_yaml() {
        let strategy = BranchStrategy::MergeToHead {
            target: "main".to_string(),
        };

        let rendered = serde_yaml::to_string(&strategy).expect("serialize strategy");
        assert!(rendered.contains("merge_to_head"));
        assert!(rendered.contains("target: main"));
    }

    #[test]
    fn workspace_manager_trait_object_methods_are_reachable() {
        let manager: Arc<dyn WorkspaceManager> = Arc::new(GitWorktreeManager::new(
            PathBuf::from("/repo"),
            PathBuf::from("/repo/.stepyard/workspaces"),
            24,
        ));
        let session_id = SessionId::new();
        let strategy = BranchStrategy::Head;
        let workspace = Workspace {
            path: PathBuf::from("/repo/.stepyard/workspaces/example"),
            branch: None,
            session_id,
        };

        assert_send_future(manager.prepare(&session_id, &strategy));
        assert_send_future(manager.finalize(&workspace, WorkflowOutcome::Success));
        assert_send_future(manager.prune());
    }
}
