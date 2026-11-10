//! P1-10 (analysis 2026-11-09, SEG-RUST-03 / SEG-RUST-01 / BUG-MOTOR-04) —
//! sandbox policy checks that run before any container runtime is contacted.
//!
//! Every test points DOCKER_HOST at a socket that does not exist, so a check
//! that wrongly lets a run through fails on "runtime unavailable" instead of
//! reaching a real daemon (and, for the opt-in case, mounting `/`).

use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

const V1_REFUSAL: &str = "bind-mounts your working directory";
const VOLUME_REFUSAL: &str = "outside the project directory";

fn stepyard() -> Command {
    let mut cmd = Command::cargo_bin("stepyard").unwrap();
    cmd.timeout(Duration::from_secs(60))
        .env(
            "DOCKER_HOST",
            "unix:///nonexistent/stepyard-test-docker.sock",
        )
        .env_remove("DOCKER_CONTEXT")
        .env_remove("STEPYARD_SANDBOX")
        .env_remove("STEPYARD_ALLOW_WORKFLOW_MOUNTS")
        .env_remove("DATABASE_URL")
        .env_remove("STEPYARD_HARNESS_DATABASE_URL")
        .env_remove("STEPYARD_SESSION_DATABASE_URL")
        .env("STEPYARD_NO_FILE_LOGS", "1");
    cmd
}

#[test]
fn v1_with_a_container_sandbox_on_the_host_directory_is_refused() {
    stepyard()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--engine",
            "v1",
            "--sandbox-runtime",
            "docker",
        ])
        .assert()
        .failure()
        .stderr(contains(V1_REFUSAL));
}

#[test]
fn v1_repo_mode_and_v1_without_sandbox_are_not_refused_by_the_policy() {
    stepyard()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--engine",
            "v1",
            "--repo",
            "acme/app",
            "--sandbox-runtime",
            "docker",
        ])
        .assert()
        .stderr(contains(V1_REFUSAL).not());
    stepyard()
        .args([
            "execute",
            "workflows/hello-world-cmd.yaml",
            "--engine",
            "v1",
            "--no-sandbox",
        ])
        .assert()
        .stderr(contains(V1_REFUSAL).not());
}

fn workflow_with_volumes(volumes: &[&str]) -> tempfile::NamedTempFile {
    let list = volumes
        .iter()
        .map(|v| format!("        - \"{v}\"\n"))
        .collect::<String>();
    let yaml = format!(
        "name: mounts\nconfig:\n  global:\n    sandbox:\n      volumes:\n{list}steps:\n  - name: a\n    type: cmd\n    run: \"true\"\n"
    );
    let file = tempfile::Builder::new().suffix(".yaml").tempfile().unwrap();
    std::fs::write(file.path(), yaml).unwrap();
    file
}

#[test]
fn workflow_volumes_outside_the_project_are_refused_on_v2() {
    for spec in [
        "/:/host",
        "/var/run/docker.sock:/var/run/docker.sock",
        "~/.ssh:/x:ro",
    ] {
        let wf = workflow_with_volumes(&[spec]);
        stepyard()
            .args([
                "execute",
                wf.path().to_str().unwrap(),
                "--sandbox-runtime",
                "docker",
            ])
            .assert()
            .failure()
            .stderr(contains(VOLUME_REFUSAL))
            .stderr(contains(spec));
    }
}

#[test]
fn project_volumes_pass_and_the_user_opt_in_lifts_the_refusal() {
    let inside = workflow_with_volumes(&["./workflows:/wf:ro", "cache:/cache"]);
    stepyard()
        .args([
            "execute",
            inside.path().to_str().unwrap(),
            "--sandbox-runtime",
            "docker",
        ])
        .assert()
        .stderr(contains(VOLUME_REFUSAL).not());

    let outside = workflow_with_volumes(&["/:/host"]);
    stepyard()
        .env("STEPYARD_ALLOW_WORKFLOW_MOUNTS", "1")
        .args([
            "execute",
            outside.path().to_str().unwrap(),
            "--sandbox-runtime",
            "docker",
        ])
        .assert()
        .failure() // the fake DOCKER_HOST stops it right after the policy
        .stderr(contains(VOLUME_REFUSAL).not());
}
