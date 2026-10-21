//! Story 2.2 — SIGINT/SIGTERM handler with graceful shutdown deadline.
//!
//! `main()` owns a `broadcast::Sender<()>` (Story 2.1 — D4). This module
//! installs Unix signal handlers (D2 — Unix-only, **not** the cross-platform
//! `tokio::signal::ctrl_c()`), fires the broadcast on first signal, waits
//! up to `grace_s` seconds for in-flight engines to settle, and returns the
//! canonical POSIX exit code (130 for SIGINT, 143 for SIGTERM).
//!
//! NFR10 — handler safety: the body touches neither the PostgreSQL pool nor
//! `docker rm -f`. Only side effects are `shutdown_tx.send(())` (best-effort)
//! and a wall-clock deadline poll of `receiver_count()`. Container destroy is
//! the Engine's responsibility via Story 2.3.

use std::process::ExitCode;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::broadcast;

/// Which signal fired — controls the exit code picked after the grace deadline.
#[derive(Debug, Clone, Copy)]
enum FiredSignal {
    Int,
    Term,
}

impl FiredSignal {
    /// POSIX exit-code convention: 128 + signum. SIGINT=2 → 130, SIGTERM=15 → 143.
    fn exit_code(self) -> ExitCode {
        match self {
            FiredSignal::Int => ExitCode::from(130),
            FiredSignal::Term => ExitCode::from(143),
        }
    }

    /// Lowercase name — matches the string policy Story 2.3 uses for
    /// `Event::SignalReceived { signal }` (`"sigint"` / `"sigterm"`).
    fn name(self) -> &'static str {
        match self {
            FiredSignal::Int => "sigint",
            FiredSignal::Term => "sigterm",
        }
    }
}

/// Install Unix signal handlers and return the canonical exit code once
/// either a signal fires and the grace deadline elapses, OR every engine
/// receiver has dropped (whichever comes first).
///
/// This future never resolves on its own — `main()` races it against
/// `Cli::run(..)` via `tokio::select!`, so a clean workflow completion wins
/// and this future is simply dropped.
///
/// `shutdown_signal` (Story 2.3 — Option B) carries the lowercase signal
/// name (`"sigint"` / `"sigterm"`) to every engine's
/// `HarnessConfig::shutdown_signal`. It is populated **before** the
/// broadcast fires, so by the time a `step()` select arm resolves the name
/// is already readable (the `OnceLock` write happens-before the broadcast
/// send, and `broadcast::Receiver::recv` establishes the release/acquire
/// pair that makes that write visible cross-thread).
pub async fn install_handlers(
    shutdown_tx: Arc<broadcast::Sender<()>>,
    shutdown_signal: Arc<OnceLock<String>>,
    grace_s: u64,
) -> ExitCode {
    let mut sigint = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "failed to install SIGINT handler");
            std::future::pending::<()>().await;
            unreachable!();
        }
    };
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "failed to install SIGTERM handler");
            std::future::pending::<()>().await;
            unreachable!();
        }
    };

    let fired = tokio::select! {
        _ = sigint.recv()  => FiredSignal::Int,
        _ = sigterm.recv() => FiredSignal::Term,
    };

    tracing::info!(signal = fired.name(), "shutdown signal received");

    // Populate the shared signal-name slot BEFORE firing the broadcast so
    // every engine select arm reads a populated `OnceLock`. `.set` returns
    // `Err` only on a second set — harmless here since we fire once per
    // process lifetime.
    let _ = shutdown_signal.set(fired.name().to_string());

    // Best-effort broadcast — no receivers is fine (engines may have already
    // exited). `SendError` is intentionally discarded per D2/NFR10.
    let _ = shutdown_tx.send(());

    // Poll for receivers to drain within the grace deadline. `receiver_count`
    // drops as each Engine's `shutdown_rx` is dropped during finalise-cancel
    // (Story 2.3 wires the select! arm that triggers the drop).
    let deadline = Instant::now() + Duration::from_secs(grace_s);
    while shutdown_tx.receiver_count() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    fired.exit_code()
}
