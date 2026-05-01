//! Integration tests for `DockerLifecycle::exec_with_env`.
//!
//! Gated on `STEPYARD_TEST_DOCKER=1` — CI environments without a daemon skip
//! these without failing. Every `Command` and every future carries a
//! `.timeout(Duration::from_secs(N))` (Rule 7b).

use std::collections::HashMap;
use std::time::Duration;

use stepyard_sandbox_orchestrator::{
    DockerLifecycle, ExecOptions, SandboxError, SandboxId, SandboxLifecycle,
};
use tokio::time::timeout;
use uuid::Uuid;

/// Wall-clock ceiling for any `docker exec`/`docker rm -f` call. Generous
/// enough for a cold-cache container on a slow runner without letting a
/// hanging daemon pin the test indefinitely.
const DOCKER_TIMEOUT: Duration = Duration::from_secs(20);

fn docker_enabled() -> bool {
    std::env::var("STEPYARD_TEST_DOCKER")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Build an image-shell container the same way `DockerLifecycle::create` does,
/// but do it directly here so the test can reuse the container name derived
/// from a known session id (AC assertions target exact argv).
async fn create_alpine_container(session_id: Uuid) -> String {
    let name = format!("minion-session-{session_id}");
    // Clean up any stale container from a prior failed test run.
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

#[tokio::test]
async fn exec_with_env_injects_env_var_verbatim() {
    // AC5 (positive control, benign value): `printenv FOO` with FOO=bar
    // yields `bar\n` exactly.
    if !docker_enabled() {
        eprintln!("[skip] STEPYARD_TEST_DOCKER not set to 1");
        return;
    }
    let session_id = Uuid::new_v4();
    let name = create_alpine_container(session_id).await;
    let lifecycle = DockerLifecycle::default();
    let id = SandboxId::from(session_id);
    let mut env = HashMap::new();
    env.insert("FOO".to_string(), "bar".to_string());

    let out = timeout(
        DOCKER_TIMEOUT,
        lifecycle.exec_with_env(
            &id,
            &["printenv".to_string(), "FOO".to_string()],
            &env,
        ),
    )
    .await
    .expect("exec_with_env did not time out")
    .expect("exec_with_env returned Err");

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "bar\n");

    teardown(&name).await;
}

#[tokio::test]
async fn exec_with_env_passes_shell_metacharacters_as_literal_string() {
    // AC5 (positive-control security assertion): env value containing
    // `$(rm -rf /)` must be surfaced to the child process as a literal
    // string; no shell expansion, no filesystem damage.
    if !docker_enabled() {
        eprintln!("[skip] STEPYARD_TEST_DOCKER not set to 1");
        return;
    }
    let session_id = Uuid::new_v4();
    let name = create_alpine_container(session_id).await;
    let lifecycle = DockerLifecycle::default();
    let id = SandboxId::from(session_id);

    // Confirm /etc/passwd is there BEFORE the call (baseline).
    let baseline = timeout(
        DOCKER_TIMEOUT,
        tokio::process::Command::new("docker")
            .args(["exec", &name, "test", "-f", "/etc/passwd"])
            .output(),
    )
    .await
    .expect("baseline test did not time out")
    .expect("baseline test did not error");
    assert!(
        baseline.status.success(),
        "container lost /etc/passwd before the test ran"
    );

    let mut env = HashMap::new();
    env.insert("PAYLOAD".to_string(), "$(rm -rf /)".to_string());

    let out = timeout(
        DOCKER_TIMEOUT,
        lifecycle.exec_with_env(
            &id,
            &["printenv".to_string(), "PAYLOAD".to_string()],
            &env,
        ),
    )
    .await
    .expect("exec_with_env did not time out")
    .expect("exec_with_env returned Err");

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(
        out.stdout, "$(rm -rf /)\n",
        "payload must reach printenv as literal — shell expansion would produce empty string"
    );

    // Post-check: /etc/passwd still present inside the container.
    let after = timeout(
        DOCKER_TIMEOUT,
        tokio::process::Command::new("docker")
            .args(["exec", &name, "test", "-f", "/etc/passwd"])
            .output(),
    )
    .await
    .expect("post-check did not time out")
    .expect("post-check did not error");
    assert!(
        after.status.success(),
        "argv-only boundary breached — /etc/passwd gone"
    );

    teardown(&name).await;
}

#[tokio::test]
async fn exec_with_env_deterministic_ordering_on_repeated_calls() {
    // AC5 (determinism): two invocations with the same env map produce
    // identical behavior — env keys are sorted by implementation so argv
    // construction is repeatable. We can't introspect argv directly, but
    // we can assert the child's view of env vars sort identically.
    if !docker_enabled() {
        eprintln!("[skip] STEPYARD_TEST_DOCKER not set to 1");
        return;
    }
    let session_id = Uuid::new_v4();
    let name = create_alpine_container(session_id).await;
    let lifecycle = DockerLifecycle::default();
    let id = SandboxId::from(session_id);
    let mut env = HashMap::new();
    env.insert("B".to_string(), "two".to_string());
    env.insert("A".to_string(), "one".to_string());
    env.insert("C".to_string(), "three".to_string());

    // `env` prints all env vars; grep restricts to the three we set, and
    // `sort` normalises ordering so we can compare across runs.
    let cmd = vec![
        "sh".to_string(),
        "-c".to_string(),
        "env | grep -E '^(A|B|C)=' | sort".to_string(),
    ];

    let first = timeout(DOCKER_TIMEOUT, lifecycle.exec_with_env(&id, &cmd, &env))
        .await
        .expect("first exec_with_env did not time out")
        .expect("first exec_with_env returned Err");
    let second = timeout(DOCKER_TIMEOUT, lifecycle.exec_with_env(&id, &cmd, &env))
        .await
        .expect("second exec_with_env did not time out")
        .expect("second exec_with_env returned Err");

    assert_eq!(first.stdout, "A=one\nB=two\nC=three\n");
    assert_eq!(first.stdout, second.stdout);

    teardown(&name).await;
}

#[tokio::test]
async fn exec_with_options_idle_timeout_returns_typed_error() {
    if !docker_enabled() {
        eprintln!("[skip] STEPYARD_TEST_DOCKER not set to 1");
        return;
    }

    let session_id = Uuid::new_v4();
    let name = create_alpine_container(session_id).await;
    let lifecycle = DockerLifecycle::default();
    let id = SandboxId::from(session_id);
    let cmd = vec!["sleep".to_string(), "300".to_string()];
    let opts = ExecOptions {
        env: HashMap::new(),
        idle_timeout: Some(Duration::from_secs(2)),
    };

    let err = timeout(
        Duration::from_secs(8),
        lifecycle.exec_with_options(&id, &cmd, &opts),
    )
    .await
    .expect("exec_with_options should return before outer timeout")
    .expect_err("sleep without stdout should idle-time out");
    match err {
        SandboxError::IdleTimeout { idle_ms } => assert_eq!(idle_ms, 2_000),
        other => panic!("expected IdleTimeout, got {other:?}"),
    }

    teardown(&name).await;
}
