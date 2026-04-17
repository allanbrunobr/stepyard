//! [`MockLifecycle`] — in-memory sandbox backend for tests.
//!
//! Records every call (create, destroy, exec) on a shared [`Vec<MockCall>`]
//! so tests can assert on ordering. No Docker daemon touched; no subprocess
//! spawned.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::sandbox::{ExecFn, ExecOutput, Sandbox, SandboxError, SandboxId, SandboxState};
use crate::SandboxLifecycle;

/// One recorded interaction with the mock backend.
#[derive(Debug, Clone, PartialEq)]
pub enum MockCall {
    Create {
        session_id: Uuid,
    },
    Destroy {
        id: SandboxId,
    },
    Exec {
        id: SandboxId,
        cmd: String,
    },
    /// Structured record of `exec_with_env`: env keys/values captured
    /// verbatim so tests can assert on exact pairs (D3, NFR22).
    ExecWithEnv {
        id: SandboxId,
        cmd: Vec<String>,
        env: HashMap<String, String>,
    },
    ReuseOrCreate {
        session_id: Uuid,
    },
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

    async fn exec(&self, id: &SandboxId, cmd: &[String]) -> Result<ExecOutput, SandboxError> {
        let cmd_str = cmd.join(" ");
        self.calls.lock().await.push(MockCall::Exec {
            id: *id,
            cmd: cmd_str.clone(),
        });
        let mut overrides = self.exec_overrides.lock().await;
        if let Some(pos) = overrides.iter().position(|(c, _)| c == &cmd_str) {
            let (_, output) = overrides.remove(pos);
            return Ok(output);
        }
        Ok(ExecOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    async fn exec_with_env(
        &self,
        id: &SandboxId,
        cmd: &[String],
        env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError> {
        self.calls.lock().await.push(MockCall::ExecWithEnv {
            id: *id,
            cmd: cmd.to_vec(),
            env: env.clone(),
        });
        Ok(ExecOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exec_with_env_records_full_env_pairs() {
        // AC5: calling exec_with_env on MockLifecycle records the exact env
        // map — a default impl that silently drops env would fail this.
        let mock = MockLifecycle::new();
        let id = SandboxId::new();
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "BAR".to_string());
        let cmd = vec!["echo".to_string(), "hello".to_string()];

        mock.exec_with_env(&id, &cmd, &env)
            .await
            .expect("exec_with_env");

        let calls = mock.calls().await;
        let (rec_id, rec_cmd, rec_env) = calls
            .iter()
            .find_map(|c| match c {
                MockCall::ExecWithEnv { id, cmd, env } => Some((*id, cmd.clone(), env.clone())),
                _ => None,
            })
            .expect("ExecWithEnv variant recorded");
        assert_eq!(rec_id, id);
        assert_eq!(rec_cmd, cmd);
        assert_eq!(rec_env, env);
    }

    type ExecLog = Arc<Mutex<Vec<(SandboxId, Vec<String>)>>>;

    /// Stub that implements `exec` but deliberately does NOT override
    /// `exec_with_env`. Used to prove the default impl delegates to `exec`.
    #[derive(Default)]
    struct StubLifecycle {
        exec_calls: ExecLog,
    }

    #[async_trait]
    impl SandboxLifecycle for StubLifecycle {
        async fn create(&self, _session_id: Uuid) -> Result<Sandbox, SandboxError> {
            unreachable!("stub is not used via create in these tests")
        }

        async fn destroy(&self, _id: &SandboxId) -> Result<(), SandboxError> {
            Ok(())
        }

        async fn exec(&self, id: &SandboxId, cmd: &[String]) -> Result<ExecOutput, SandboxError> {
            self.exec_calls.lock().await.push((*id, cmd.to_vec()));
            Ok(ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
    }

    #[tokio::test]
    async fn default_exec_with_env_delegates_to_exec() {
        // AC6: a type that does NOT override exec_with_env sees its `exec`
        // called by the trait default (env is dropped, backward-compat).
        let stub = StubLifecycle::default();
        let id = SandboxId::new();
        let mut env = HashMap::new();
        env.insert("DROPPED".to_string(), "ignored".to_string());
        let cmd = vec!["true".to_string()];

        stub.exec_with_env(&id, &cmd, &env)
            .await
            .expect("default delegation");

        let recorded = stub.exec_calls.lock().await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, id);
        assert_eq!(recorded[0].1, cmd);
    }
}
