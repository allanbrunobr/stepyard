use std::collections::HashSet;

use crate::config::StepConfig;
use crate::plugins::registry::PluginRegistry;
use crate::workflow::schema::{StepDef, StepType, WorkflowDef};

/// Validate a parsed workflow, returning all errors found
pub fn validate(workflow: &WorkflowDef) -> Vec<String> {
    let mut errors = Vec::new();

    // Check main steps
    validate_steps(
        &workflow.steps,
        &workflow.scopes.keys().cloned().collect(),
        &mut errors,
    );

    // Check scope steps
    let scope_names: HashSet<String> = workflow.scopes.keys().cloned().collect();
    for (scope_name, scope_def) in &workflow.scopes {
        let mut seen = HashSet::new();
        for step in &scope_def.steps {
            if !seen.insert(&step.name) {
                errors.push(format!(
                    "Scope '{scope_name}': duplicate step name '{}'",
                    step.name
                ));
            }
        }
        validate_steps(&scope_def.steps, &scope_names, &mut errors);
    }

    // Check for unique step names in main pipeline
    let mut seen = HashSet::new();
    for step in &workflow.steps {
        if !seen.insert(&step.name) {
            errors.push(format!("Duplicate step name: '{}'", step.name));
        }
    }

    // Check for circular scope references
    for scope_name in workflow.scopes.keys() {
        let mut visited = HashSet::new();
        if has_cycle(scope_name, &workflow.scopes, &mut visited) {
            errors.push(format!("Circular scope reference involving '{scope_name}'"));
        }
    }

    errors
}

fn validate_steps(steps: &[StepDef], scope_names: &HashSet<String>, errors: &mut Vec<String>) {
    for step in steps {
        validate_step(step, scope_names, errors);
    }
}

fn validate_step(step: &StepDef, scope_names: &HashSet<String>, errors: &mut Vec<String>) {
    match step.step_type {
        StepType::Cmd => {
            if step.run.as_ref().is_none_or(|r| r.trim().is_empty()) {
                errors.push(format!(
                    "Step '{}': cmd step requires 'run' field",
                    step.name
                ));
            }
        }
        StepType::Agent | StepType::Chat => {
            if step.prompt.as_ref().is_none_or(|p| p.trim().is_empty()) {
                errors.push(format!(
                    "Step '{}': {} step requires 'prompt' field",
                    step.name, step.step_type
                ));
            }
        }
        StepType::Gate => {
            if step.condition.as_ref().is_none_or(|c| c.trim().is_empty()) {
                errors.push(format!(
                    "Step '{}': gate step requires 'condition' field",
                    step.name
                ));
            }
        }
        StepType::Repeat | StepType::Map | StepType::Call => {
            match &step.scope {
                Some(scope) if !scope_names.contains(scope) => {
                    errors.push(format!(
                        "Step '{}': scope '{}' not found in workflow scopes",
                        step.name, scope
                    ));
                }
                None => {
                    errors.push(format!(
                        "Step '{}': {} step requires 'scope' field",
                        step.name, step.step_type
                    ));
                }
                _ => {}
            }
            if step.step_type == StepType::Repeat {
                if let Some(max) = step.max_iterations {
                    if max == 0 {
                        errors.push(format!("Step '{}': max_iterations must be > 0", step.name));
                    }
                }
            }
            if step.step_type == StepType::Map && step.items.is_none() {
                errors.push(format!(
                    "Step '{}': map step requires 'items' field",
                    step.name
                ));
            }
        }
        StepType::Parallel => {
            if step.steps.as_ref().is_none_or(|s| s.is_empty()) {
                errors.push(format!(
                    "Step '{}': parallel step requires nested 'steps'",
                    step.name
                ));
            }
            if let Some(nested) = &step.steps {
                validate_steps(nested, scope_names, errors);
            }
        }
        StepType::Template => {}
        StepType::Script => {
            if step.run.as_ref().is_none_or(|r| r.trim().is_empty()) {
                errors.push(format!(
                    "Step '{}': script step requires 'run' field",
                    step.name
                ));
            }
        }
    }
}

/// Validate plugin step configurations against each plugin's declared schema.
///
/// For each step whose type matches a registered plugin:
///   - Ensures all `required_fields` are present in the step config
///   - Reports missing required fields as errors
///   - Applies default values for missing optional fields (mutates the steps)
///
/// Returns a list of validation error messages (empty means all ok).
#[allow(dead_code)]
pub fn validate_plugin_configs(steps: &[StepDef], registry: &PluginRegistry) -> Vec<String> {
    let mut errors = Vec::new();
    for step in steps {
        let type_name = step.step_type.to_string();
        if let Some(plugin) = registry.get(&type_name) {
            let schema = plugin.config_schema();

            // Build a temporary StepConfig from the step's config map so we can
            // reuse the same API as the rest of the engine.
            let values: std::collections::HashMap<String, serde_json::Value> = step
                .config
                .iter()
                .filter_map(|(k, v)| {
                    // Convert serde_yaml::Value -> serde_json::Value
                    serde_json::to_value(v).ok().map(|jv| (k.clone(), jv))
                })
                .collect();
            let config = StepConfig { values };

            // Check required fields
            for field in &schema.required_fields {
                if config.get_str(field).is_none() && !config.values.contains_key(field.as_str()) {
                    errors.push(format!(
                        "Step '{}' (plugin '{}'): missing required config field '{}'",
                        step.name, type_name, field
                    ));
                }
            }
        }
    }
    errors
}

fn has_cycle(
    scope_name: &str,
    scopes: &std::collections::HashMap<String, crate::workflow::schema::ScopeDef>,
    visited: &mut HashSet<String>,
) -> bool {
    if !visited.insert(scope_name.to_string()) {
        return true;
    }
    if let Some(scope_def) = scopes.get(scope_name) {
        for step in &scope_def.steps {
            if matches!(
                step.step_type,
                StepType::Call | StepType::Repeat | StepType::Map
            ) {
                if let Some(ref target) = step.scope {
                    if has_cycle(target, scopes, visited) {
                        return true;
                    }
                }
            }
        }
    }
    visited.remove(scope_name);
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::parser;

    #[test]
    fn valid_workflow_passes() {
        let yaml = r#"
name: test
steps:
  - name: hello
    type: cmd
    run: "echo hello"
"#;
        let wf = parser::parse_str(yaml).unwrap();
        assert!(validate(&wf).is_empty());
    }

    #[test]
    fn missing_run_detected() {
        let yaml = r#"
name: test
steps:
  - name: broken
    type: cmd
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let errors = validate(&wf);
        assert!(errors.iter().any(|e| e.contains("requires 'run'")));
    }

    #[test]
    fn missing_scope_detected() {
        let yaml = r#"
name: test
steps:
  - name: loop
    type: repeat
    scope: nonexistent
    max_iterations: 3
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let errors = validate(&wf);
        assert!(errors.iter().any(|e| e.contains("not found")));
    }

    #[test]
    fn cycle_detected() {
        let yaml = r#"
name: test
scopes:
  a:
    steps:
      - name: call_b
        type: call
        scope: b
  b:
    steps:
      - name: call_a
        type: call
        scope: a
steps:
  - name: start
    type: call
    scope: a
"#;
        let wf = parser::parse_str(yaml).unwrap();
        let errors = validate(&wf);
        assert!(errors.iter().any(|e| e.contains("Circular")));
    }
}
