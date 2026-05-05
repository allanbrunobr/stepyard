//! [`PodmanLifecycle`] — Podman backend via the `podman` CLI subprocess.
//!
//! This intentionally mirrors [`crate::DockerLifecycle`]. The provider changes,
//! not the [`crate::SandboxLifecycle`] trait contract.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::docker_errors::{classify_create_stderr, classify_destroy_stderr};
use crate::sandbox::{ExecFn, ExecOutput, Sandbox, SandboxError, SandboxId, SandboxState};
use crate::{CreateOptions, ExecOptions, SandboxLifecycle};

const PODMAN: &str = "podman";

/// Tunables for [`PodmanLifecycle`].
#[derive(Debug, Clone)]
pub struct PodmanLifecycleConfig {
    pub image: String,
    pub shell: String,
}

impl Default for PodmanLifecycleConfig {
    fn default() -> Self {
        Self {
            image: "alpine:latest".into(),
            shell: "sh".into(),
        }
    }
}

/// Podman-backed [`SandboxLifecycle`] impl. Uses the system `podman` CLI.
#[derive(Debug, Clone)]
pub struct PodmanLifecycle {
    config: PodmanLifecycleConfig,
}

impl Default for PodmanLifecycle {
    fn default() -> Self {
        Self::new(PodmanLifecycleConfig::default())
    }
}

impl PodmanLifecycle {
    pub fn new(config: PodmanLifecycleConfig) -> Self {
        Self { config }
    }

    fn container_name(session_id: Uuid) -> String {
        format!("minion-session-{session_id}")
    }

    async fn find_container(name: &str) -> Result<Option<String>, SandboxError> {
        let output = Command::new(PODMAN)
            .args(["ps", "-q", "--filter", &format!("name=^/{name}$")])
            .output()
            .await
            .map_err(|e| SandboxError::BackendUnavailable(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!stdout.is_empty()).then_some(stdout))
    }
}

#[async_trait]
impl SandboxLifecycle for PodmanLifecycle {
    async fn create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
        self.create_with_options(session_id, &CreateOptions::default())
            .await
    }

    async fn create_with_options(
        &self,
        session_id: Uuid,
        opts: &CreateOptions,
    ) -> Result<Sandbox, SandboxError> {
        let name = Self::container_name(session_id);
        let image = opts.image.as_deref().unwrap_or(&self.config.image);

        let mut command = Command::new(PODMAN);
        command
            .args(["run", "-d", "--name", &name])
            .arg("--label")
            .arg(format!("session_id={session_id}"));
        for volume in &opts.volumes {
            command.arg("-v").arg(volume);
        }
        if let Some(cpus) = opts.cpus {
            command.arg("--cpus").arg(cpus.to_string());
        }
        if let Some(memory) = &opts.memory {
            command.arg("--memory").arg(memory);
        }
        for dns in &opts.dns {
            command.arg("--dns").arg(dns);
        }
        if !opts.network.allow.is_empty() || !opts.network.deny.is_empty() {
            command.args(["--network", "bridge"]);
        }
        command
            .arg(image)
            .arg(&self.config.shell)
            .args(["-c", "trap : TERM INT; sleep infinity & wait"]);

        let output = command
            .output()
            .await
            .map_err(|e| SandboxError::BackendUnavailable(e.to_string()))?;

        if !output.status.success() {
            return Err(classify_create_stderr(&output.stderr));
        }

        let id = SandboxId::new();
        Ok(Sandbox {
            state: Arc::new(Mutex::new(SandboxState {
                id,
                destroyed: false,
            })),
            exec_fn: Arc::new(PodmanExec {
                container_name: name,
            }),
        })
    }

    async fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError> {
        tracing::warn!(sandbox_id = %id, "PodmanLifecycle::destroy called without session id — use destroy_by_session");
        Ok(())
    }

    async fn destroy_by_session(&self, session_id: Uuid) -> Result<(), SandboxError> {
        let name = Self::container_name(session_id);
        let output = Command::new(PODMAN)
            .args(["rm", "-f", &name])
            .output()
            .await
            .map_err(|e| SandboxError::BackendUnavailable(e.to_string()))?;
        if !output.status.success() {
            if let Some(err) = classify_destroy_stderr(&output.stderr) {
                return Err(err);
            }
        }
        Ok(())
    }

    async fn exec(&self, id: &SandboxId, cmd: &[String]) -> Result<ExecOutput, SandboxError> {
        let name = Self::container_name(*id.as_uuid());
        let mut args: Vec<&str> = vec!["exec", &name];
        args.extend(cmd.iter().map(String::as_str));
        let output = Command::new(PODMAN)
            .args(&args)
            .output()
            .await
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    async fn exec_with_env(
        &self,
        id: &SandboxId,
        cmd: &[String],
        env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        let name = Self::container_name(*id.as_uuid());
        let mut pairs: Vec<(&String, &String)> = env.iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        let env_values: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();

        let mut args: Vec<&str> = Vec::with_capacity(2 + env_values.len() * 2 + 1 + cmd.len());
        args.push("exec");
        for value in &env_values {
            args.push("--env");
            args.push(value);
        }
        args.push(&name);
        args.extend(cmd.iter().map(String::as_str));

        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        tracing::debug!(
            container = %name,
            env_keys = ?keys,
            argc = args.len(),
            "podman exec_with_env"
        );

        let output = Command::new(PODMAN)
            .args(&args)
            .output()
            .await
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    async fn exec_with_options(
        &self,
        id: &SandboxId,
        cmd: &[String],
        opts: &ExecOptions,
    ) -> Result<ExecOutput, SandboxError> {
        let Some(idle_timeout) = opts.idle_timeout else {
            return self.exec_with_env(id, cmd, &opts.env).await;
        };

        let name = Self::container_name(*id.as_uuid());
        let mut pairs: Vec<(&String, &String)> = opts.env.iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        let env_values: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();

        let mut command = Command::new(PODMAN);
        command.arg("exec");
        for value in &env_values {
            command.arg("--env").arg(value);
        }
        command.arg(&name);
        command.args(cmd);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        tracing::debug!(
            container = %name,
            env_keys = ?keys,
            idle_timeout_ms = idle_timeout.as_millis() as u64,
            "podman exec_with_options"
        );

        let mut child = command
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("podman exec stdout pipe missing".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("podman exec stderr pipe missing".into()))?;

        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            stderr.read_to_end(&mut buf).await.map(|_| buf)
        });

        let mut stdout = BufReader::new(stdout);
        let mut stdout_buf = Vec::new();
        loop {
            match tokio::time::timeout(idle_timeout, stdout.read_until(b'\n', &mut stdout_buf))
                .await
            {
                Ok(Ok(0)) => break,
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    let _ = stderr_task.await;
                    return Err(SandboxError::ExecFailed(e.to_string()));
                }
                Err(_) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    let _ = stderr_task.await;
                    return Err(SandboxError::IdleTimeout {
                        idle_ms: idle_timeout.as_millis() as u64,
                    });
                }
            }
        }

        let status = child
            .wait()
            .await
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        let stderr_buf = stderr_task
            .await
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;

        Ok(ExecOutput {
            stdout: String::from_utf8_lossy(&stdout_buf).to_string(),
            stderr: String::from_utf8_lossy(&stderr_buf).to_string(),
            exit_code: status.code().unwrap_or(-1),
        })
    }

    async fn reuse_or_create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
        self.reuse_or_create_with_options(session_id, &CreateOptions::default())
            .await
    }

    async fn reuse_or_create_with_options(
        &self,
        session_id: Uuid,
        opts: &CreateOptions,
    ) -> Result<Sandbox, SandboxError> {
        let name = Self::container_name(session_id);
        if Self::find_container(&name).await?.is_some() {
            return Ok(Sandbox {
                state: Arc::new(Mutex::new(SandboxState {
                    id: SandboxId::new(),
                    destroyed: false,
                })),
                exec_fn: Arc::new(PodmanExec {
                    container_name: name,
                }),
            });
        }
        self.create_with_options(session_id, opts).await
    }
}

struct PodmanExec {
    container_name: String,
}

#[async_trait]
impl ExecFn for PodmanExec {
    async fn exec(&self, _id: SandboxId, cmd: &str) -> Result<ExecOutput, SandboxError> {
        let output = Command::new(PODMAN)
            .args(["exec", &self.container_name, "sh", "-c", cmd])
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
