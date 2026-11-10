//! BUG-CLI-13 (analysis 2026-11-09): `stepyard validate` applied only the v1
//! rules and approved workflows the default engine (v2) refuses at run time.
//! No database or Docker needed.

use std::time::Duration;

use assert_cmd::Command;
use predicates::str::contains;

fn stepyard() -> Command {
    let mut cmd = Command::cargo_bin("stepyard").unwrap();
    cmd.timeout(Duration::from_secs(60));
    cmd
}

fn workflow(yaml: &str) -> tempfile::NamedTempFile {
    let file = tempfile::Builder::new().suffix(".yaml").tempfile().unwrap();
    std::fs::write(file.path(), yaml).unwrap();
    file
}

/// Valid for v1 (its validator ignores step config) but refused by the v2
/// adapter: a bare-number timeout has no unit.
const V1_ONLY: &str = "name: v1-only\nsteps:\n  - name: c\n    type: cmd\n    run: \"true\"\n    config:\n      timeout: 30\n";

#[test]
fn validate_defaults_to_v2_and_reports_what_v2_refuses() {
    let wf = workflow(V1_ONLY);
    stepyard()
        .args(["validate", wf.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("--engine v2"))
        .stderr(contains("timeout"));
}

#[test]
fn validate_engine_v1_keeps_the_v1_rules_only() {
    let wf = workflow(V1_ONLY);
    stepyard()
        .args(["validate", "--engine", "v1", wf.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("valid for --engine v1"));
}

#[test]
fn every_shipped_workflow_validates_for_the_default_engine() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("workflows");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            stepyard()
                .args(["validate", path.to_str().unwrap()])
                .assert()
                .success();
            checked += 1;
        }
    }
    assert!(checked >= 12, "checked {checked}");
}
