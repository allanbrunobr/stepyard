//! Sandbox lifecycle abstractions for the Stepyard Engine v2 harness.
//!
//! This crate owns "how do we get a running execution environment for a
//! session and how do we tear it down?". It does NOT know what steps run
//! inside — that is `stepyard-harness`'s job (Story 2.3).
//!
//! # Key types
//!
//! * [`SandboxLifecycle`] — the trait the harness calls. Two impls ship here:
//!   - [`DockerLifecycle`] — real Docker via the `docker` CLI subprocess.
//!   - [`PodmanLifecycle`] — real Podman via the `podman` CLI subprocess.
//!   - [`MockLifecycle`] — in-memory, zero daemon calls, used by tests.
//! * [`Sandbox`] — an opaque handle returned by `create`. Holds the id,
//!   provides `exec`, and goes to `Destroyed` once the orchestrator tears
//!   it down. Calls after that return [`SandboxError::Destroyed`] without
//!   panic (NFC3).
//! * [`SandboxId`] — newtype over `uuid::Uuid`. The container identifier
//!   that crosses process boundaries.
//!
//! # Invariants (NFC3, Invariante 3 of ARCHITECTURE.md)
//!
//! * Containers are cattle. Destroying and recreating a sandbox never loses
//!   state that matters — anything load-bearing is in the `Session` log.
//! * `create` is idempotent with respect to `session_id` when paired with
//!   [`SandboxLifecycle::reuse_or_create`]: the harness can call it every
//!   step and get either the cached container or a fresh one.

mod docker;
mod docker_errors;
mod local;
mod podman;
pub mod mock;
mod sandbox;
mod workspace;

pub use docker::{DockerLifecycle, DockerLifecycleConfig};
pub use local::LocalShellLifecycle;
pub use mock::{MockCall, MockLifecycle};
pub use podman::{PodmanLifecycle, PodmanLifecycleConfig};
pub use sandbox::{ExecOutput, Sandbox, SandboxError, SandboxId};
pub use workspace::{
    BranchStrategy, FinalizeReport, GitWorktreeManager, PruneReport, PrunedWorkspace,
    WorkflowOutcome, Workspace, WorkspaceError, WorkspaceManager,
};

use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

/// Network policy carried at sandbox creation time.
///
/// Docker/Podman support here is intentionally conservative: non-empty
/// allow/deny lists select the default bridge network today. Fine-grained
/// egress enforcement remains a provider-specific hardening layer.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NetworkOptions {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

/// Structured options used when creating a sandbox.
///
/// The default value preserves legacy behavior for callers that only know about
/// [`SandboxLifecycle::create`]. Providers that override
/// [`SandboxLifecycle::create_with_options`] may inspect these values.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CreateOptions {
    pub image: Option<String>,
    pub volumes: Vec<String>,
    pub cpus: Option<f64>,
    pub memory: Option<String>,
    pub dns: Vec<String>,
    pub network: NetworkOptions,
}

/// Structured execution options for sandbox backends.
///
/// Story 5.1 adds this as a non-breaking extension point: existing
/// implementations can inherit [`SandboxLifecycle::exec_with_options`]'s
/// default delegation to [`SandboxLifecycle::exec_with_env`], while migrated
/// backends can inspect `idle_timeout` when Story 5.2 wires real streaming
/// idle detection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOptions {
    pub env: HashMap<String, String>,
    pub idle_timeout: Option<Duration>,
}

/// The contract every sandbox backend implements.
///
/// Implementors are `Send + Sync` so the harness can share a single
/// orchestrator across concurrent sessions (Invariante 9).
#[async_trait]
pub trait SandboxLifecycle: Send + Sync {
    /// Create a brand-new sandbox for this `session_id`.
    async fn create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError>;

    /// Create a brand-new sandbox with structured provider options.
    ///
    /// Default delegation preserves existing implementors and makes the method
    /// additive. Providers that support resource limits, volumes, or network
    /// shaping override this method.
    async fn create_with_options(
        &self,
        session_id: Uuid,
        _opts: &CreateOptions,
    ) -> Result<Sandbox, SandboxError> {
        self.create(session_id).await
    }

    /// Tear down the sandbox with `id`. Safe to call on an already-destroyed
    /// sandbox — implementations should return `Ok(())` in that case.
    ///
    /// The harness prefers [`SandboxLifecycle::destroy_by_session`] for
    /// cancel/timeout/signal cleanup because backends like Docker cannot map
    /// an opaque [`SandboxId`] back to the container they created — only the
    /// session UUID carries that identity across the trait boundary.
    async fn destroy(&self, id: &SandboxId) -> Result<(), SandboxError>;

    /// Tear down every sandbox bound to this `session_id`. This is the
    /// teardown path the harness uses on cancel, timeout, and signal — the
    /// session UUID is what the backend can use to locate real resources
    /// (e.g. Docker looks up `minion-session-<uuid>`).
    ///
    /// The default implementation converts the UUID into a [`SandboxId`]
    /// and delegates to [`SandboxLifecycle::destroy`]. Backends whose
    /// [`SandboxLifecycle::destroy`] cannot find the real resource from a
    /// [`SandboxId`] alone MUST override this (see `DockerLifecycle`).
    ///
    /// Safe to call when no sandbox exists for the session — destruction is
    /// idempotent.
    async fn destroy_by_session(&self, session_id: Uuid) -> Result<(), SandboxError> {
        self.destroy(&SandboxId::from(session_id)).await
    }

    /// Execute `cmd` (argv form) inside the sandbox identified by `id`.
    /// No env injection — callers that need env pairs use [`exec_with_env`].
    async fn exec(&self, id: &SandboxId, cmd: &[String]) -> Result<ExecOutput, SandboxError>;

    /// Execute `cmd` with a structured env map inside the sandbox `id`.
    /// Required — no default impl, because a default that delegates to
    /// [`exec`] silently drops `env`, a production-visible bug. Every
    /// backend must decide explicitly how to propagate env pairs.
    async fn exec_with_env(
        &self,
        id: &SandboxId,
        cmd: &[String],
        env: &HashMap<String, String>,
    ) -> Result<ExecOutput, SandboxError>;

    /// Execute `cmd` with structured execution options. The default impl
    /// preserves current backend behavior by delegating to `exec_with_env`
    /// and ignoring `idle_timeout`; Story 5.2 backends override this to
    /// enforce output-idle detection.
    async fn exec_with_options(
        &self,
        id: &SandboxId,
        cmd: &[String],
        opts: &ExecOptions,
    ) -> Result<ExecOutput, SandboxError> {
        self.exec_with_env(id, cmd, &opts.env).await
    }

    /// Execute `cmd` with the caller's terminal attached to the sandbox.
    ///
    /// Defaulting to [`SandboxError::InteractiveNotSupported`] keeps this
    /// additive for non-container backends. Docker/Podman override it with
    /// `exec -it` so operators can debug a live session without bypassing
    /// the sandbox lifecycle abstraction.
    async fn exec_interactive(
        &self,
        _id: &SandboxId,
        _cmd: &[String],
    ) -> Result<i32, SandboxError> {
        Err(SandboxError::InteractiveNotSupported)
    }

    /// Return the live sandbox for this session if one already exists,
    /// otherwise create a new one. The default impl just calls `create` —
    /// backends that care about reuse (Docker) override it.
    async fn reuse_or_create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
        self.create(session_id).await
    }

    /// Return the live sandbox for this session if one already exists,
    /// otherwise create a new one with the supplied options.
    async fn reuse_or_create_with_options(
        &self,
        session_id: Uuid,
        opts: &CreateOptions,
    ) -> Result<Sandbox, SandboxError> {
        self.create_with_options(session_id, opts).await
    }
}

/// Round 3 Story 6 — compile-time `Send` checks for the
/// [`SandboxLifecycle`] trait-object surface.
///
/// The harness shares `Arc<dyn SandboxLifecycle>` across tasks
/// (Invariante 9). These checks are never invoked; the compiler typechecks
/// them so that any change which makes a method's returned future `!Send`
/// (e.g. switching to `#[async_trait(?Send)]`) breaks the build before
/// the harness can spawn it.
#[cfg(test)]
mod send_check {
    use super::*;

    fn assert_send_future<F: std::future::Future + Send>(_: F) {}

    #[allow(dead_code)]
    fn create_future_is_send(lifecycle: &dyn SandboxLifecycle, session_id: Uuid) {
        assert_send_future(lifecycle.create(session_id));
    }

    #[allow(dead_code)]
    fn create_with_options_future_is_send(
        lifecycle: &dyn SandboxLifecycle,
        session_id: Uuid,
        opts: &CreateOptions,
    ) {
        assert_send_future(lifecycle.create_with_options(session_id, opts));
    }

    #[allow(dead_code)]
    fn destroy_future_is_send(lifecycle: &dyn SandboxLifecycle, id: &SandboxId) {
        assert_send_future(lifecycle.destroy(id));
    }

    #[allow(dead_code)]
    fn destroy_by_session_future_is_send(
        lifecycle: &dyn SandboxLifecycle,
        session_id: Uuid,
    ) {
        assert_send_future(lifecycle.destroy_by_session(session_id));
    }

    #[allow(dead_code)]
    fn exec_future_is_send(
        lifecycle: &dyn SandboxLifecycle,
        id: &SandboxId,
        cmd: &[String],
    ) {
        assert_send_future(lifecycle.exec(id, cmd));
    }

    #[allow(dead_code)]
    fn exec_with_env_future_is_send(
        lifecycle: &dyn SandboxLifecycle,
        id: &SandboxId,
        cmd: &[String],
        env: &HashMap<String, String>,
    ) {
        assert_send_future(lifecycle.exec_with_env(id, cmd, env));
    }

    #[allow(dead_code)]
    fn exec_with_options_future_is_send(
        lifecycle: &dyn SandboxLifecycle,
        id: &SandboxId,
        cmd: &[String],
        opts: &ExecOptions,
    ) {
        assert_send_future(lifecycle.exec_with_options(id, cmd, opts));
    }

    #[allow(dead_code)]
    fn exec_interactive_future_is_send(
        lifecycle: &dyn SandboxLifecycle,
        id: &SandboxId,
        cmd: &[String],
    ) {
        assert_send_future(lifecycle.exec_interactive(id, cmd));
    }

    #[allow(dead_code)]
    fn reuse_or_create_future_is_send(
        lifecycle: &dyn SandboxLifecycle,
        session_id: Uuid,
    ) {
        assert_send_future(lifecycle.reuse_or_create(session_id));
    }

    #[allow(dead_code)]
    fn reuse_or_create_with_options_future_is_send(
        lifecycle: &dyn SandboxLifecycle,
        session_id: Uuid,
        opts: &CreateOptions,
    ) {
        assert_send_future(lifecycle.reuse_or_create_with_options(session_id, opts));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct EnvOnlyLifecycle {
        recorded_env: Arc<Mutex<Option<HashMap<String, String>>>>,
    }

    #[async_trait]
    impl SandboxLifecycle for EnvOnlyLifecycle {
        async fn create(&self, _session_id: Uuid) -> Result<Sandbox, SandboxError> {
            Err(SandboxError::CreateFailed("not used".into()))
        }

        async fn destroy(&self, _id: &SandboxId) -> Result<(), SandboxError> {
            Ok(())
        }

        async fn exec(
            &self,
            _id: &SandboxId,
            _cmd: &[String],
        ) -> Result<ExecOutput, SandboxError> {
            Err(SandboxError::ExecFailed("not used".into()))
        }

        async fn exec_with_env(
            &self,
            _id: &SandboxId,
            _cmd: &[String],
            env: &HashMap<String, String>,
        ) -> Result<ExecOutput, SandboxError> {
            *self.recorded_env.lock().await = Some(env.clone());
            Ok(ExecOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            })
        }
    }

    #[tokio::test]
    async fn exec_with_options_default_impl_delegates_to_exec_with_env() {
        let lifecycle = EnvOnlyLifecycle::default();
        let id = SandboxId::new();
        let cmd = vec!["true".to_string()];
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let opts = ExecOptions {
            env: env.clone(),
            idle_timeout: Some(Duration::from_secs(30)),
        };

        lifecycle
            .exec_with_options(&id, &cmd, &opts)
            .await
            .expect("default exec_with_options");

        assert_eq!(*lifecycle.recorded_env.lock().await, Some(env));
    }

    #[tokio::test]
    async fn exec_interactive_default_impl_is_not_supported() {
        let lifecycle = EnvOnlyLifecycle::default();
        let id = SandboxId::new();
        let cmd = vec!["sh".to_string()];

        let err = lifecycle
            .exec_interactive(&id, &cmd)
            .await
            .expect_err("default interactive exec should be unsupported");

        assert!(matches!(err, SandboxError::InteractiveNotSupported));
    }
}
