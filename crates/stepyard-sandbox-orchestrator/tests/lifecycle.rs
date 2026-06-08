//! Lifecycle tests.
//!
//! Mock-based tests always run. Docker-backed tests are gated by the
//! `STEPYARD_TEST_DOCKER=1` env var so CI without a daemon does not fail.

use std::sync::Arc;

use stepyard_sandbox_orchestrator::{
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

#[tokio::test]
async fn mock_destroy_by_session_default_records_destroy_with_session_derived_id() {
    // MockLifecycle does NOT override destroy_by_session — it relies on the
    // SandboxLifecycle trait default at lib.rs which converts session_id into
    // a SandboxId via `From<Uuid>` and delegates to `destroy(&id)`. If a
    // future refactor changes that default to use SandboxId::new() (random)
    // or to skip the destroy call entirely, the harness signal/timeout paths
    // would silently leak containers in production. This test pins the
    // default's contract so a mutation gets caught here, not in prod.
    let mock = MockLifecycle::new();
    let session_id = Uuid::new_v4();

    mock.destroy_by_session(session_id)
        .await
        .expect("destroy_by_session");

    let calls = mock.calls().await;
    let recorded = calls
        .iter()
        .find_map(|c| match c {
            MockCall::Destroy { id } => Some(*id),
            _ => None,
        })
        .expect("Destroy variant recorded via the default impl chain");
    assert_eq!(
        *recorded.as_uuid(),
        session_id,
        "destroy_by_session default must derive SandboxId from session_id via From<Uuid>"
    );
}

// ── Docker tests (require daemon, gated by env var) ───────────────────

fn docker_enabled() -> bool {
    std::env::var("STEPYARD_TEST_DOCKER")
        .map(|v| v == "1")
        .unwrap_or(false)
}

#[tokio::test]
async fn docker_create_exec_destroy_roundtrip() {
    if !docker_enabled() {
        eprintln!("[skip] STEPYARD_TEST_DOCKER not set to 1");
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
        eprintln!("[skip] STEPYARD_TEST_DOCKER not set to 1");
        return;
    }
    let lifecycle = DockerLifecycle::default();
    let session_id = Uuid::new_v4();
    lifecycle
        .destroy_by_session(session_id)
        .await
        .expect("destroy non-existent is Ok");
}

#[tokio::test]
async fn docker_destroy_by_session_reaches_override_through_trait_object() {
    // The harness calls destroy_by_session through Arc<dyn SandboxLifecycle>,
    // not on a concrete DockerLifecycle. Guards against a future refactor
    // that drops `destroy_by_session` from the trait impl and silently
    // falls back to the default (which delegates to the no-op `destroy`) —
    // that would leave real containers running.
    if !docker_enabled() {
        eprintln!("[skip] STEPYARD_TEST_DOCKER not set to 1");
        return;
    }
    let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(DockerLifecycle::default());
    let session_id = Uuid::new_v4();
    let sandbox = lifecycle.create(session_id).await.expect("create");
    let _ = sandbox.exec("echo hi").await.expect("exec");

    lifecycle
        .destroy_by_session(session_id)
        .await
        .expect("destroy_by_session via trait object");

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
    assert!(
        stdout.is_empty(),
        "container still listed after trait-dispatched destroy_by_session: {stdout}"
    );
}
