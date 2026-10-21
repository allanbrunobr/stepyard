//! [`LocalShellLifecycle`] — run commands directly on the host via `sh -c`.
//!
//! Used by `stepyard execute --no-sandbox --engine v2`. No Docker daemon, no
//! container boundaries — each `exec` spawns a fresh host process in the
//! current working directory. On Unix, each spawned process becomes its own
//! process-group leader so dropping an in-flight exec future kills descendants
//! as well as the direct child.
//!
//! Trades the container isolation guarantees for simpler local runs. Callers
//! that need isolation must pick [`crate::DockerLifecycle`] instead.

use std::collections::HashMap;
use std::process::Stdio;
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

#[derive(Debug)]
struct LocalProcessGroupKillOnDrop {
    pgid: i32,
}

impl LocalProcessGroupKillOnDrop {
    #[allow(dead_code)] // referenced only on Unix; non-Unix builds keep the helper shape.
    fn new(pgid: i32) -> Self {
        Self { pgid }
    }
}

#[cfg(unix)]
impl Drop for LocalProcessGroupKillOnDrop {
    fn drop(&mut self) {
        // SAFETY: a negative pid targets the process group whose id is
        // `abs(pid)`. The pgid comes from a child we spawned after
        // `process_group(0)`, so it names a group owned by this lifecycle.
        // ESRCH for an already-reaped group is intentionally ignored.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
impl Drop for LocalProcessGroupKillOnDrop {
    fn drop(&mut self) {}
}

async fn run_local_command(mut command: Command) -> Result<ExecOutput, SandboxError> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(unix)]
    command.process_group(0);

    let child = command
        .spawn()
        .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;

    #[cfg(unix)]
    let _kill_group = {
        let pid = child
            .id()
            .expect("Child::id is Some between spawn and wait") as i32;
        LocalProcessGroupKillOnDrop::new(pid)
    };

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
    Ok(ExecOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
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

    async fn exec(&self, _id: &SandboxId, cmd: &[String]) -> Result<ExecOutput, SandboxError> {
        if cmd.is_empty() {
            return Err(SandboxError::ExecFailed("empty argv".into()));
        }
        let mut command = Command::new(&cmd[0]);
        command.args(&cmd[1..]);
        run_local_command(command).await
    }

    async fn exec_with_env(
        &self,
        _id: &SandboxId,
        cmd: &[String],
        env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        if cmd.is_empty() {
            return Err(SandboxError::ExecFailed("empty argv".into()));
        }
        let mut command = Command::new(&cmd[0]);
        command.args(&cmd[1..]).envs(env);
        run_local_command(command).await
    }
}

struct LocalShellExec;

#[async_trait]
impl ExecFn for LocalShellExec {
    async fn exec(&self, _id: SandboxId, cmd: &str) -> Result<ExecOutput, SandboxError> {
        // `run_local_command` adds `kill_on_drop` plus Unix process-group
        // cleanup so a cancelled harness future stops descendants too.
        let mut command = Command::new("sh");
        command.args(["-c", cmd]);
        run_local_command(command).await
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

    #[tokio::test]
    async fn exec_with_env_propagates_pairs_to_child_process() {
        let lifecycle = LocalShellLifecycle::new();
        let id = SandboxId::new();
        let mut env = HashMap::new();
        env.insert("LOCAL_SHELL_TEST_VAR".to_string(), "propagated".to_string());
        let cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf %s \"$LOCAL_SHELL_TEST_VAR\"".to_string(),
        ];
        let output = lifecycle.exec_with_env(&id, &cmd, &env).await.unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, "propagated");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_exec_future_kills_descendant_process() {
        let lifecycle = LocalShellLifecycle::new();
        let sandbox = lifecycle.create(Uuid::new_v4()).await.unwrap();
        let pidfile =
            std::env::temp_dir().join(format!("stepyard-local-shell-{}.pid", Uuid::new_v4()));
        let script = format!("sleep 60 & echo $! > {}; wait", pidfile.display());

        let handle = {
            let sandbox = sandbox.clone();
            tokio::spawn(async move { sandbox.exec(&script).await })
        };

        let started = std::time::Instant::now();
        let descendant_pid: i32 = loop {
            if let Ok(s) = std::fs::read_to_string(&pidfile) {
                if let Ok(pid) = s.trim().parse() {
                    break pid;
                }
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "descendant pidfile never appeared",
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };

        handle.abort();
        let _ = handle.await;

        let started = std::time::Instant::now();
        loop {
            // SAFETY: `kill(pid, 0)` is a liveness probe and does not send a
            // signal. The pid came from a child spawned by this test.
            let rc = unsafe { libc::kill(descendant_pid, 0) };
            if rc == -1 {
                let err = std::io::Error::last_os_error();
                assert_eq!(
                    err.raw_os_error(),
                    Some(libc::ESRCH),
                    "kill(pid, 0) failed with unexpected errno: {err}",
                );
                let _ = std::fs::remove_file(&pidfile);
                return;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(2),
                "descendant pid {descendant_pid} still alive after dropping exec future",
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
}
