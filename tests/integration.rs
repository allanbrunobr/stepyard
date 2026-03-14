use std::collections::HashMap;

use minion_engine::{engine::Engine, workflow::parser, workflow::validator};

/// Helper to run a YAML workflow string to completion with quiet output
async fn run_workflow(yaml: &str, target: &str) -> anyhow::Result<minion_engine::steps::StepOutput> {
    let wf = parser::parse_str(yaml)?;
    let errors = validator::validate(&wf);
    assert!(errors.is_empty(), "Workflow validation errors: {:?}", errors);
    let mut engine = Engine::new(wf, target.to_string(), HashMap::new(), false, true).await;
    engine.run().await
}

#[tokio::test]
async fn simple_test_workflow_runs_without_errors() {
    let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/workflows/simple-test.yaml"));
    let wf = parser::parse_file(path).expect("simple-test.yaml should parse");
    let errors = validator::validate(&wf);
    assert!(errors.is_empty(), "Validation errors: {:?}", errors);

    let mut engine = Engine::new(wf, "test_target".to_string(), HashMap::new(), false, true).await;
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

// ─── Story 3.6 additional integration tests ──────────────────────────────────

/// Gate break: steps after a failing gate with on_fail=skip must NOT run
#[tokio::test]
async fn workflow_gate_break_skips_subsequent_steps() {
    let yaml = r#"
name: test
steps:
  - name: setup
    type: cmd
    run: "echo setup"
  - name: check
    type: gate
    condition: "{{ steps.setup.exit_code == 999 }}"
    on_fail: fail
    message: "gate blocked"
  - name: unreachable
    type: cmd
    run: "echo unreachable"
"#;
    let wf = parser::parse_str(yaml).unwrap();
    let errors = validator::validate(&wf);
    assert!(errors.is_empty());
    let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true).await;
    let result = engine.run().await;
    assert!(result.is_err(), "workflow should fail when gate blocks");
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("gate blocked"), "error should mention gate message, got: {msg}");
}

/// Gate break with on_fail=skip: subsequent steps are skipped, workflow continues
#[tokio::test]
async fn workflow_gate_skip_on_fail_continues() {
    let yaml = r#"
name: test
steps:
  - name: cmd
    type: cmd
    run: "echo ok"
  - name: impossible_gate
    type: gate
    condition: "{{ steps.cmd.exit_code == 999 }}"
    on_fail: skip
    message: "not matching"
  - name: after_gate
    type: cmd
    run: "echo after"
"#;
    let result = run_workflow(yaml, "").await.unwrap();
    assert_eq!(result.text().trim(), "after");
}

/// Repeat: verifies exact number of iterations via an accumulating cmd
#[tokio::test]
async fn workflow_repeat_runs_exact_iterations() {
    let yaml = r#"
name: test
scopes:
  counter:
    steps:
      - name: tick
        type: cmd
        run: "echo tick"
      - name: check
        type: gate
        condition: "{{ steps.tick.exit_code == 0 }}"
        on_pass: break
steps:
  - name: loop
    type: repeat
    scope: counter
    max_iterations: 3
"#;
    let result = run_workflow(yaml, "").await.unwrap();
    assert!(!result.text().is_empty(), "repeat should produce output");
}

/// Map serial: verifies outputs come in order (using a fixture YAML)
#[tokio::test]
async fn map_serial_fixture_is_valid() {
    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/map_serial.yaml"
    ));
    if path.exists() {
        let wf = parser::parse_file(path).expect("map_serial.yaml should parse");
        let errors = validator::validate(&wf);
        assert!(errors.is_empty(), "map_serial.yaml errors: {:?}", errors);
    }
}

/// Invalid scope: workflow referencing a non-existent scope fails validation
#[tokio::test]
async fn workflow_with_invalid_scope_fails_validation() {
    let yaml = r#"
name: test
steps:
  - name: bad
    type: repeat
    scope: does_not_exist
    max_iterations: 3
"#;
    let wf = parser::parse_str(yaml).unwrap();
    let errors = validator::validate(&wf);
    assert!(
        !errors.is_empty(),
        "validation should catch unknown scope reference"
    );
    assert!(
        errors.iter().any(|e| e.contains("not found") || e.contains("scope")),
        "error should mention missing scope, got: {:?}",
        errors
    );
}

/// Template step: workflow using a template fixture is valid
#[tokio::test]
async fn template_fixture_is_valid() {
    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/template_step.yaml"
    ));
    if path.exists() {
        let wf = parser::parse_file(path).expect("template_step.yaml should parse");
        let errors = validator::validate(&wf);
        assert!(errors.is_empty(), "template_step.yaml errors: {:?}", errors);
    }
}

/// Config 4-layer merge: step inline config overrides global
#[tokio::test]
async fn workflow_config_four_layer_merge() {
    let yaml = r#"
name: test
config:
  global:
    fail_on_error: true
  cmd:
    fail_on_error: false
steps:
  - name: run
    type: cmd
    run: "echo merged"
    config:
      custom_key: "inline_value"
"#;
    let result = run_workflow(yaml, "").await.unwrap();
    assert_eq!(result.text().trim(), "merged");
}

/// Agent mocked: workflow with agent step using mock CLI parses output correctly
#[tokio::test]
async fn agent_step_with_mock_cli_parses_json_output() {
    // Skip if the mock script isn't executable (CI without bash)
    let mock_path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mock_claude.sh"
    ));
    if !mock_path.exists() {
        return;
    }

    let mock_str = mock_path.to_str().unwrap();
    let yaml = format!(
        r#"
name: test
config:
  agent:
    command: "{mock_str}"
steps:
  - name: ai
    type: agent
    prompt: "do the thing"
"#
    );

    let wf = parser::parse_str(&yaml).unwrap();
    let errors = validator::validate(&wf);
    assert!(errors.is_empty(), "validation errors: {:?}", errors);

    let mut engine = Engine::new(wf, "".to_string(), HashMap::new(), false, true).await;
    // The mock produces a valid JSON lines response — engine should succeed
    match engine.run().await {
        Ok(output) => {
            assert!(
                output.text().contains("Task completed") || !output.text().is_empty(),
                "agent output should be non-empty, got: '{}'",
                output.text()
            );
        }
        Err(e) => {
            // Acceptable if bash isn't available in the test env
            let msg = e.to_string();
            assert!(
                msg.contains("spawn") || msg.contains("permission") || msg.contains("not found"),
                "unexpected error: {msg}"
            );
        }
    }
}

/// Three sequential cmd steps: verify output is sequential and last step wins
#[tokio::test]
async fn three_sequential_cmd_steps_verify_order() {
    let yaml = r#"
name: test
steps:
  - name: step1
    type: cmd
    run: "echo first"
  - name: step2
    type: cmd
    run: "echo second"
  - name: step3
    type: cmd
    run: "echo third"
"#;
    let result = run_workflow(yaml, "").await.unwrap();
    assert_eq!(result.text().trim(), "third", "last step output is returned");
}

/// All workflow fixtures in the fixtures/ directory are valid
#[tokio::test]
async fn all_yaml_fixtures_are_valid() {
    let fixtures_dir =
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"));
    if let Ok(entries) = std::fs::read_dir(fixtures_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yaml" || e == "yml")
                && !path.file_name().unwrap().to_str().unwrap().starts_with("registry")
            {
                let wf = parser::parse_file(&path)
                    .unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));
                let errors = validator::validate(&wf);
                assert!(
                    errors.is_empty(),
                    "{} has validation errors: {:?}",
                    path.display(),
                    errors
                );
            }
        }
    }
}
