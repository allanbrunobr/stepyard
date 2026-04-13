//! [`LocalShellLifecycle`] — run commands directly on the host via `sh -c`.
//!
//! Used by `minion execute --no-sandbox --engine v2`. No Docker daemon, no
//! container boundaries, no cleanup — each `exec` spawns a fresh `sh -c`
//! process in the current working directory.
//!
//! Trades the container isolation guarantees for simpler local runs. Callers
//! that need isolation must pick [`crate::DockerLifecycle`] instead.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::sandbox::{ExecFn, ExecOutput, Sandbox, SandboxError, SandboxId, SandboxState};
use crate::SandboxLifecycle;

/// [`SandboxLifecycle`] that runs every step on the host with `sh -c`.
#[derive(Debug, Default, Clone)]
pub struct LocalShellLifecycle;

impl LocalShellLifecycle {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SandboxLifecycle for LocalShellLifecycle {
    async fn create(&self, _session_id: Uuid) -> Result<Sandbox, SandboxError> {
        let id = SandboxId::new();
        Ok(Sandbox {
            state: Arc::new(Mutex::new(SandboxState {
                id,
                destroyed: false,
            })),
            exec_fn: Arc::new(LocalShellExec),
        })
    }

    async fn destroy(&self, _id: &SandboxId) -> Result<(), SandboxError> {
        Ok(())
    }
}

struct LocalShellExec;

#[async_trait]
impl ExecFn for LocalShellExec {
    async fn exec(&self, _id: SandboxId, cmd: &str) -> Result<ExecOutput, SandboxError> {
        // `kill_on_drop` so a cancelled harness future stops the subprocess
        // instead of letting it run to completion in the background.
        let output = Command::new("sh")
            .args(["-c", cmd])
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_simple_command_on_host() {
        let lifecycle = LocalShellLifecycle::new();
        let sandbox = lifecycle.create(Uuid::new_v4()).await.unwrap();
        let output = sandbox.exec("echo hello-local").await.unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("hello-local"));
    }

    #[tokio::test]
    async fn non_zero_exit_returns_in_exec_output() {
        let lifecycle = LocalShellLifecycle::new();
        let sandbox = lifecycle.create(Uuid::new_v4()).await.unwrap();
        let output = sandbox.exec("exit 7").await.unwrap();
        assert_eq!(output.exit_code, 7);
    }
}
