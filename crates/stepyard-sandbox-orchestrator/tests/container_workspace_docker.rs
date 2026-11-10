//! Round trip of a project through a session container: copy in, change
//! files in the container, list the changes with git, copy them back.
//!
//! Ignored unless `STEPYARD_TEST_DOCKER=1` and Cargo is passed `--ignored`.
//! Uses `alpine` and installs `git` with `apk` (needs network), so it does
//! not depend on the full sandbox image. The workflow file is unchanged:
//! CI does not pass `--ignored`, so these tests stay listed as ignored.

use std::time::Duration;

use stepyard_sandbox_orchestrator::{
    ChangedPath, CreateOptions, DockerLifecycle, DockerLifecycleConfig, SandboxLifecycle,
};
use tokio::time::timeout;
use uuid::Uuid;

const STEP_TIMEOUT: Duration = Duration::from_secs(120);

/// Removes the container even when an assertion panics.
struct RemoveOnDrop(String);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.0])
            .output();
    }
}

fn docker_enabled() -> bool {
    std::env::var("STEPYARD_TEST_DOCKER")
        .map(|v| v == "1")
        .unwrap_or(false)
}

#[tokio::test]
#[ignore = "needs a local Docker daemon; STEPYARD_TEST_DOCKER=1 cargo test -- --ignored"]
async fn project_round_trips_through_the_session_container() {
    assert!(
        docker_enabled(),
        "set STEPYARD_TEST_DOCKER=1 to run this ignored test"
    );
    let host = tempfile::tempdir().unwrap();
    std::fs::write(host.path().join("keep.txt"), "same\n").unwrap();
    std::fs::write(host.path().join("edit.txt"), "before\n").unwrap();
    std::fs::write(host.path().join("gone.txt"), "x\n").unwrap();
    std::fs::create_dir(host.path().join("target")).unwrap();
    std::fs::write(host.path().join("target/big.bin"), "build output\n").unwrap();

    let lifecycle = DockerLifecycle::new(DockerLifecycleConfig {
        image: "alpine:latest".into(),
        shell: "sh".into(),
    });
    let session = Uuid::new_v4();
    let opts = CreateOptions {
        workdir: Some("/workspace".into()),
        ..CreateOptions::default()
    };
    timeout(STEP_TIMEOUT, lifecycle.create_with_options(session, &opts))
        .await
        .expect("create timed out")
        .expect("create");
    let handle = lifecycle
        .container_handle(session)
        .expect("docker has a handle");
    let _cleanup = RemoveOnDrop(handle.name.clone());

    let result = timeout(STEP_TIMEOUT, async {
        handle
            .copy_in(host.path(), "/workspace", &["target".to_string()])
            .await
            .expect("copy_in");
        let out = handle
            .sh("set -e; apk add --no-cache -q git >/dev/null; \
                 cd /workspace; test ! -e target; \
                 git init -q && git add -A && \
                 git -c user.name=t -c user.email=t@t commit -qm base; \
                 echo after > edit.txt; rm gone.txt; mkdir -p new && echo n > new/file.txt")
            .await
            .expect("sh");
        assert!(
            out.status.success(),
            "container setup failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut changes = handle
            .changed_paths("/workspace")
            .await
            .expect("changed_paths");
        changes.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        assert_eq!(
            changes,
            vec![
                ChangedPath::Deleted("gone.txt".into()),
                ChangedPath::Modified("edit.txt".into()),
                ChangedPath::Modified("new/file.txt".into()),
            ]
        );
        handle
            .copy_out("/workspace", &changes, host.path())
            .await
            .expect("copy_out")
    })
    .await;
    let report = result.expect("round trip timed out");

    assert_eq!(report.deleted, vec!["gone.txt"]);
    assert_eq!(report.copied.len(), 2);
    let read = |p: &str| std::fs::read_to_string(host.path().join(p)).unwrap();
    assert_eq!(read("edit.txt"), "after\n");
    assert_eq!(read("new/file.txt"), "n\n");
    assert_eq!(read("keep.txt"), "same\n");
    assert!(!host.path().join("gone.txt").exists());
}

#[tokio::test]
#[ignore = "needs a local Docker daemon; STEPYARD_TEST_DOCKER=1 cargo test -- --ignored"]
async fn copy_out_refuses_a_symlink_that_points_outside_the_workspace() {
    assert!(
        docker_enabled(),
        "set STEPYARD_TEST_DOCKER=1 to run this ignored test"
    );
    let host = tempfile::tempdir().unwrap();
    std::fs::write(host.path().join("keep.txt"), "same\n").unwrap();

    let lifecycle = DockerLifecycle::new(DockerLifecycleConfig {
        image: "alpine:latest".into(),
        shell: "sh".into(),
    });
    let session = Uuid::new_v4();
    let opts = CreateOptions {
        workdir: Some("/workspace".into()),
        ..CreateOptions::default()
    };
    timeout(STEP_TIMEOUT, lifecycle.create_with_options(session, &opts))
        .await
        .expect("create timed out")
        .expect("create");
    let handle = lifecycle
        .container_handle(session)
        .expect("docker has a handle");
    let _cleanup = RemoveOnDrop(handle.name.clone());

    let result = timeout(STEP_TIMEOUT, async {
        handle
            .copy_in(host.path(), "/workspace", &[])
            .await
            .expect("copy_in");
        let out = handle
            .sh("set -e; apk add --no-cache -q git >/dev/null; \
                 cd /workspace; git init -q && git add -A && \
                 git -c user.name=t -c user.email=t@t commit -qm base; \
                 ln -s /etc/passwd leak")
            .await
            .expect("sh");
        assert!(
            out.status.success(),
            "container setup failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        handle
            .copy_out(
                "/workspace",
                &[ChangedPath::Modified("leak".into())],
                host.path(),
            )
            .await
    })
    .await
    .expect("symlink copy timed out");

    let err = result.expect_err("symlink must not be copied to the host");
    assert!(
        err.to_string().contains("symlink") || err.to_string().contains("refusing"),
        "{err}"
    );
    let leak = host.path().join("leak");
    if leak.exists() {
        assert!(
            !std::fs::symlink_metadata(&leak)
                .unwrap()
                .file_type()
                .is_symlink(),
            "docker cp left a symlink on the host"
        );
    }
}
