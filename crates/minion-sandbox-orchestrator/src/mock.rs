//! [`MockLifecycle`] — in-memory sandbox backend for tests.
//!
//! Records every call (create, destroy, exec) on a shared [`Vec<MockCall>`]
//! so tests can assert on ordering. No Docker daemon touched; no subprocess
//! spawned.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::sandbox::{ExecFn, ExecOutput, Sandbox, SandboxError, SandboxId, SandboxState};
use crate::SandboxLifecycle;

/// One recorded interaction with the mock backend.
#[derive(Debug, Clone, PartialEq)]
pub enum MockCall {
    Create { session_id: Uuid },
    Destroy { id: SandboxId },
    Exec { id: SandboxId, cmd: String },
    ReuseOrCreate { session_id: Uuid },
}

/// In-memory [`SandboxLifecycle`] that never calls Docker and records every
/// call on its internal `calls` vector. Use [`Self::calls`] to inspect the
/// sequence from a test.
#[derive(Default, Clone)]
pub struct MockLifecycle {
    calls: Arc<Mutex<Vec<MockCall>>>,
    // When an entry exists, an `exec` with that cmd returns this preset
    // output — lets tests dictate what the mock reports without running any
    // real command.
    exec_overrides: Arc<Mutex<Vec<(String, ExecOutput)>>>,
}

impl MockLifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of every call so far, in chronological order.
    pub async fn calls(&self) -> Vec<MockCall> {
        self.calls.lock().await.clone()
    }

    /// Preset a response for a specific `cmd`. Next `exec(cmd)` pops this
    /// and returns it; if nothing is preset the default is an `ExecOutput`
    /// with empty stdout, empty stderr, exit 0.
    pub async fn set_exec_response(&self, cmd: &str, output: ExecOutput) {
        self.exec_overrides
            .lock()
            .await
            .push((cmd.to_string(), output));
    }
}

#[async_trait]
impl SandboxLifecycle for MockLifecycle {
    async fn create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
        self.calls.lock().await.push(MockCall::Create { session_id });
        let id = SandboxId::new();
        Ok(Sandbox {
            state: Arc::new(Mutex::new(SandboxState {
                id,
                destroyed: false,
            })),
            exec_fn: Arc::new(MockExec {
                calls: self.calls.clone(),
                overrides: self.exec_overrides.clone(),
            }),
        })
    }

    async fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError> {
        self.calls.lock().await.push(MockCall::Destroy { id: *id });
        Ok(())
    }

    async fn reuse_or_create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
        self.calls
            .lock()
            .await
            .push(MockCall::ReuseOrCreate { session_id });
        self.create(session_id).await
    }
}

struct MockExec {
    calls: Arc<Mutex<Vec<MockCall>>>,
    overrides: Arc<Mutex<Vec<(String, ExecOutput)>>>,
}

#[async_trait]
impl ExecFn for MockExec {
    async fn exec(&self, id: SandboxId, cmd: &str) -> Result<ExecOutput, SandboxError> {
        self.calls.lock().await.push(MockCall::Exec {
            id,
            cmd: cmd.to_string(),
        });
        let mut overrides = self.overrides.lock().await;
        if let Some(pos) = overrides.iter().position(|(c, _)| c == cmd) {
            let (_, output) = overrides.remove(pos);
            return Ok(output);
        }
        Ok(ExecOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }
}

/// Helper: mark a live [`Sandbox`] as destroyed without going through a
/// backend. Tests use this to verify the [`SandboxError::Destroyed`] path
/// without having to implement a stateful mock destroy.
pub async fn mark_destroyed(sandbox: &Sandbox) {
    let mut state = sandbox.state.lock().await;
    state.destroyed = true;
}
