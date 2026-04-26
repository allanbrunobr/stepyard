//! Startup crash recovery — the D8 three-phase reconcile (NFR11 / NFR12).
//!
//! Orchestrates the work that must happen BEFORE any [`Engine`](crate::Engine)
//! is constructed at process start. The binary's `execute` path calls
//! [`reconcile`] unconditionally; three phases run sequentially:
//!
//! 1. **Session reconciliation.** Every row in `sessions` whose `status` is
//!    still `running` is marked `failed`, with a `SignalReceived` event
//!    (`signal = "crash_recovery"`) appended to its log.
//! 2. **Container reconciliation.** Every live Docker container named
//!    `minion-session-<uuid>` whose UUID does NOT correspond to a currently
//!    `running` session is destroyed (NFR12 — idempotent).
//! 3. **Worktree pruning.** Stub for Epic 4.
//!
//! # Emit-before-IO exemption
//!
//! Exempt from emit-before-IO rule: runs before any live session exists at
//! startup — there is no in-flight step whose teardown we could corrupt.
//! Appending `SignalReceived` to an already-aborted session is benign: the
//! log is append-only and the status transition to `failed` is idempotent
//! (`UPDATE … WHERE status = 'running'`).
//!
//! # Why this lives in the harness crate
//!
//! The binary's `src/startup.rs` re-exports this module. Placing the logic
//! here lets `crates/stepyard-harness/tests/startup_reconcile.rs` exercise it
//! without the workspace growing a circular `minion-engine → minion-harness`
//! build edge.

use std::collections::HashSet;

use stepyard_core::Event;
use stepyard_sandbox_orchestrator::{DockerLifecycle, SandboxLifecycle};
use stepyard_session::{Session, SessionError, SessionId};
use sqlx::PgPool;
use tokio::process::Command;
use uuid::Uuid;

/// Counters returned by a successful [`reconcile`] call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileReport {
    pub sessions_reconciled: usize,
    pub containers_pruned: usize,
}

/// Errors raised by [`reconcile`].
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("session reconcile failed: {0}")]
    Session(#[from] SessionError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// The D8 three-phase startup reconcile.
///
/// Runs the three phases in order and returns a [`ReconcileReport`] with the
/// counters. `lifecycle` is a concrete [`DockerLifecycle`] (not a trait
/// object) because Phase 2 only targets Docker-owned containers — the
/// caller in `execute_v2` constructs a dedicated `DockerLifecycle::default()`
/// for reconcile even when the runtime lifecycle is `LocalShellLifecycle`.
pub async fn reconcile(
    pg: &PgPool,
    lifecycle: &DockerLifecycle,
) -> Result<ReconcileReport, ReconcileError> {
    // Exempt from emit-before-IO rule: runs before any live session exists at startup.
    let mut report = ReconcileReport::default();

    // ── Phase 1: session reconciliation ─────────────────────────────────
    // SELECT the set of sessions whose status is still `running`. For each,
    // append `SignalReceived { signal: "crash_recovery" }` to the log and
    // flip the row to `failed`. `Session::fail()` uses
    // `UPDATE … WHERE status = 'running'` — so a second pass is a true no-op.
    let running_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM sessions WHERE status = 'running'")
            .fetch_all(pg)
            .await?;

    for id in &running_ids {
        let session_id: SessionId = (*id).into();
        let mut session = Session::load(pg, session_id).await?;
        let payload = serde_json::to_value(Event::SignalReceived {
            signal: "crash_recovery".to_string(),
        })
        .expect("SignalReceived always serializes as a JSON object");
        session.append(payload).await?;
        session.fail().await?;
        report.sessions_reconciled += 1;
    }

    // ── Phase 2: container reconciliation ───────────────────────────────
    // Requery the `running` set — Phase 1 drained it, but a future refactor
    // may leave some rows alive (for example if we ever add per-tenant
    // selective reconcile). The logic stays correct either way.
    let still_running: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM sessions WHERE status = 'running'")
            .fetch_all(pg)
            .await?;
    let running_set: HashSet<Uuid> = still_running.into_iter().collect();

    // Docker's `--filter name=` is a SUBSTRING match, not a glob — the
    // Story AC writes `name=minion-session-*` descriptively, but passing a
    // literal `*` here makes Docker look for containers whose names contain
    // the asterisk character, which is never true. We pass the trailing-dash
    // prefix instead; `extract_session_uuid` below is the strict filter that
    // ensures we only ever destroy `minion-session-<uuid>` names.
    let ps = Command::new("docker")
        .args([
            "ps",
            "--filter",
            "name=minion-session-",
            "--format",
            "{{.Names}}",
        ])
        .output()
        .await;

    match ps {
        Ok(output) if output.status.success() => {
            let names = String::from_utf8_lossy(&output.stdout);
            for name in names.lines().map(str::trim).filter(|s| !s.is_empty()) {
                let Some(uuid) = extract_session_uuid(name) else {
                    tracing::warn!(
                        container = %name,
                        "ignoring container with unparseable session UUID",
                    );
                    continue;
                };
                if running_set.contains(&uuid) {
                    continue;
                }
                // `destroy_by_session` wraps `docker rm -f <name>` and
                // tolerates "No such container" stderr as success (NFR12).
                if let Err(e) = lifecycle.destroy_by_session(uuid).await {
                    tracing::warn!(
                        container = %name,
                        error = %e,
                        "destroy_by_session failed; continuing reconcile",
                    );
                    continue;
                }
                report.containers_pruned += 1;
            }
        }
        Ok(output) => {
            tracing::warn!(
                exit_code = output.status.code().unwrap_or(-1),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "docker ps returned non-zero; skipping container reconcile",
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "docker unavailable; skipping container reconcile",
            );
        }
    }

    // ── Phase 3: worktree pruning ───────────────────────────────────────
    // TODO(Epic 4): D8 two-phase prune — see Epic 4 Story N.M

    tracing::info!(
        sessions_reconciled = report.sessions_reconciled,
        containers_pruned = report.containers_pruned,
        "startup reconcile complete",
    );
    Ok(report)
}

/// Extract the session UUID from a container name of the form
/// `minion-session-<uuid>`. Returns `None` if the prefix is missing or the
/// suffix is not a valid UUID.
fn extract_session_uuid(container_name: &str) -> Option<Uuid> {
    let suffix = container_name.strip_prefix("minion-session-")?;
    Uuid::parse_str(suffix).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_session_uuid_parses_valid_name() {
        let uuid = Uuid::new_v4();
        let name = format!("minion-session-{uuid}");
        assert_eq!(extract_session_uuid(&name), Some(uuid));
    }

    #[test]
    fn extract_session_uuid_rejects_missing_prefix() {
        assert_eq!(
            extract_session_uuid("some-other-00000000-0000-0000-0000-000000000000"),
            None,
        );
    }

    #[test]
    fn extract_session_uuid_rejects_non_uuid_suffix() {
        assert_eq!(extract_session_uuid("minion-session-not-a-uuid"), None);
    }

    #[test]
    fn extract_session_uuid_rejects_empty_suffix() {
        assert_eq!(extract_session_uuid("minion-session-"), None);
    }

    #[test]
    fn extract_session_uuid_rejects_trailing_junk() {
        let uuid = Uuid::new_v4();
        let name = format!("minion-session-{uuid}-extra");
        assert_eq!(extract_session_uuid(&name), None);
    }

    #[test]
    fn reconcile_report_default_is_zero() {
        let r = ReconcileReport::default();
        assert_eq!(r.sessions_reconciled, 0);
        assert_eq!(r.containers_pruned, 0);
    }
}

/// Round 3 Story 6 — compile-time `Send` check for [`reconcile`].
///
/// `reconcile` is the D8 startup phase awaited by `execute_v2` before any
/// per-session engine spawns. Verifying its future is `Send` keeps it
/// `tokio::spawn`-safe for any future refactor that runs reconcile on a
/// dedicated task.
#[cfg(test)]
mod send_check {
    use super::*;

    fn assert_send_future<F: std::future::Future + Send>(_: F) {}

    #[allow(dead_code)]
    fn reconcile_future_is_send(pg: &PgPool, lifecycle: &DockerLifecycle) {
        assert_send_future(reconcile(pg, lifecycle));
    }
}
