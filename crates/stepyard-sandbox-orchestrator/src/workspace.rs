//! Git worktree lifecycle contract for Stepyard workspace isolation.
//!
//! The workspace manager sits at the same IO boundary as sandbox lifecycle
//! management: it coordinates external processes and filesystem state for the
//! harness, while `stepyard-core` remains an IO-free contract crate.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, SystemTime};
use stepyard_session::SessionId;
use thiserror::Error;
use tokio::process::Command;

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

impl BranchStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::MergeToHead { .. } => "merge_to_head",
            Self::NamedBranch { .. } => "named_branch",
        }
    }

    fn branch_and_base(&self, session_id: &SessionId) -> (Option<String>, String) {
        match self {
            Self::Head => (None, "HEAD".to_string()),
            Self::MergeToHead { target } => (
                Some(format!("stepyard/session-{}", session_id.as_uuid())),
                target.clone(),
            ),
            Self::NamedBranch { name } => (Some(name.clone()), "HEAD".to_string()),
        }
    }
}

/// Prepared workspace metadata passed from `prepare` to `finalize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub merge_target: Option<String>,
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
    pub merged: bool,
    pub conflicts: Vec<String>,
}

impl FinalizeReport {
    pub fn new(timestamp: DateTime<Utc>, merged: bool, conflicts: Vec<String>) -> Self {
        Self {
            timestamp,
            merged,
            conflicts,
        }
    }
}

/// Summary of work performed by stale workspace pruning.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneReport {
    pub timestamp: DateTime<Utc>,
    pub worktrees_pruned: u32,
    pub worktrees_preserved: u32,
    pub pruned: Vec<PrunedWorkspace>,
}

impl PruneReport {
    pub fn new(
        timestamp: DateTime<Utc>,
        worktrees_pruned: u32,
        worktrees_preserved: u32,
        pruned: Vec<PrunedWorkspace>,
    ) -> Self {
        Self {
            timestamp,
            worktrees_pruned,
            worktrees_preserved,
            pruned,
        }
    }
}

/// One workspace directory removed by [`WorkspaceManager::prune`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrunedWorkspace {
    pub path: String,
    pub reason: String,
}

impl PrunedWorkspace {
    pub fn orphan_no_git_entry(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: "orphan_no_git_entry".to_string(),
        }
    }
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

    pub fn workspace_path(&self, session_id: &SessionId) -> PathBuf {
        self.workspaces_dir
            .join(format!("stepyard-session-{session_id}"))
    }

    async fn git_output(
        &self,
        op: &'static str,
        args: Vec<OsString>,
    ) -> Result<Output, WorkspaceError> {
        self.git_output_in(&self.repo_root, op, args).await
    }

    async fn git_output_in(
        &self,
        cwd: &Path,
        op: &'static str,
        args: Vec<OsString>,
    ) -> Result<Output, WorkspaceError> {
        tokio::time::timeout(
            Duration::from_secs(30),
            Command::new("git").current_dir(cwd).args(args).output(),
        )
        .await
        .map_err(|_| WorkspaceError::GitCommand {
            op: op.to_string(),
            stderr: "timed out after 30s".to_string(),
        })?
        .map_err(WorkspaceError::from)
    }

    async fn git_success(
        &self,
        op: &'static str,
        args: Vec<OsString>,
    ) -> Result<Output, WorkspaceError> {
        let output = self.git_output(op, args).await?;
        if output.status.success() {
            return Ok(output);
        }
        Err(WorkspaceError::GitCommand {
            op: op.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    async fn listed_worktree_paths(&self) -> Result<HashSet<PathBuf>, WorkspaceError> {
        let output = self
            .git_success(
                "worktree list",
                vec![
                    OsString::from("worktree"),
                    OsString::from("list"),
                    OsString::from("--porcelain"),
                ],
            )
            .await?;
        let mut paths = HashSet::new();
        for path in String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(PathBuf::from)
        {
            paths.insert(tokio::fs::canonicalize(&path).await.unwrap_or(path));
        }
        Ok(paths)
    }

    fn is_past_retention(&self, modified: SystemTime) -> bool {
        let retention = Duration::from_secs(self.retention_hours.saturating_mul(60 * 60));
        let cutoff = SystemTime::now()
            .checked_sub(retention)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        modified < cutoff
    }
}

#[async_trait]
impl WorkspaceManager for GitWorktreeManager {
    async fn prepare(
        &self,
        session_id: &SessionId,
        strategy: &BranchStrategy,
    ) -> Result<Workspace, WorkspaceError> {
        let path = self.workspace_path(session_id);
        if path.exists() {
            return Err(WorkspaceError::WorktreeExists { path });
        }

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let (branch, base) = strategy.branch_and_base(session_id);
        let mut args = vec![OsString::from("worktree"), OsString::from("add")];
        if let Some(branch_name) = &branch {
            args.push(OsString::from("-b"));
            args.push(OsString::from(branch_name));
        }
        args.push(path.clone().into_os_string());
        args.push(OsString::from(&base));
        self.git_success("worktree add", args).await?;

        Ok(Workspace {
            path,
            branch,
            merge_target: match strategy {
                BranchStrategy::MergeToHead { target } => Some(target.clone()),
                _ => None,
            },
            session_id: *session_id,
        })
    }

    async fn finalize(
        &self,
        workspace: &Workspace,
        outcome: WorkflowOutcome,
    ) -> Result<FinalizeReport, WorkspaceError> {
        let Some(target) = workspace.merge_target.as_ref() else {
            return Ok(FinalizeReport {
                timestamp: Utc::now(),
                merged: false,
                conflicts: Vec::new(),
            });
        };
        let Some(source) = workspace.branch.as_ref() else {
            return Ok(FinalizeReport {
                timestamp: Utc::now(),
                merged: false,
                conflicts: Vec::new(),
            });
        };
        if outcome != WorkflowOutcome::Success {
            return Ok(FinalizeReport {
                timestamp: Utc::now(),
                merged: false,
                conflicts: Vec::new(),
            });
        }

        self.git_success(
            "checkout merge target",
            vec![OsString::from("checkout"), OsString::from(target)],
        )
        .await
        .map_err(|err| match err {
            WorkspaceError::GitCommand { .. } => WorkspaceError::TargetBranchNotFound {
                target: target.clone(),
            },
            other => other,
        })?;

        let merge = self
            .git_output(
                "merge",
                vec![
                    OsString::from("merge"),
                    OsString::from("--no-ff"),
                    OsString::from(source),
                ],
            )
            .await?;
        if merge.status.success() {
            self.git_success(
                "worktree remove",
                vec![
                    OsString::from("worktree"),
                    OsString::from("remove"),
                    OsString::from("--force"),
                    workspace.path.clone().into_os_string(),
                ],
            )
            .await?;
            self.git_success(
                "branch delete",
                vec![
                    OsString::from("branch"),
                    OsString::from("-d"),
                    OsString::from(source),
                ],
            )
            .await?;
            return Ok(FinalizeReport {
                timestamp: Utc::now(),
                merged: true,
                conflicts: Vec::new(),
            });
        }

        let files_output = self
            .git_success(
                "conflict files",
                vec![
                    OsString::from("diff"),
                    OsString::from("--name-only"),
                    OsString::from("--diff-filter=U"),
                ],
            )
            .await?;
        let files: Vec<String> = String::from_utf8_lossy(&files_output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        let _ = self
            .git_output(
                "merge abort",
                vec![OsString::from("merge"), OsString::from("--abort")],
            )
            .await;
        Err(WorkspaceError::MergeConflict { files })
    }

    async fn prune(&self) -> Result<PruneReport, WorkspaceError> {
        let mut report = PruneReport::new(Utc::now(), 0, 0, Vec::new());

        let prune = self
            .git_output(
                "worktree prune",
                vec![OsString::from("worktree"), OsString::from("prune")],
            )
            .await?;
        if !prune.status.success() {
            let stderr = String::from_utf8_lossy(&prune.stderr).to_string();
            if !stderr.contains("nothing to prune") {
                return Err(WorkspaceError::GitCommand {
                    op: "worktree prune".to_string(),
                    stderr,
                });
            }
        }

        let listed = self.listed_worktree_paths().await?;
        let mut entries = match tokio::fs::read_dir(&self.workspaces_dir).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(report),
            Err(err) => return Err(err.into()),
        };

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if !name.starts_with("stepyard-session-") || !entry.file_type().await?.is_dir() {
                continue;
            }

            let comparable_path = tokio::fs::canonicalize(&path).await.unwrap_or(path.clone());
            if listed.contains(&comparable_path) {
                report.worktrees_preserved += 1;
                continue;
            }

            let modified = entry.metadata().await?.modified()?;
            if !self.is_past_retention(modified) {
                report.worktrees_preserved += 1;
                continue;
            }

            let status = self
                .git_output_in(
                    &path,
                    "worktree status",
                    vec![OsString::from("status"), OsString::from("--porcelain")],
                )
                .await;
            let status = match status {
                Ok(output) if output.status.success() => output,
                Ok(output) => {
                    tracing::warn!(
                        path = %path.display(),
                        exit_code = output.status.code().unwrap_or(-1),
                        stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                        "worktree preserved because git status failed",
                    );
                    report.worktrees_preserved += 1;
                    continue;
                }
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "worktree preserved because git status failed",
                    );
                    report.worktrees_preserved += 1;
                    continue;
                }
            };

            let status_text = String::from_utf8_lossy(&status.stdout);
            if !status_text.trim().is_empty() {
                let uncommitted_files_count = status_text
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count();
                tracing::warn!(
                    path = %path.display(),
                    uncommitted_files_count,
                    "worktree preserved due to uncommitted changes",
                );
                report.worktrees_preserved += 1;
                continue;
            }

            tokio::fs::remove_dir_all(&path).await?;
            let pruned = PrunedWorkspace::orphan_no_git_entry(path.display().to_string());
            tracing::info!(
                path = %pruned.path,
                reason = %pruned.reason,
                "workspace pruned",
            );
            report.worktrees_pruned += 1;
            report.pruned.push(pruned);
        }

        Ok(report)
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
    fn branch_strategy_reports_wire_strings() {
        assert_eq!(BranchStrategy::Head.as_str(), "head");
        assert_eq!(
            BranchStrategy::MergeToHead {
                target: "main".to_string()
            }
            .as_str(),
            "merge_to_head"
        );
        assert_eq!(
            BranchStrategy::NamedBranch {
                name: "feat/test".to_string()
            }
            .as_str(),
            "named_branch"
        );
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
            merge_target: None,
            session_id,
        };

        assert_send_future(manager.prepare(&session_id, &strategy));
        assert_send_future(manager.finalize(&workspace, WorkflowOutcome::Success));
        assert_send_future(manager.prune());
    }

    async fn git(cwd: &Path, args: &[&str]) -> std::process::Output {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            Command::new("git").current_dir(cwd).args(args).output(),
        )
        .await
        .expect("git command timed out")
        .expect("git command started")
    }

    async fn create_temp_repo() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        tokio::fs::create_dir(&repo).await.expect("create repo dir");

        let init = git(&repo, &["init", "-b", "main"]).await;
        assert!(init.status.success(), "git init: {init:?}");
        let name = git(&repo, &["config", "user.name", "Stepyard Test"]).await;
        assert!(name.status.success(), "git config name: {name:?}");
        let email = git(&repo, &["config", "user.email", "stepyard@example.invalid"]).await;
        assert!(email.status.success(), "git config email: {email:?}");

        tokio::fs::write(repo.join("README.md"), "hello\n")
            .await
            .expect("write readme");
        let add = git(&repo, &["add", "README.md"]).await;
        assert!(add.status.success(), "git add: {add:?}");
        let commit = git(&repo, &["commit", "-m", "init"]).await;
        assert!(commit.status.success(), "git commit: {commit:?}");

        temp
    }

    #[tokio::test]
    async fn prepare_named_branch_creates_git_worktree() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let workspaces = temp.path().join("workspaces");
        let manager = GitWorktreeManager::new(repo, workspaces.clone(), 24);
        let session_id = SessionId::new();

        let workspace = manager
            .prepare(
                &session_id,
                &BranchStrategy::NamedBranch {
                    name: "feat/test".to_string(),
                },
            )
            .await
            .expect("prepare worktree");

        assert_eq!(
            workspace.path,
            workspaces.join(format!("stepyard-session-{session_id}"))
        );
        assert!(workspace.path.exists(), "worktree path exists");
        assert_eq!(workspace.branch.as_deref(), Some("feat/test"));

        let branch = git(&workspace.path, &["branch", "--show-current"]).await;
        assert!(branch.status.success(), "git branch: {branch:?}");
        assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "feat/test");
    }

    #[tokio::test]
    async fn prepare_existing_worktree_path_returns_typed_error() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let workspaces = temp.path().join("workspaces");
        let manager = GitWorktreeManager::new(repo, workspaces, 24);
        let session_id = SessionId::new();

        manager
            .prepare(
                &session_id,
                &BranchStrategy::NamedBranch {
                    name: "feat/test".to_string(),
                },
            )
            .await
            .expect("first prepare");

        let err = manager
            .prepare(
                &session_id,
                &BranchStrategy::NamedBranch {
                    name: "feat/test-again".to_string(),
                },
            )
            .await
            .expect_err("second prepare should fail");

        assert!(matches!(err, WorkspaceError::WorktreeExists { .. }));
    }

    #[tokio::test]
    async fn prepare_merge_to_head_records_merge_target() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let workspaces = temp.path().join("workspaces");
        let manager = GitWorktreeManager::new(repo, workspaces, 24);
        let session_id = SessionId::new();

        let workspace = manager
            .prepare(
                &session_id,
                &BranchStrategy::MergeToHead {
                    target: "main".to_string(),
                },
            )
            .await
            .expect("prepare worktree");

        assert_eq!(
            workspace.branch.as_deref(),
            Some(format!("stepyard/session-{}", session_id.as_uuid()).as_str())
        );
        assert_eq!(workspace.merge_target.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn finalize_merge_to_head_clean_merge_removes_temp_branch_and_worktree() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let workspaces = temp.path().join("workspaces");
        let manager = GitWorktreeManager::new(repo.clone(), workspaces, 24);
        let session_id = SessionId::new();

        let workspace = manager
            .prepare(
                &session_id,
                &BranchStrategy::MergeToHead {
                    target: "main".to_string(),
                },
            )
            .await
            .expect("prepare worktree");

        tokio::fs::write(workspace.path.join("feature.txt"), "feature\n")
            .await
            .expect("write feature");
        assert!(git(&workspace.path, &["add", "feature.txt"])
            .await
            .status
            .success());
        assert!(git(&workspace.path, &["commit", "-m", "feature"])
            .await
            .status
            .success());

        let report = manager
            .finalize(&workspace, WorkflowOutcome::Success)
            .await
            .expect("clean finalize");
        assert!(report.merged);
        assert!(report.conflicts.is_empty());
        assert!(
            !workspace.path.exists(),
            "worktree removed after clean merge"
        );
        assert!(
            repo.join("feature.txt").exists(),
            "main received merged file"
        );
        let branch = workspace.branch.as_deref().unwrap();
        let listed = git(&repo, &["branch", "--list", branch]).await;
        assert!(listed.status.success());
        assert!(
            String::from_utf8_lossy(&listed.stdout).trim().is_empty(),
            "temp branch removed after clean merge"
        );
    }

    #[tokio::test]
    async fn finalize_merge_to_head_conflict_preserves_branch_and_worktree() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let workspaces = temp.path().join("workspaces");
        let manager = GitWorktreeManager::new(repo.clone(), workspaces, 24);
        let session_id = SessionId::new();

        let workspace = manager
            .prepare(
                &session_id,
                &BranchStrategy::MergeToHead {
                    target: "main".to_string(),
                },
            )
            .await
            .expect("prepare worktree");

        tokio::fs::write(workspace.path.join("README.md"), "branch\n")
            .await
            .expect("write branch readme");
        assert!(git(&workspace.path, &["add", "README.md"])
            .await
            .status
            .success());
        assert!(git(&workspace.path, &["commit", "-m", "branch change"])
            .await
            .status
            .success());

        tokio::fs::write(repo.join("README.md"), "main\n")
            .await
            .expect("write main readme");
        assert!(git(&repo, &["add", "README.md"]).await.status.success());
        assert!(git(&repo, &["commit", "-m", "main change"])
            .await
            .status
            .success());

        let err = manager
            .finalize(&workspace, WorkflowOutcome::Success)
            .await
            .expect_err("conflict should fail finalize");
        assert!(matches!(
            err,
            WorkspaceError::MergeConflict { ref files } if files == &vec!["README.md".to_string()]
        ));
        assert!(workspace.path.exists(), "conflicted worktree is preserved");
        let branch = workspace.branch.as_deref().unwrap();
        let listed = git(&repo, &["branch", "--list", branch]).await;
        assert!(listed.status.success());
        assert!(
            !String::from_utf8_lossy(&listed.stdout).trim().is_empty(),
            "conflicted temp branch is preserved"
        );
        let status = git(&repo, &["status", "--porcelain"]).await;
        assert!(status.status.success());
        assert_eq!(String::from_utf8_lossy(&status.stdout).trim(), "");
        let readme = tokio::fs::read_to_string(repo.join("README.md"))
            .await
            .expect("read main readme");
        assert_eq!(readme, "main\n");
    }

    #[tokio::test]
    async fn prune_missing_workspaces_dir_is_noop() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let manager = GitWorktreeManager::new(repo, temp.path().join("missing-workspaces"), 0);

        let report = manager.prune().await.expect("prune");

        assert_eq!(report.worktrees_pruned, 0);
        assert_eq!(report.worktrees_preserved, 0);
        assert!(report.pruned.is_empty());
    }

    #[tokio::test]
    async fn prune_removes_clean_stale_orphan_workspace_dir() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let workspaces = temp.path().join("workspaces");
        tokio::fs::create_dir_all(&workspaces)
            .await
            .expect("create workspaces");
        let orphan = workspaces.join("stepyard-session-orphan");
        tokio::fs::create_dir(&orphan).await.expect("create orphan");
        assert!(git(&orphan, &["init", "-b", "main"]).await.status.success());

        let manager = GitWorktreeManager::new(repo, workspaces, 0);
        let report = manager.prune().await.expect("prune");

        assert_eq!(report.worktrees_pruned, 1);
        assert_eq!(report.worktrees_preserved, 0);
        assert_eq!(
            report.pruned,
            vec![PrunedWorkspace::orphan_no_git_entry(
                orphan.display().to_string()
            )]
        );
        assert!(!orphan.exists(), "clean stale orphan is removed");
    }

    #[tokio::test]
    async fn prune_preserves_orphan_workspace_with_uncommitted_changes() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let workspaces = temp.path().join("workspaces");
        tokio::fs::create_dir_all(&workspaces)
            .await
            .expect("create workspaces");
        let orphan = workspaces.join("stepyard-session-dirty");
        tokio::fs::create_dir(&orphan).await.expect("create orphan");
        assert!(git(&orphan, &["init", "-b", "main"]).await.status.success());
        tokio::fs::write(orphan.join("dirty.txt"), "dirty\n")
            .await
            .expect("write dirty file");

        let manager = GitWorktreeManager::new(repo, workspaces, 0);
        let report = manager.prune().await.expect("prune");

        assert_eq!(report.worktrees_pruned, 0);
        assert_eq!(report.worktrees_preserved, 1);
        assert!(report.pruned.is_empty());
        assert!(orphan.exists(), "dirty orphan is preserved");
    }

    #[tokio::test]
    async fn prune_preserves_registered_worktree() {
        let temp = create_temp_repo().await;
        let repo = temp.path().join("repo");
        let workspaces = temp.path().join("workspaces");
        let manager = GitWorktreeManager::new(repo, workspaces, 0);
        let session_id = SessionId::new();

        let workspace = manager
            .prepare(
                &session_id,
                &BranchStrategy::NamedBranch {
                    name: "feat/prune-preserve".to_string(),
                },
            )
            .await
            .expect("prepare registered worktree");

        let report = manager.prune().await.expect("prune");

        assert_eq!(report.worktrees_pruned, 0);
        assert_eq!(report.worktrees_preserved, 1);
        assert!(report.pruned.is_empty());
        assert!(workspace.path.exists(), "registered worktree is preserved");
    }
}
