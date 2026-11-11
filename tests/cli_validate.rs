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

/// v2 renders only `steps`, `target`, `vars`, `args` and `scope`: a step
/// using `{{ stack.* }}` would fail mid-run, after earlier steps ran.
#[test]
fn validate_v2_refuses_v1_only_template_namespaces() {
    let wf = workflow(
        "# {{ prompts.x }} in a comment does not count\nname: stack\nsteps:\n  - name: lint\n    type: cmd\n    run: \"{{ stack.tools.lint }}\"\n",
    );
    stepyard()
        .args(["validate", wf.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("{{ stack.* }}"))
        .stderr(contains("--engine v2"));
    stepyard()
        .args(["validate", "--engine", "v1", wf.path().to_str().unwrap()])
        .assert()
        .success();
}

/// The YAML-SPEC "Full example" uses v1-only `{{ steps.X.output }}`; the
/// spec says so, and `validate` must agree on both engines.
#[test]
fn the_yaml_spec_full_example_is_v1_only() {
    let spec = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/YAML-SPEC.md"),
    )
    .unwrap();
    let section = spec
        .split("## Full example with all top-level fields")
        .nth(1)
        .expect("full example section");
    let yaml = section
        .split("```yaml\n")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("yaml block");
    let wf = workflow(yaml);
    stepyard()
        .args(["validate", wf.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(contains("{{ steps.<name>.output }}"));
    stepyard()
        .args(["validate", "--engine", "v1", wf.path().to_str().unwrap()])
        .assert()
        .success();
}

/// Each complete workflow in docs/EXAMPLES.md validates for the engine its
/// heading declares: the default (v2), or v1 when it says "v1 only", and then
/// the default engine must refuse it. Keeps the examples and `validate`
/// telling the same story.
#[test]
fn every_complete_example_validates_for_its_engine() {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/EXAMPLES.md"),
    )
    .unwrap();
    let (mut v2, mut v1) = (0, 0);
    for section in doc.split("\n### ").skip(1) {
        let heading = section.lines().next().unwrap_or_default();
        let Some(yaml) = section
            .split("```yaml\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
        else {
            continue;
        };
        if !(yaml.starts_with("name:") && yaml.contains("\nsteps:")) {
            continue;
        }
        let wf = workflow(yaml);
        let file = wf.path().to_str().unwrap();
        if heading.contains("v1 only") {
            stepyard()
                .args(["validate", "--engine", "v1", file])
                .assert()
                .success();
            stepyard().args(["validate", file]).assert().failure();
            v1 += 1;
        } else {
            stepyard().args(["validate", file]).assert().success();
            v2 += 1;
        }
    }
    assert!(v2 >= 3 && v1 >= 2, "v2 {v2}, v1 {v1}");
}

/// Each shipped workflow validates for the engine it declares: the default
/// (v2), or v1 when its header says `Requires --engine v1`, and then the
/// default engine must refuse it.
#[test]
fn every_shipped_workflow_validates_for_its_engine() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("workflows");
    let (mut v2, mut v1) = (0, 0);
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let file = path.to_str().unwrap();
        if std::fs::read_to_string(&path)
            .unwrap()
            .contains("Requires --engine v1")
        {
            stepyard()
                .args(["validate", "--engine", "v1", file])
                .assert()
                .success();
            stepyard().args(["validate", file]).assert().failure();
            v1 += 1;
        } else {
            stepyard().args(["validate", file]).assert().success();
            v2 += 1;
        }
    }
    assert!(v2 >= 10 && v1 >= 2, "v2 {v2}, v1 {v1}");
}
