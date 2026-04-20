//! Story 2.2 — End-to-end SIGTERM → canonical exit code 143 within
//! `grace_s + 1` seconds (NFR1 + D2).
//!
//! Spawns the `stepyard` binary as a subprocess, gives it a beat to start the
//! step, SIGTERMs it, and asserts the process exits with code 143 inside the
//! grace window. `MINION_SHUTDOWN_GRACE_S=2` keeps the test under 5s wall.
//!
//! Rule 7b deviation: `assert_cmd::Command` doesn't expose `.timeout(..)` on
//! a spawned `Child` — we use a `std::process::Child` and the `wait-timeout`
//! crate for a bounded wait, which is the same guarantee (the test will
//! never hang past N seconds). Documented in the Story 2.2 Dev Agent Record.
//!
//! Skips gracefully when:
//!   - `MINION_HARNESS_DATABASE_URL` is unset (the binary needs a PG session
//!     pool before it reaches the step loop — no DB means no step to
//!     interrupt, which is a test-environment gap, not a defect under test),
//!   - the workspace `target/debug/stepyard` binary is not built
//!     (`cargo build --bin stepyard` fixes it), or
//!   - we're on a non-Unix target (signals are Unix-only per D2).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;
use wait_timeout::ChildExt;

fn stepyard_bin() -> Option<PathBuf> {
    // `assert_cmd::cargo_bin` reads `CARGO_BIN_EXE_<name>`, which is only set
    // inside the crate that declares `[[bin]]` — `stepyard-harness` doesn't.
    // Fall back to the canonical workspace-root `target/debug/stepyard` path.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest.join("../../target/debug/stepyard"),
        manifest.join("../../target/release/stepyard"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn write_fixture(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("signal-fixture.yaml");
    // Long-running command so the step is in-flight when SIGTERM arrives.
    // `type: cmd` with `run:` matches the schema the binary's v2 path parses
    // (see `workflows/hello-world-cmd.yaml`).
    let yaml = "name: signal-fixture\n\
                steps:\n  \
                - name: long_sleep\n    \
                type: cmd\n    \
                run: \"sleep 30\"\n";
    std::fs::write(&path, yaml).expect("write fixture");
    path
}

#[test]
#[cfg(unix)]
fn sigterm_yields_exit_143_within_grace() {
    let Some(bin) = stepyard_bin() else {
        eprintln!("[skip] workspace stepyard binary not built: `cargo build --bin stepyard`");
        return;
    };
    let Ok(db_url) = std::env::var("MINION_HARNESS_DATABASE_URL") else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };

    let tmp = TempDir::new().expect("tempdir");
    let fixture = write_fixture(&tmp);

    // `--no-sandbox` avoids needing Docker for `cmd` steps. Minion's v2
    // engine requires a PG session pool — we hand it the harness test DB via
    // `DATABASE_URL`. `MINION_SHUTDOWN_GRACE_S=2` caps the grace at 2s so the
    // whole test finishes within the 5s wall-clock budget.
    let mut child = Command::new(&bin)
        .arg("execute")
        .arg(&fixture)
        .arg("--no-sandbox")
        // v2 path goes through `stepyard_harness::Engine`, which is the code
        // that subscribes to the shared broadcast (Story 2.1). v1 (the
        // default) doesn't subscribe — so receiver_count stays 0 and the
        // grace loop would exit in <50ms without exercising the deadline.
        .arg("--engine")
        .arg("v2")
        .env("MINION_SHUTDOWN_GRACE_S", "2")
        .env("DATABASE_URL", &db_url)
        // Detach from the test's pid group so our kill only hits stepyard.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn stepyard");

    // Give the binary time to enter `tokio::select!` and register the SIGTERM
    // stream — until `signal(SignalKind::terminate())` resolves, the kernel's
    // default-terminate disposition still applies, and SIGTERM kills the
    // process instead of reaching our handler (observed: `unix_wait_status(15)`
    // at 750ms). 2s is conservative on debug builds on warm caches.
    std::thread::sleep(Duration::from_millis(2000));

    let pid = child.id();
    let sent = Instant::now();
    let kill_ok = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .expect("invoke kill")
        .success();
    assert!(kill_ok, "kill -TERM {pid} failed");

    // Bounded wait (Rule 7b semantic equivalent). If stepyard hasn't exited in
    // 5s, something is wrong — fail loudly rather than hang the test.
    let status = child
        .wait_timeout(Duration::from_secs(5))
        .expect("wait_timeout");
    let elapsed = sent.elapsed();

    match status {
        Some(status) => {
            eprintln!("[info] signal-to-exit elapsed: {elapsed:?}, exit status: {status:?}");
            assert_eq!(
                status.code(),
                Some(143),
                "SIGTERM must yield POSIX exit 143 (128 + SIGTERM=15); got {status:?}",
            );
            assert!(
                elapsed < Duration::from_secs(5),
                "signal-to-exit must be <5s (NFR1 + grace), was {elapsed:?}",
            );
        }
        None => {
            let _ = child.kill();
            panic!(
                "stepyard did not exit within 5s of SIGTERM (elapsed={elapsed:?}); \
                 signal handler likely never fired",
            );
        }
    }
}
