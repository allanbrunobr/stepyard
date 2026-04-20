//! Story 3.5 — negative-control security tests for the argv-not-shell
//! invariant (D7 / NFR-argv).
//!
//! This file is the living CI enforcement of a single rule: user env
//! values reach the container as argv elements, never as shell code.
//! If any future refactor reintroduces a `format!("KEY={value}", …)` or
//! a joined `sh -c` string at the minion layer, the positive-control
//! below fails.
//!
//! # Running
//!
//! Integration-tier: requires a live Docker daemon. Gated behind
//! `MINION_TEST_DOCKER=1` (matches the convention established in
//! `crates/minion-sandbox-orchestrator/tests/exec_with_env_docker.rs`).
//! Tests early-return without failure when the flag is not set, so CI
//! without Docker skips them gracefully.
//!
//! # Layer choice
//!
//! Tests drive [`DockerLifecycle::exec_with_env`] directly rather than
//! the full `Engine::step` pipeline. The argv-not-shell invariant lives
//! at the Lifecycle layer (Story 3.2); the upstream harness plumbing
//! (Stories 3.3–3.4) is covered by its own tests. Routing through
//! Engine would add a Session/Postgres dependency without strengthening
//! the negative-control signal.
//!
//! # Rules
//!
//! Rule 7a: no `tokio::time::sleep(…)`. Rule 7b: every `Command` and
//! every `exec_with_env` future is wrapped in `timeout(DOCKER_TIMEOUT, …)`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use stepyard_sandbox_orchestrator::{DockerLifecycle, SandboxId, SandboxLifecycle};
use tokio::time::timeout;
use uuid::Uuid;

/// Wall-clock ceiling for any `docker` subprocess or `exec_with_env`
/// future. Generous enough for a cold-image pull on a slow runner
/// without letting a hanging daemon pin the test indefinitely.
const DOCKER_TIMEOUT: Duration = Duration::from_secs(20);

fn docker_enabled() -> bool {
    std::env::var("MINION_TEST_DOCKER")
        .map(|v| v == "1")
        .unwrap_or(false)
}

async fn create_alpine_container(session_id: Uuid) -> String {
    let name = format!("minion-session-{session_id}");
    let _ = timeout(
        DOCKER_TIMEOUT,
        tokio::process::Command::new("docker")
            .args(["rm", "-f", &name])
            .output(),
    )
    .await;

    let out = timeout(
        DOCKER_TIMEOUT,
        tokio::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &name,
                "alpine:latest",
                "sh",
                "-c",
                "trap : TERM INT; sleep infinity & wait",
            ])
            .output(),
    )
    .await
    .expect("docker run did not time out")
    .expect("docker run did not error");
    assert!(
        out.status.success(),
        "docker run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    name
}

async fn teardown(name: &str) {
    let _ = timeout(
        DOCKER_TIMEOUT,
        tokio::process::Command::new("docker")
            .args(["rm", "-f", name])
            .output(),
    )
    .await;
}

/// Positive-control (minion's argv-only guarantee).
///
/// Given an env pair whose value contains `$(touch <host-marker>)`, the
/// `printenv` invocation must (a) emit the payload verbatim, and (b)
/// leave the host filesystem untouched. A joined `sh -c "KEY=$value …"`
/// at any minion call site would expand `$(…)` on the HOST before docker
/// ever saw the value — the host marker would then exist, failing the
/// second assertion.
#[tokio::test]
async fn positive_control_host_filesystem_untouched() {
    if !docker_enabled() {
        eprintln!("[skip] MINION_TEST_DOCKER not set to 1");
        return;
    }

    // Per-test unique marker — a single `cargo test` invocation reuses
    // the same PID across all its tests, so `process::id()` would
    // collide. A fresh Uuid per call is collision-safe.
    let marker = format!("/tmp/minion-pwned-{}", Uuid::new_v4());
    let _ = std::fs::remove_file(&marker);
    assert!(
        !PathBuf::from(&marker).exists(),
        "pre-condition: marker must not exist before the test runs"
    );

    let session_id = Uuid::new_v4();
    let container = create_alpine_container(session_id).await;
    let lifecycle = DockerLifecycle::default();
    let id = SandboxId::from(session_id);

    let payload = format!("$(touch {marker})");
    let mut env = HashMap::new();
    env.insert("MSG".to_string(), payload.clone());

    let out = timeout(
        DOCKER_TIMEOUT,
        lifecycle.exec_with_env(
            &id,
            &["printenv".to_string(), "MSG".to_string()],
            &env,
        ),
    )
    .await
    .expect("exec_with_env did not time out")
    .expect("exec_with_env returned Err");

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(
        out.stdout,
        format!("{payload}\n"),
        "payload must reach printenv verbatim — any shell expansion would strip `$(…)`"
    );
    assert!(
        !PathBuf::from(&marker).exists(),
        "argv-only boundary breached — host filesystem was modified by env value interpolation"
    );

    teardown(&container).await;
}

/// Negative-control (escape hatch IS user-owned).
///
/// When the workflow author EXPLICITLY picks `sh -c "echo $MSG"` as the
/// command, the shell inside the container does expand `$MSG`. Minion
/// does not paternalistically escape values — the expansion boundary is
/// clear and user-owned.
#[tokio::test]
async fn negative_control_user_owned_sh_c_expansion() {
    // Escape hatch behavior — user chose sh -c, user owns expansion safety
    if !docker_enabled() {
        eprintln!("[skip] MINION_TEST_DOCKER not set to 1");
        return;
    }

    let session_id = Uuid::new_v4();
    let container = create_alpine_container(session_id).await;
    let lifecycle = DockerLifecycle::default();
    let id = SandboxId::from(session_id);

    let mut env = HashMap::new();
    env.insert("MSG".to_string(), "pwned".to_string());

    let out = timeout(
        DOCKER_TIMEOUT,
        lifecycle.exec_with_env(
            &id,
            &[
                "sh".to_string(),
                "-c".to_string(),
                "echo $MSG".to_string(),
            ],
            &env,
        ),
    )
    .await
    .expect("exec_with_env did not time out")
    .expect("exec_with_env returned Err");

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "pwned\n");

    teardown(&container).await;
}

/// Epic 5 placeholder — CLI template substitution (`{{KEY}}`).
///
/// When Epic 5 Story 5.x lands `minion run --var KEY=VALUE`, replace the
/// body with the test described in Story 3.5 AC4:
///
/// * `minion run --var MSG='$(rm -rf /)'` against a workflow with
///   `command: ["echo", "{{MSG}}"]`
/// * assert stdout is literally `$(rm -rf /)\n`
/// * assert host filesystem is untouched
///
/// Kept `#[ignore]` (not `panic!`) so `cargo test --ignored` passes
/// trivially while the feature is unshipped — the label is the hook
/// implementers grep for.
#[tokio::test]
#[ignore = "Epic 5 — CLI template substitution not yet implemented"]
async fn epic_5_cli_var_substitution_positive_control_placeholder() {
    // Intentionally empty. See doc-comment above.
}
