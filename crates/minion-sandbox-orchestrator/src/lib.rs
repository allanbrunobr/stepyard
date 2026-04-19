//! Sandbox lifecycle abstractions for the Minion Engine v2 harness.
//!
//! This crate owns "how do we get a running execution environment for a
//! session and how do we tear it down?". It does NOT know what steps run
//! inside — that is `minion-harness`'s job (Story 2.3).
//!
//! # Key types
//!
//! * [`SandboxLifecycle`] — the trait the harness calls. Two impls ship here:
//!   - [`DockerLifecycle`] — real Docker via the `docker` CLI subprocess.
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
mod local;
pub mod mock;
mod sandbox;

pub use docker::{DockerLifecycle, DockerLifecycleConfig};
pub use local::LocalShellLifecycle;
pub use mock::{MockCall, MockLifecycle};
pub use sandbox::{ExecOutput, Sandbox, SandboxError, SandboxId};

use async_trait::async_trait;
use uuid::Uuid;

/// The contract every sandbox backend implements.
///
/// Implementors are `Send + Sync` so the harness can share a single
/// orchestrator across concurrent sessions (Invariante 9).
#[async_trait]
pub trait SandboxLifecycle: Send + Sync {
    /// Create a brand-new sandbox for this `session_id`.
    async fn create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError>;

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

    /// Return the live sandbox for this session if one already exists,
    /// otherwise create a new one. The default impl just calls `create` —
    /// backends that care about reuse (Docker) override it.
    async fn reuse_or_create(&self, session_id: Uuid) -> Result<Sandbox, SandboxError> {
        self.create(session_id).await
    }
}
