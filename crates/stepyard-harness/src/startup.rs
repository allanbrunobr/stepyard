//! Startup crash recovery — the D8 three-phase reconcile (NFR11 / NFR12).
//!
//! Orchestrates the work that must happen BEFORE any [`Engine`](crate::Engine)
//! is constructed at process start. The binary's `execute` path calls
//! [`reconcile`] unconditionally; three phases run sequentially:
//!
//! 1. **Session reconciliation.** Every row in `sessions` whose `status` is
//!    still `running` AND whose owning process is gone is marked `failed`,
//!    with a `SignalReceived` event (`signal = "crash_recovery"`) appended to
//!    its log. "Owner is gone" means two things: the session started more
//!    than [`LEASE_GRACE`] ago, and nobody holds its [`SessionLease`].
//! 2. **Container reconciliation.** A live Docker container named
//!    `stepyard-session-<uuid>` is destroyed only when THIS database knows
//!    the session and it is no longer `running` (NFR12 — idempotent).
//!    Sessions spared in phase 1 are still `running`, so their containers are
//!    kept; a UUID this database does not know belongs to another database
//!    or project sharing the Docker daemon and is left alone (logged).
//! 3. **Worktree pruning.** Startup-only stale worktree cleanup.
//!
//! # Liveness: why the lease exists
//!
//! Several `stepyard execute` processes can share one database (two
//! terminals, two dashboard dispatches, two CI jobs). A reconcile that
//! treated every `running` row as abandoned would fail the other process's
//! live session and `docker rm -f` its container, losing the work in
//! `/workspace`. The owner therefore holds a PostgreSQL session-level
//! advisory lock keyed by the session id for as long as the session runs
//! ([`SessionLease`]). PostgreSQL releases that lock when the owner's
//! connection closes — including when the process crashes or is killed — so
//! `pg_try_advisory_lock` succeeding is direct evidence that the owner is
//! gone. [`LEASE_GRACE`] covers the short window between inserting the
//! session row and taking the lease.
//!
//! # Emit-before-IO exemption
//!
//! Exempt from emit-before-IO rule: only sessions whose owner is gone are
//! touched, so there is no in-flight step whose teardown we could corrupt.
//! Appending `SignalReceived` to an already-aborted session is benign: the
//! log is append-only and the status transition to `failed` is idempotent
//! (`UPDATE … WHERE status = 'running'`).
//!
//! # Why this lives in the harness crate
//!
//! The binary's `src/startup.rs` re-exports this module. Placing the logic
//! here lets `crates/stepyard-harness/tests/startup_reconcile.rs` exercise it
//! without the workspace growing a circular `stepyard → stepyard-harness`
//! build edge.

use std::collections::HashSet;
use std::time::Duration;

use sqlx::{Connection, PgConnection, PgPool};
use stepyard_core::{Event, Signal};
use stepyard_sandbox_orchestrator::{DockerLifecycle, SandboxLifecycle, WorkspaceManager};
use stepyard_session::{Session, SessionError, SessionId};
use tokio::process::Command;
use uuid::Uuid;

/// Counters returned by a successful [`reconcile`] call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileReport {
    pub sessions_reconciled: usize,
    /// `running` sessions left alone because their owner is alive (it holds
    /// the [`SessionLease`]) or because they started within [`LEASE_GRACE`].
    pub sessions_spared: usize,
    pub containers_pruned: usize,
    pub worktrees_pruned: usize,
    pub worktrees_preserved: usize,
}

/// Errors raised by [`reconcile`].
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("session reconcile failed: {0}")]
    Session(#[from] SessionError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// How long a freshly inserted `running` session is protected from reconcile
/// before its owner must hold the [`SessionLease`]. The owner takes the lease
/// right after the INSERT, so this only has to cover milliseconds; the margin
/// absorbs clock skew between the database and slow startups.
pub const LEASE_GRACE: Duration = Duration::from_secs(60);

/// Advisory-lock key for a session's liveness lease: the first 8 bytes of the
/// session UUID. A collision can only make reconcile spare a dead session
/// (fail safe); it can never make it touch a live one.
pub fn session_lease_key(session_id: Uuid) -> i64 {
    let bytes = session_id.as_bytes();
    let mut head = [0u8; 8];
    head.copy_from_slice(&bytes[..8]);
    i64::from_be_bytes(head)
}

/// Errors raised by [`SessionLease::acquire`].
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("session {0} lease is already held by another connection")]
    AlreadyHeld(Uuid),
}

/// Proof that the current process owns a `running` session.
///
/// Holds a PostgreSQL session-level advisory lock on a dedicated connection
/// (detached from the pool so the pool never recycles it). The lock lives as
/// long as this value: dropping it closes the connection, and PostgreSQL
/// releases the lock — which is also what happens if the process dies.
#[derive(Debug)]
pub struct SessionLease {
    conn: Option<PgConnection>,
    session_id: Uuid,
}

impl SessionLease {
    /// Take the lease for `session_id`. Call it right after creating the
    /// session row and keep the value alive until the session is terminal.
    pub async fn acquire(pg: &PgPool, session_id: Uuid) -> Result<Self, LeaseError> {
        let mut conn = pg.acquire().await?.detach();
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(session_lease_key(session_id))
            .fetch_one(&mut conn)
            .await?;
        if !got {
            return Err(LeaseError::AlreadyHeld(session_id));
        }
        Ok(Self {
            conn: Some(conn),
            session_id,
        })
    }

    /// The session this lease protects.
    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    /// Release the lease explicitly and close its connection. Dropping the
    /// value has the same effect once the server notices the closed socket.
    pub async fn release(mut self) -> Result<(), sqlx::Error> {
        if let Some(mut conn) = self.conn.take() {
            sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(session_lease_key(self.session_id))
                .execute(&mut conn)
                .await?;
            conn.close().await?;
        }
        Ok(())
    }
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
    reconcile_with_workspace_manager(pg, lifecycle, None).await
}

/// Variant of [`reconcile`] used by binaries that have a configured workspace
/// manager at startup. Keeping [`reconcile`] as the two-argument entry point
/// preserves the existing test and library API for callers that only need
/// session/container cleanup.
pub async fn reconcile_with_workspace_manager(
    pg: &PgPool,
    lifecycle: &DockerLifecycle,
    workspace_manager: Option<&dyn WorkspaceManager>,
) -> Result<ReconcileReport, ReconcileError> {
    // Exempt from emit-before-IO rule: runs before any live session exists at startup.
    let mut report = ReconcileReport::default();

    // ── Phase 1: session reconciliation ─────────────────────────────────
    // Candidates are `running` rows older than LEASE_GRACE. For each, try the
    // owner's advisory lock on one dedicated connection: if another
    // connection holds it, the owner is alive and the session is spared.
    // Only when we get the lock (owner gone) do we append
    // `SignalReceived { signal: "crash_recovery" }` and flip the row to
    // `failed`; then we release the lock. `Session::fail()` uses
    // `UPDATE … WHERE status = 'running'` — so a second pass is a true no-op.
    let candidates: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE status = 'running' \
         AND started_at < NOW() - make_interval(secs => $1)",
    )
    .bind(LEASE_GRACE.as_secs_f64())
    .fetch_all(pg)
    .await?;
    let running_total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE status = 'running'")
            .fetch_one(pg)
            .await?;
    report.sessions_spared = usize::try_from(running_total)
        .unwrap_or(usize::MAX)
        .saturating_sub(candidates.len());

    if !candidates.is_empty() {
        let mut probe = pg.acquire().await?;
        for id in &candidates {
            let key = session_lease_key(*id);
            let owner_gone: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
                .bind(key)
                .fetch_one(&mut *probe)
                .await?;
            if !owner_gone {
                report.sessions_spared += 1;
                continue;
            }
            let outcome = fail_abandoned_session(pg, *id).await;
            // Release before propagating any error so the probe connection
            // goes back to the pool without a lock attached.
            sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(key)
                .execute(&mut *probe)
                .await?;
            if outcome? {
                report.sessions_reconciled += 1;
            }
        }
    }

    // ── Phase 2: container reconciliation ───────────────────────────────
    // Requery the `running` set — Phase 1 drained it, but a future refactor
    // may leave some rows alive (for example if we ever add per-tenant
    // selective reconcile). The logic stays correct either way.
    // Docker's `--filter name=` is a SUBSTRING match, not a glob — passing a
    // literal `*` would look for an asterisk. `extract_session_uuid` below is
    // the strict filter that keeps only `stepyard-session-<uuid>` names.
    let ps = Command::new("docker")
        .args([
            "ps",
            "--filter",
            "name=stepyard-session-",
            "--format",
            "{{.Names}}",
        ])
        .output()
        .await;

    match ps {
        Ok(output) if output.status.success() => {
            let names = String::from_utf8_lossy(&output.stdout);
            let mut found: Vec<(String, Uuid)> = Vec::new();
            for name in names.lines().map(str::trim).filter(|s| !s.is_empty()) {
                match extract_session_uuid(name) {
                    Some(uuid) => found.push((name.to_string(), uuid)),
                    None => tracing::warn!(
                        container = %name,
                        "ignoring container with unparseable session UUID",
                    ),
                }
            }
            let uuids: Vec<Uuid> = found.iter().map(|(_, u)| *u).collect();
            // Only sessions this database knows to be over. A running session
            // (live owner, or spared by the grace window) keeps its container,
            // and an unknown UUID belongs to someone else's database.
            let known_dead: HashSet<Uuid> = if uuids.is_empty() {
                HashSet::new()
            } else {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM sessions WHERE id = ANY($1) AND status <> 'running'",
                )
                .bind(&uuids)
                .fetch_all(pg)
                .await?
                .into_iter()
                .collect()
            };
            for (name, uuid) in found {
                if !known_dead.contains(&uuid) {
                    tracing::debug!(
                        container = %name,
                        "keeping container: session running or unknown to this database",
                    );
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
    if let Some(workspace_manager) = workspace_manager {
        match workspace_manager.prune().await {
            Ok(prune_report) => {
                report.worktrees_pruned += prune_report.worktrees_pruned as usize;
                report.worktrees_preserved += prune_report.worktrees_preserved as usize;
                for pruned in prune_report.pruned {
                    let event = Event::WorkspacePruned {
                        path: pruned.path,
                        reason: pruned.reason,
                    };
                    tracing::info!(
                        event = %serde_json::to_string(&event)
                            .expect("WorkspacePruned always serializes"),
                        "workspace pruned during startup reconcile",
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "workspace prune failed; continuing startup reconcile",
                );
            }
        }
    }

    tracing::info!(
        sessions_reconciled = report.sessions_reconciled,
        sessions_spared = report.sessions_spared,
        containers_pruned = report.containers_pruned,
        worktrees_pruned = report.worktrees_pruned,
        worktrees_preserved = report.worktrees_preserved,
        "startup reconcile complete",
    );
    Ok(report)
}

/// Append `crash_recovery` and flip an abandoned session to `failed`.
/// Returns `false` when the row stopped being `running` between the
/// candidate query and the lock (its owner finished it), in which case
/// nothing is written.
async fn fail_abandoned_session(pg: &PgPool, id: Uuid) -> Result<bool, ReconcileError> {
    let session_id: SessionId = id.into();
    let mut session = Session::load(pg, session_id).await?;
    if session.status() != stepyard_session::SessionStatus::Running {
        return Ok(false);
    }
    let payload = serde_json::to_value(Event::SignalReceived {
        signal: Signal::from("crash_recovery"),
    })
    .expect("SignalReceived always serializes as a JSON object");
    session.append(payload).await?;
    session.fail().await?;
    Ok(true)
}

/// Extract the session UUID from a container name of the form
/// `stepyard-session-<uuid>`. Returns `None` if the prefix is missing or the
/// suffix is not a valid UUID.
fn extract_session_uuid(container_name: &str) -> Option<Uuid> {
    let suffix = container_name.strip_prefix("stepyard-session-")?;
    Uuid::parse_str(suffix).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_session_uuid_parses_valid_name() {
        let uuid = Uuid::new_v4();
        let name = format!("stepyard-session-{uuid}");
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
        assert_eq!(extract_session_uuid("stepyard-session-not-a-uuid"), None);
    }

    #[test]
    fn extract_session_uuid_rejects_empty_suffix() {
        assert_eq!(extract_session_uuid("stepyard-session-"), None);
    }

    #[test]
    fn extract_session_uuid_rejects_trailing_junk() {
        let uuid = Uuid::new_v4();
        let name = format!("stepyard-session-{uuid}-extra");
        assert_eq!(extract_session_uuid(&name), None);
    }

    #[test]
    fn session_lease_key_is_the_big_endian_head_of_the_uuid() {
        let id = Uuid::parse_str("01234567-89ab-cdef-0123-456789abcdef").unwrap();
        assert_eq!(session_lease_key(id), 0x0123_4567_89ab_cdef_i64);
        let high = Uuid::parse_str("ffffffff-ffff-ffff-0000-000000000000").unwrap();
        assert_eq!(session_lease_key(high), -1);
    }

    #[test]
    fn reconcile_report_default_is_zero() {
        let r = ReconcileReport::default();
        assert_eq!(r.sessions_reconciled, 0);
        assert_eq!(r.sessions_spared, 0);
        assert_eq!(r.containers_pruned, 0);
        assert_eq!(r.worktrees_pruned, 0);
        assert_eq!(r.worktrees_preserved, 0);
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
