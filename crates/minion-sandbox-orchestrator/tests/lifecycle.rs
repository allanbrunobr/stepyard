//! Lifecycle tests.
//!
//! Mock-based tests always run. Docker-backed tests are gated by the
//! `MINION_TEST_DOCKER=1` env var so CI without a daemon does not fail.

use minion_sandbox_orchestrator::{
    mock::mark_destroyed, DockerLifecycle, ExecOutput, MockCall, MockLifecycle, SandboxError,
    SandboxLifecycle,
};
use uuid::Uuid;

// ── Mock tests (no Docker) ──────────────────────────────────────────────

#[tokio::test]
async fn mock_records_create_then_exec_then_destroy() {
    let mock = MockLifecycle::new();
    let sid = Uuid::new_v4();
    let sandbox = mock.create(sid).await.expect("create");
    let out = sandbox.exec("echo hi").await.expect("exec");
    assert_eq!(out.exit_code, 0);
    mock.destroy(&sandbox.id().await).await.expect("destroy");

    let calls = mock.calls().await;
    assert!(matches!(calls[0], MockCall::Create { session_id } if session_id == sid));
    assert!(matches!(&calls[1], MockCall::Exec { cmd, .. } if cmd == "echo hi"));
    assert!(matches!(calls[2], MockCall::Destroy { .. }));
}

#[tokio::test]
async fn mock_exec_returns_preset_override() {
    let mock = MockLifecycle::new();
    mock.set_exec_response(
        "uname",
        ExecOutput {
            stdout: "Linux\n".into(),
            stderr: String::new(),
            exit_code: 0,
        },
    )
    .await;
    let sandbox = mock.create(Uuid::new_v4()).await.expect("create");
    let out = sandbox.exec("uname").await.expect("exec");
    assert_eq!(out.stdout, "Linux\n");
}

#[tokio::test]
async fn exec_on_destroyed_sandbox_returns_destroyed_error_without_panic() {
    // AC: "um Sandbox destruido pelo orchestrator no meio de um step ...
    // retorna Err(SandboxError::Destroyed) sem panic"
    let mock = MockLifecycle::new();
    let sandbox = mock.create(Uuid::new_v4()).await.expect("create");

    // Simulate the orchestrator tearing the sandbox down mid-step.
    mark_destroyed(&sandbox).await;

    let err = sandbox.exec("echo hi").await.expect_err("should error");
    assert!(
        matches!(err, SandboxError::Destroyed(_)),
        "expected Destroyed, got {err:?}"
    );
    assert!(sandbox.is_destroyed().await);
}

#[tokio::test]
async fn reuse_or_create_is_recorded_separately_from_create() {
    let mock = MockLifecycle::new();
    let sid = Uuid::new_v4();
    let _sandbox = mock.reuse_or_create(sid).await.expect("reuse_or_create");
    let calls = mock.calls().await;
    // reuse_or_create calls into create under the hood on the mock, so
    // both variants appear. Order: ReuseOrCreate, Create.
    assert!(matches!(calls[0], MockCall::ReuseOrCreate { session_id } if session_id == sid));
    assert!(matches!(calls[1], MockCall::Create { session_id } if session_id == sid));
}

// ── Docker tests (require daemon, gated by env var) ───────────────────

fn docker_enabled() -> bool {
    std::env::var("MINION_TEST_DOCKER").map(|v| v == "1").unwrap_or(false)
}

#[tokio::test]
async fn docker_create_exec_destroy_roundtrip() {
    if !docker_enabled() {
        eprintln!("[skip] MINION_TEST_DOCKER not set to 1");
        return;
    }
    let lifecycle = DockerLifecycle::default();
    let session_id = Uuid::new_v4();

    let sandbox = lifecycle.create(session_id).await.expect("create");

    let out = sandbox.exec("echo hi").await.expect("exec");
    assert_eq!(out.stdout, "hi\n", "stdout should be 'hi\\n'");
    assert!(out.is_success());

    lifecycle
        .destroy_by_session(session_id)
        .await
        .expect("destroy");

    // Container should no longer be listed.
    let ps = tokio::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "-q",
            "--filter",
            &format!("name=minion-session-{session_id}"),
        ])
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&ps.stdout).trim().to_string();
    assert!(stdout.is_empty(), "container still listed: {stdout}");
}

#[tokio::test]
async fn docker_destroy_is_idempotent() {
    if !docker_enabled() {
        eprintln!("[skip] MINION_TEST_DOCKER not set to 1");
        return;
    }
    let lifecycle = DockerLifecycle::default();
    let session_id = Uuid::new_v4();
    lifecycle
        .destroy_by_session(session_id)
        .await
        .expect("destroy non-existent is Ok");
}
