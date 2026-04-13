//! [`DockerLifecycle`] — real-Docker backend via the `docker` CLI subprocess.
//!
//! Kept deliberately minimal: create, destroy, exec — nothing else. Workspace
//! copy, volume mounts, and resource limits live in the engine's legacy
//! `SandboxConfig` and will be folded in during Story 2.3 when the harness
//! refactor unifies sandbox config with lifecycle.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::sandbox::{ExecFn, ExecOutput, Sandbox, SandboxError, SandboxId, SandboxState};
use crate::SandboxLifecycle;

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
        let name = Self::container_name(session_id);

        // `docker run -d --name <name> <image> sh -c "sleep infinity"`
        // The container stays alive so we can `docker exec` into it per step.
        let output = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &name,
                "--label",
                &format!("session_id={session_id}"),
                &self.config.image,
                &self.config.shell,
                "-c",
                "trap : TERM INT; sleep infinity & wait",
            ])
            .output()
            .await
            .map_err(|e| SandboxError::BackendUnavailable(e.to_string()))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(SandboxError::CreateFailed(err));
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
        // We only have the SandboxId here; the container name includes the
        // *session* id. The harness (Story 2.3) will track the mapping. For
        // now, destroy-by-SandboxId is only reachable via MockLifecycle; a
        // real Docker teardown happens via `destroy_by_session`.
        tracing::warn!(sandbox_id = %id, "DockerLifecycle::destroy called without session id — use destroy_by_session");
        Ok(())
    }

    async fn reuse_or_create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
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
        self.create(session_id).await
    }
}

impl DockerLifecycle {
    /// Teardown helper that actually works in practice — the harness knows
    /// the session id and can call this directly. `destroy` on the trait
    /// alone is kept for MockLifecycle parity.
    pub async fn destroy_by_session(&self, session_id: Uuid) -> Result<(), SandboxError> {
        let name = Self::container_name(session_id);
        let output = Command::new("docker")
            .args(["rm", "-f", &name])
            .output()
            .await
            .map_err(|e| SandboxError::BackendUnavailable(e.to_string()))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            // `docker rm -f` on a non-existent container is not an error
            // for our purposes — destroy is idempotent.
            if !err.contains("No such container") {
                return Err(SandboxError::DestroyFailed(err));
            }
        }
        Ok(())
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
