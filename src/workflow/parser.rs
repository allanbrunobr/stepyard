use std::path::Path;

use anyhow::{Context, Result};

use super::schema::WorkflowDef;

/// Parse a YAML file into a WorkflowDef
pub fn parse_file(path: &Path) -> Result<WorkflowDef> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    parse_str(&content)
}

/// Parse a YAML string into a WorkflowDef
pub fn parse_str(yaml: &str) -> Result<WorkflowDef> {
    let workflow: WorkflowDef =
        serde_yaml::from_str(yaml).context("Failed to parse workflow YAML")?;
    Ok(workflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_workflow() {
        let yaml = r#"
name: test
steps:
  - name: hello
    type: cmd
    run: "echo hello"
"#;
        let wf = parse_str(yaml).unwrap();
        assert_eq!(wf.name, "test");
        assert_eq!(wf.steps.len(), 1);
        assert_eq!(wf.steps[0].name, "hello");
    }

    #[test]
    fn parse_workflow_with_scopes() {
        let yaml = r#"
name: test
scopes:
  my_scope:
    steps:
      - name: inner
        type: cmd
        run: "echo inner"
    outputs: "{{ steps.inner.stdout }}"
steps:
  - name: outer
    type: repeat
    scope: my_scope
    max_iterations: 3
"#;
        let wf = parse_str(yaml).unwrap();
        assert_eq!(wf.scopes.len(), 1);
        assert!(wf.scopes.contains_key("my_scope"));
        assert_eq!(wf.scopes["my_scope"].steps.len(), 1);
    }

    #[test]
    fn parse_invalid_yaml_fails() {
        let yaml = "this is not: [valid yaml: {";
        assert!(parse_str(yaml).is_err());
    }

    #[test]
    fn parse_missing_required_fields_fails() {
        let yaml = r#"
description: "missing name and steps"
"#;
        assert!(parse_str(yaml).is_err());
    }

    #[test]
    fn parse_fix_issue_yaml() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/workflows/fix-issue.yaml"
        ));
        let wf = parse_file(path).expect("fix-issue.yaml should parse without errors");
        assert_eq!(wf.name, "fix-github-issue");
        assert!(wf.scopes.contains_key("lint_fix"), "lint_fix scope missing");
        assert!(wf.scopes.contains_key("test_fix"), "test_fix scope missing");
        assert!(!wf.steps.is_empty(), "steps should not be empty");
        // Verify scopes have expected steps
        let lint_fix = &wf.scopes["lint_fix"];
        assert_eq!(lint_fix.steps.len(), 3);
        let test_fix = &wf.scopes["test_fix"];
        assert_eq!(test_fix.steps.len(), 3);
    }
}
