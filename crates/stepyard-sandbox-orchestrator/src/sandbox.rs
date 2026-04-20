//! The [`Sandbox`] handle and its supporting types.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

/// Newtype wrapper over [`Uuid`] — the stable cross-process identifier of a
/// sandbox. The backend maps this to its real container id (Docker container
/// hash, mock counter, etc.) internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SandboxId(pub Uuid);

impl SandboxId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for SandboxId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SandboxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for SandboxId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for SandboxId {
    fn from(u: Uuid) -> Self {
        Self(u)
    }
}

/// Result of executing a command inside a sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl ExecOutput {
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Domain errors returned by [`SandboxLifecycle`](crate::SandboxLifecycle) and
/// [`Sandbox`] operations.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The sandbox has been torn down; subsequent `exec` calls fail with
    /// this variant (NFC3). The harness treats this as a retryable signal
    /// because Session replay can reconstruct the step.
    #[error("sandbox {0} is destroyed")]
    Destroyed(SandboxId),

    /// The sandbox backend is unavailable (daemon down, missing binary, etc).
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),

    /// The `create` call failed.
    #[error("failed to create sandbox: {0}")]
    CreateFailed(String),

    /// The `destroy` call failed.
    #[error("failed to destroy sandbox: {0}")]
    DestroyFailed(String),

    /// A command executed inside the sandbox failed in a way other than a
    /// non-zero exit code (non-zero exits come back inside [`ExecOutput`]).
    #[error("exec failure: {0}")]
    ExecFailed(String),

    /// Catch-all for invariants the orchestrator detects at runtime.
    #[error("invalid state: {0}")]
    InvalidState(String),
}

/// Internal state shared between the [`Sandbox`] handle and any backend
/// method that needs to know whether the sandbox has been torn down.
///
/// Kept behind `Arc<Mutex<_>>` so `Sandbox` can be `Clone + Send + Sync` and
/// multiple harness tasks can hold handles to the same container.
#[derive(Debug)]
pub(crate) struct SandboxState {
    pub(crate) id: SandboxId,
    pub(crate) destroyed: bool,
}

/// Opaque handle to a live sandbox. Clone cheaply; under the hood it
/// reference-counts a shared state slot that tracks whether the backend has
/// destroyed the container.
#[derive(Clone)]
pub struct Sandbox {
    pub(crate) state: Arc<Mutex<SandboxState>>,
    pub(crate) exec_fn: Arc<dyn ExecFn>,
}

impl Sandbox {
    /// The stable cross-process identifier of this sandbox.
    pub async fn id(&self) -> SandboxId {
        self.state.lock().await.id
    }

    /// Whether the orchestrator has marked this sandbox destroyed. After
    /// this returns `true`, [`Self::exec`] returns [`SandboxError::Destroyed`].
    pub async fn is_destroyed(&self) -> bool {
        self.state.lock().await.destroyed
    }

    /// Run a shell command inside the sandbox. Blocks until completion.
    ///
    /// # Errors
    /// * [`SandboxError::Destroyed`] if the orchestrator tore down the
    ///   container before the call.
    /// * [`SandboxError::ExecFailed`] if the backend itself misbehaved (not
    ///   to be confused with a non-zero exit code from the guest command —
    ///   that comes back as `ExecOutput { exit_code: != 0 }`).
    pub async fn exec(&self, cmd: &str) -> Result<ExecOutput, SandboxError> {
        let (id, destroyed) = {
            let state = self.state.lock().await;
            (state.id, state.destroyed)
        };
        if destroyed {
            return Err(SandboxError::Destroyed(id));
        }
        self.exec_fn.exec(id, cmd).await
    }
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandbox").finish_non_exhaustive()
    }
}

/// Backend-provided implementation of "run this string inside container X".
/// Kept as a trait so backends can plug in without leaking their types into
/// the public `Sandbox` surface.
#[async_trait::async_trait]
pub(crate) trait ExecFn: Send + Sync {
    async fn exec(&self, id: SandboxId, cmd: &str) -> Result<ExecOutput, SandboxError>;
}
