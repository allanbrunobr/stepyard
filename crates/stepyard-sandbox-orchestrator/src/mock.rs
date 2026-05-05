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
use crate::{CreateOptions, ExecOptions, SandboxLifecycle};

/// One recorded interaction with the mock backend.
#[derive(Debug, Clone, PartialEq)]
pub enum MockCall {
    Create {
        session_id: Uuid,
    },
    CreateWithOptions {
        session_id: Uuid,
        opts: CreateOptions,
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
    /// Structured record of `exec_with_options`: the full options struct is
    /// captured so tests can assert that fields like `idle_timeout` were not
    /// silently dropped.
    ExecWithOptions {
        id: SandboxId,
        cmd: Vec<String>,
        opts: ExecOptions,
    },
    ExecInteractive {
        id: SandboxId,
        cmd: Vec<String>,
    },
    ReuseOrCreate {
        session_id: Uuid,
    },
    ReuseOrCreateWithOptions {
        session_id: Uuid,
        opts: CreateOptions,
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
    exec_with_options_error: Arc<Mutex<Option<SandboxError>>>,
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

    /// Make the next `exec_with_options` call return `err` after recording
    /// the full [`ExecOptions`] payload. Used by harness tests that need to
    /// drive structured sandbox failures such as idle timeout.
    pub async fn set_exec_with_options_error(&self, err: SandboxError) {
        *self.exec_with_options_error.lock().await = Some(err);
    }

    fn make_sandbox(&self) -> Result<Sandbox, SandboxError> {
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
}

#[async_trait]
impl SandboxLifecycle for MockLifecycle {
    async fn create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
        self.calls.lock().await.push(MockCall::Create { session_id });
        self.make_sandbox()
    }

    async fn create_with_options(
        &self,
        session_id: Uuid,
        opts: &CreateOptions,
    ) -> Result<Sandbox, SandboxError> {
        self.calls.lock().await.push(MockCall::CreateWithOptions {
            session_id,
            opts: opts.clone(),
        });
        self.make_sandbox()
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

    async fn exec_with_options(
        &self,
        id: &SandboxId,
        cmd: &[String],
        opts: &ExecOptions,
    ) -> Result<ExecOutput, SandboxError> {
        self.calls.lock().await.push(MockCall::ExecWithOptions {
            id: *id,
            cmd: cmd.to_vec(),
            opts: opts.clone(),
        });
        if let Some(err) = self.exec_with_options_error.lock().await.take() {
            return Err(err);
        }
        Ok(ExecOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    async fn exec_interactive(
        &self,
        id: &SandboxId,
        cmd: &[String],
    ) -> Result<i32, SandboxError> {
        self.calls.lock().await.push(MockCall::ExecInteractive {
            id: *id,
            cmd: cmd.to_vec(),
        });
        Ok(0)
    }

    async fn reuse_or_create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
        self.calls
            .lock()
            .await
            .push(MockCall::ReuseOrCreate { session_id });
        self.create(session_id).await
    }

    async fn reuse_or_create_with_options(
        &self,
        session_id: Uuid,
        opts: &CreateOptions,
    ) -> Result<Sandbox, SandboxError> {
        self.calls
            .lock()
            .await
            .push(MockCall::ReuseOrCreateWithOptions {
                session_id,
                opts: opts.clone(),
            });
        self.create_with_options(session_id, opts).await
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
        // map — an impl that silently drops env would fail this.
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

    #[tokio::test]
    async fn exec_with_options_records_full_options() {
        let mock = MockLifecycle::new();
        let id = SandboxId::new();
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "BAR".to_string());
        let opts = ExecOptions {
            env,
            idle_timeout: Some(std::time::Duration::from_secs(30)),
        };
        let cmd = vec!["echo".to_string(), "hello".to_string()];

        mock.exec_with_options(&id, &cmd, &opts)
            .await
            .expect("exec_with_options");

        let calls = mock.calls().await;
        let recorded = calls
            .iter()
            .find_map(|c| match c {
                MockCall::ExecWithOptions { id, cmd, opts } => {
                    Some((*id, cmd.clone(), opts.clone()))
                }
                _ => None,
            })
            .expect("ExecWithOptions variant recorded");
        assert_eq!(recorded.0, id);
        assert_eq!(recorded.1, cmd);
        assert_eq!(recorded.2, opts);
    }

    #[tokio::test]
    async fn exec_interactive_records_command() {
        let mock = MockLifecycle::new();
        let id = SandboxId::new();
        let cmd = vec!["sh".to_string()];

        let status = mock
            .exec_interactive(&id, &cmd)
            .await
            .expect("mock interactive exec");

        assert_eq!(status, 0);
        assert_eq!(
            mock.calls().await,
            vec![MockCall::ExecInteractive { id, cmd }]
        );
    }
}
