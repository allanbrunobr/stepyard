//! Shared helpers for the CLI integration tests.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A throwaway git repository to use as the working directory of
/// `stepyard execute`. v2 prepares a git worktree per session under
/// `<cwd>/.stepyard/workspaces`; run from the repository root, every test
/// run registered session worktrees in the developer's own `.git`.
#[allow(dead_code)] // not every test binary that includes this module uses it
pub fn temp_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        &["init", "-q"][..],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ][..],
    ] {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    }
    dir
}

/// Absolute path of a file in this repository (for workflows referenced from
/// a test that runs in another working directory).
#[allow(dead_code)]
pub fn repo_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}
