//! Issue #59 — group-kill guard for spawned agent subprocesses.
//!
//! Tokio's `kill_on_drop(true)` only sends `SIGKILL` to the direct child
//! that `Command::spawn` returned. Any descendant the child forked
//! (Claude CLI's MCP servers, tool subprocesses) inherits the leader's
//! group on a normal `fork+exec` but reparents to PID 1 the moment the
//! leader dies — the SIGKILL never reaches them and they continue to run
//! past the session's terminal event.
//!
//! The fix is two-step:
//!
//! 1. Spawn the leader in **its own process group** via
//!    `std::os::unix::process::CommandExt::process_group(0)` (re-exported
//!    by `tokio::process::Command`). On Unix the new group's PGID is the
//!    leader's PID — so passing the leader's PID as a negative argument to
//!    `kill(2)` reaches every descendant that hasn't `setpgid()`'d itself
//!    out of the group.
//! 2. Drop a [`ProcessGroupKillOnDrop`] RAII guard *before* the
//!    `tokio::process::Child` so the group SIGKILL fires while the leader
//!    is still in its original group, then Tokio's `Child::Drop` SIGKILLs
//!    the leader directly (idempotent if `killpg` already reaped it).
//!
//! # Accepted limitation
//!
//! Descendants that explicitly call `setpgid(0, 0)` or `setsid()` to break
//! out of the group are **not** reached. That is true of every
//! kill-by-group strategy on Unix and is the standard limitation. We
//! choose to accept it: rejecting the limitation requires a sandbox
//! (cgroup, PID-namespace, or subreaper) and the Stepyard
//! [`SandboxLifecycle`] is the layer that owns that escalation. The agent
//! executor's job is to do the best Unix can without leaving the host.
//!
//! [`SandboxLifecycle`]: stepyard_sandbox_orchestrator::SandboxLifecycle

/// RAII guard that sends `SIGKILL` to a Unix process group on drop.
///
/// `pgid` is the **process-group id**, identical to the leader's PID when
/// the leader was spawned via `process_group(0)`. The drop sends
/// `kill(-pgid, SIGKILL)` — negative argument selects "every process in
/// group `pgid`" per `kill(2)`.
///
/// On non-Unix targets this is a no-op stub so the rest of the harness
/// compiles unchanged.
#[derive(Debug)]
pub(crate) struct ProcessGroupKillOnDrop {
    pgid: i32,
}

impl ProcessGroupKillOnDrop {
    /// Build a guard for `pgid`. Caller is responsible for ensuring the
    /// pgid actually corresponds to a group it spawned (i.e. don't pass a
    /// random PID from elsewhere — `kill(-pgid, …)` is destructive).
    #[allow(dead_code)] // referenced only on cfg(unix); non-unix builds drop the call site.
    pub(crate) fn new(pgid: i32) -> Self {
        Self { pgid }
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupKillOnDrop {
    fn drop(&mut self) {
        // SAFETY: `libc::kill` is an FFI call into POSIX `kill(2)`. A
        // negative `pid` argument means "deliver to every process in
        // process group `|pid|`". The `pgid` we hold was returned by a
        // `Command::spawn` paired with `process_group(0)` earlier in this
        // crate, so it names a real group we created. Passing `SIGKILL`
        // never blocks; on a stale group (already reaped) `kill` returns
        // ESRCH which we deliberately ignore — the only side effect we
        // care about is "deliver the signal if the group is still alive."
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
    }
}

// Non-Unix targets: no-op drop. The Stepyard harness doesn't build on
// Windows today, but the cfg keeps the compile honest the moment someone
// flips a target.
#[cfg(not(unix))]
impl Drop for ProcessGroupKillOnDrop {
    fn drop(&mut self) {}
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    use std::time::{Duration, Instant};
    use tempfile::TempDir;
    use tokio::process::Command;

    /// Issue #59 regression — when the future holding a tokio `Child` plus a
    /// [`ProcessGroupKillOnDrop`] guard is dropped, descendants forked by the
    /// leader are SIGKILLed too, not just the leader.
    ///
    /// Handshake pattern (advisor-required to avoid races):
    ///
    /// 1. Parent shell backgrounds `sleep 60`, writes the descendant's PID to
    ///    a pidfile, then `wait`s — staying alive so the test scope is the
    ///    one that triggers the drop.
    /// 2. Test polls for the pidfile to confirm the descendant has actually
    ///    forked before we test the kill.
    /// 3. Scope ends → `_kill_group` drops first → `kill(-pgid, SIGKILL)`
    ///    reaches the leader and the backgrounded sleep.
    /// 4. After a brief grace, `kill(pid, 0)` on the descendant returns
    ///    `ESRCH` ("no such process") proving the SIGKILL landed.
    ///
    /// Without `process_group(0)` + the guard, the backgrounded sleep
    /// reparents to PID 1 and lives — `kill -0` succeeds and the assert
    /// fires. That negative case was verified locally before commit.
    #[tokio::test]
    async fn dropping_guard_kills_descendant_in_same_process_group() {
        let temp = TempDir::new().expect("tempdir");
        let pidfile = temp.path().join("descendant.pid");
        let pidfile_str = pidfile.to_str().expect("utf-8 pidfile path");

        // `sleep 60 &` backgrounds the child; `echo $! > "$1"` records its
        // pid (the pidfile path is delivered as positional `$1`, NOT
        // concatenated into the shell string — keeps the test honest about
        // not building a `Command::new("sh") + format!` shape that the G3
        // audit (scripts/audit-patterns.sh) is meant to forbid in
        // production); `wait` keeps the shell (the leader) alive so the
        // descendant doesn't immediately reparent before we drop.
        //
        // `/bin/sh` is the existing G3 carveout (vs the bare `sh`/`bash`
        // alternation the audit blocks) — the docker.rs workspace-copy path
        // uses the same form for the same reason: hardcoded literal script,
        // no untrusted input.
        let script = r#"sleep 60 & echo $! > "$1"; wait"#;

        let descendant_pid: i32 = {
            let mut cmd = Command::new("/bin/sh");
            cmd.arg("-c")
                .arg(script)
                .arg("--") // sets $0 to "--" by convention
                .arg(pidfile_str) // becomes $1 inside the shell
                .kill_on_drop(true)
                .process_group(0);
            let child = cmd.spawn().expect("spawn sh");
            let pgid = child
                .id()
                .expect("Child::id is Some between spawn and wait") as i32;
            let _kill_group = ProcessGroupKillOnDrop::new(pgid);

            // Poll for the pidfile — bounded to prevent a hang if the shell
            // blew up before recording the pid.
            let started = Instant::now();
            let pid_str = loop {
                if let Ok(s) = std::fs::read_to_string(&pidfile) {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        break trimmed.to_string();
                    }
                }
                assert!(
                    started.elapsed() < Duration::from_secs(5),
                    "descendant pidfile never appeared",
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            };
            let pid: i32 = pid_str.parse().expect("descendant pid is a valid i32");
            // Drop `_kill_group`, then `child` here on scope exit.
            pid
        };

        // SIGKILL is delivered asynchronously; give the kernel a moment.
        // Polling is faster + less flaky than a single fixed sleep.
        let started = Instant::now();
        loop {
            // SAFETY: `kill(pid, 0)` does not deliver any signal — it only
            // checks whether `pid` exists. The pid we hold came from a
            // `Child` we spawned, so it can only refer to (a) a process we
            // own, or (b) be already-dead. No collateral damage either way.
            let rc = unsafe { libc::kill(descendant_pid, 0) };
            if rc == -1 {
                let err = std::io::Error::last_os_error();
                assert_eq!(
                    err.raw_os_error(),
                    Some(libc::ESRCH),
                    "kill(pid, 0) failed with unexpected errno: {err}",
                );
                return; // descendant is gone — test passes
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "descendant pid {descendant_pid} still alive after group drop",
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}
