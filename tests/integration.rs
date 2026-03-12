use std::collections::HashMap;

use minion_engine::{engine::Engine, workflow::parser, workflow::validator};

/// Helper to run a YAML workflow string to completion with quiet output
async fn run_workflow(yaml: &str, target: &str) -> anyhow::Result<minion_engine::steps::StepOutput> {
    let wf = parser::parse_str(yaml)?;
    let errors = validator::validate(&wf);
    assert!(errors.is_empty(), "Workflow validation errors: {:?}", errors);
    let mut engine = Engine::new(wf, target.to_string(), HashMap::new(), false, true);
    engine.run().await
}

#[tokio::test]
async fn simple_test_workflow_runs_without_errors() {
    let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/workflows/simple-test.yaml"));
    let wf = parser::parse_file(path).expect("simple-test.yaml should parse");
    let errors = validator::validate(&wf);
    assert!(errors.is_empty(), "Validation errors: {:?}", errors);

    let mut engine = Engine::new(wf, "test_target".to_string(), HashMap::new(), false, true);
    let result = engine.run().await.expect("simple-test workflow should succeed");
    // Last step is ls -la — output should be non-empty
    assert!(!result.text().is_empty());
}

#[tokio::test]
async fn workflow_with_two_cmd_steps_returns_last_output() {
    let yaml = r#"
name: test
steps:
  - name: first
    type: cmd
    run: "echo alpha"
  - name: second
    type: cmd
    run: "echo beta"
"#;
    let result = run_workflow(yaml, "").await.unwrap();
    assert_eq!(result.text().trim(), "beta");
}

#[tokio::test]
async fn workflow_target_is_accessible_in_templates() {
    let yaml = r#"
name: test
steps:
  - name: greet
    type: cmd
    run: "echo hello_{{ target }}"
"#;
    let result = run_workflow(yaml, "world").await.unwrap();
    assert_eq!(result.text().trim(), "hello_world");
}

#[tokio::test]
async fn workflow_step_output_flows_to_next_step() {
    let yaml = r#"
name: test
steps:
  - name: produce
    type: cmd
    run: "echo 'the_value'"
  - name: consume
    type: cmd
    run: "echo 'got={{ steps.produce.stdout }}'"
"#;
    let result = run_workflow(yaml, "").await.unwrap();
    assert!(result.text().contains("the_value"));
}

#[tokio::test]
async fn workflow_with_gate_passes_when_condition_true() {
    let yaml = r#"
name: test
steps:
  - name: cmd
    type: cmd
    run: "echo ok"
  - name: check
    type: gate
    condition: "{{ steps.cmd.exit_code == 0 }}"
    on_fail: fail
    message: "Command succeeded"
  - name: done
    type: cmd
    run: "echo done"
"#;
    let result = run_workflow(yaml, "").await.unwrap();
    assert_eq!(result.text().trim(), "done");
}

#[tokio::test]
async fn workflow_with_repeat_runs_scoped_steps() {
    let yaml = r#"
name: test
scopes:
  inner:
    steps:
      - name: step
        type: cmd
        run: "echo iteration"
steps:
  - name: loop_step
    type: repeat
    scope: inner
    max_iterations: 2
"#;
    let result = run_workflow(yaml, "").await.unwrap();
    // Scope output final_value contains the last iteration's output
    assert!(result.text().contains("iteration"));
}

#[tokio::test]
async fn fix_issue_yaml_is_valid() {
    let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/workflows/fix-issue.yaml"));
    let wf = parser::parse_file(path).expect("fix-issue.yaml should parse");
    let errors = validator::validate(&wf);
    assert!(errors.is_empty(), "fix-issue.yaml validation errors: {:?}", errors);
}
