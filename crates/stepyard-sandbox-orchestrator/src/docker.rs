//! [`DockerLifecycle`] — real-Docker backend via the `docker` CLI subprocess.
//!
//! Kept deliberately minimal: create, destroy, exec — nothing else. Workspace
//! copy, volume mounts, and resource limits live in the engine's legacy
//! `SandboxConfig` and will be folded in during Story 2.3 when the harness
//! refactor unifies sandbox config with lifecycle.

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

/// Tunables for [`DockerLifecycle`]. Override the image to use something
/// other than `alpine:latest` for the container body, and set a custom
/// shell command if `sh` is not on PATH.
#[derive(Debug, Clone)]
pub struct DockerLifecycleConfig {
    pub image: String,
    pub shell: String,
}

impl Default for DockerLifecycleConfig {
    fn default() -> Self {
        Self {
            image: "alpine:latest".into(),
            shell: "sh".into(),
        }
    }
}

/// Docker-backed [`SandboxLifecycle`] impl. Uses the system `docker` CLI.
/// Assumes the daemon is reachable from the current process (no remote
/// socket or TCP). Follow-up (Story 2.3) may swap this for `bollard`.
#[derive(Debug, Clone)]
pub struct DockerLifecycle {
    config: DockerLifecycleConfig,
}

impl Default for DockerLifecycle {
    fn default() -> Self {
        Self::new(DockerLifecycleConfig::default())
    }
}

impl DockerLifecycle {
    pub fn new(config: DockerLifecycleConfig) -> Self {
        Self { config }
    }

    /// Container-naming convention so `reuse_or_create` can find a previous
    /// container by session.
    fn container_name(session_id: Uuid) -> String {
        format!("minion-session-{session_id}")
    }

    /// Return the docker container id for a name, or None if not running.
    async fn find_container(name: &str) -> Result<Option<String>, SandboxError> {
        let output = Command::new("docker")
            .args(["ps", "-q", "--filter", &format!("name=^/{name}$")])
            .output()
            .await
            .map_err(|e| SandboxError::BackendUnavailable(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!stdout.is_empty()).then_some(stdout))
    }
}

#[async_trait]
impl SandboxLifecycle for DockerLifecycle {
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

        // `docker run -d --name <name> <image> sh -c "sleep infinity"`
        // The container stays alive so we can `docker exec` into it per step.
        let mut command = Command::new("docker");
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

        // We generate our own SandboxId rather than using the Docker hash,
        // so callers have a stable UUID even when the container is recreated.
        let id = SandboxId::new();
        Ok(Sandbox {
            state: Arc::new(Mutex::new(SandboxState {
                id,
                destroyed: false,
            })),
            exec_fn: Arc::new(DockerExec {
                container_name: name,
            }),
        })
    }

    async fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError> {
        // The container name includes the *session* id; a bare SandboxId
        // cannot address the running container. Callers should route teardown
        // through `SandboxLifecycle::destroy_by_session`. Left as a warning
        // (not an error) to preserve the trait's `destroy`-is-idempotent
        // contract for mock-backed callers.
        tracing::warn!(sandbox_id = %id, "DockerLifecycle::destroy called without session id — use destroy_by_session");
        Ok(())
    }

    async fn destroy_by_session(&self, session_id: Uuid) -> Result<(), SandboxError> {
        let name = Self::container_name(session_id);
        let output = Command::new("docker")
            .args(["rm", "-f", &name])
            .output()
            .await
            .map_err(|e| SandboxError::BackendUnavailable(e.to_string()))?;
        if !output.status.success() {
            // `docker rm -f` on an already-gone container is not an error
            // — destroy is idempotent. The classifier returns `None` for
            // that case and typed variants for everything else.
            if let Some(err) = classify_destroy_stderr(&output.stderr) {
                return Err(err);
            }
        }
        Ok(())
    }

    async fn exec(&self, id: &SandboxId, cmd: &[String]) -> Result<ExecOutput, SandboxError> {
        // The harness convention (Story 2.x) maps SandboxId back to
        // session_id by construction (`SandboxId::from(session_id.as_uuid())`),
        // so we can recover the container name here. This is the argv entry
        // point — the legacy `sh -c` path stays on [`DockerExec`] (Sandbox
        // handle carveout).
        let name = Self::container_name(*id.as_uuid());
        let mut docker_args: Vec<&str> = vec!["exec", &name];
        for arg in cmd {
            docker_args.push(arg);
        }
        let output = Command::new("docker")
            .args(&docker_args)
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
        // Argv-only enforcement point (D7, NFR-argv). Every env pair goes as
        // a separate `--env K=V` argv element — never concatenated into a
        // shell string. Keys sorted for deterministic argv ordering so tests
        // can assert on repeatability.
        let name = Self::container_name(*id.as_uuid());
        let mut pairs: Vec<(&String, &String)> = env.iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        let env_values: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();

        let mut docker_args: Vec<&str> =
            Vec::with_capacity(2 + env_values.len() * 2 + 1 + cmd.len());
        docker_args.push("exec");
        for value in &env_values {
            docker_args.push("--env");
            docker_args.push(value);
        }
        docker_args.push(&name);
        for arg in cmd {
            docker_args.push(arg);
        }

        // NFR-secrets (NFR8): log keys only, never values.
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        tracing::debug!(
            container = %name,
            env_keys = ?keys,
            argc = docker_args.len(),
            "docker exec_with_env"
        );

        let output = Command::new("docker")
            .args(&docker_args)
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

        // Argv-only enforcement point (D7, NFR-argv). Mirrors
        // `exec_with_env` but keeps stdout/stderr piped so the idle timer can
        // reset on each stdout read.
        let name = Self::container_name(*id.as_uuid());
        let mut pairs: Vec<(&String, &String)> = opts.env.iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        let env_values: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();

        let mut command = Command::new("docker");
        command.arg("exec");
        for value in &env_values {
            command.arg("--env").arg(value);
        }
        command.arg(&name);
        command.args(cmd);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        // NFR-secrets: log keys only, never values.
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        tracing::debug!(
            container = %name,
            env_keys = ?keys,
            idle_timeout_ms = idle_timeout.as_millis() as u64,
            "docker exec_with_options"
        );

        let mut child = command
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("docker exec stdout pipe missing".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("docker exec stderr pipe missing".into()))?;

        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            stderr.read_to_end(&mut buf).await.map(|_| buf)
        });

        let mut stdout = BufReader::new(stdout);
        let mut stdout_buf = Vec::new();
        loop {
            match tokio::time::timeout(
                idle_timeout,
                stdout.read_until(b'\n', &mut stdout_buf),
            )
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
        if let Some(_cid) = Self::find_container(&name).await? {
            return Ok(Sandbox {
                state: Arc::new(Mutex::new(SandboxState {
                    id: SandboxId::new(),
                    destroyed: false,
                })),
                exec_fn: Arc::new(DockerExec {
                    container_name: name,
                }),
            });
        }
        self.create_with_options(session_id, opts).await
    }
}

struct DockerExec {
    container_name: String,
}

#[async_trait]
impl ExecFn for DockerExec {
    async fn exec(&self, _id: SandboxId, cmd: &str) -> Result<ExecOutput, SandboxError> {
        let output = Command::new("docker")
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
