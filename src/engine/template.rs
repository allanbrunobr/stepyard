// Template preprocessing: handles ?, !, and from("name") syntax before Tera rendering

use std::collections::HashMap;

use crate::engine::context::Context;
use crate::error::StepError;

/// Result of template preprocessing
#[derive(Debug)]
pub struct PreprocessResult {
    /// The transformed template string (ready for Tera)
    pub template: String,
    /// Extra variables to inject into the Tera context (from() lookups)
    pub injected: HashMap<String, serde_json::Value>,
}

/// Pre-process a template string before passing to Tera:
///
/// - `{{ expr? }}` → `{{ expr | default(value="") }}`  (safe accessor)
/// - `{{ expr! }}` → validate existence, then `{{ expr }}`  (strict accessor)
/// - `from("name")` → `__from_<name>__` variable (injected into Tera context)
///   - Step data is injected under the sanitized variable name
///   - Missing step in normal/strict context → `StepError::Fail`
///   - Missing step in safe (`?`) context → empty sentinel injected
pub fn preprocess_template(template: &str, ctx: &Context) -> Result<PreprocessResult, StepError> {
    let mut result = String::with_capacity(template.len());
    let mut injected: HashMap<String, serde_json::Value> = HashMap::new();
    let mut remaining = template;

    while !remaining.is_empty() {
        let double_brace = remaining.find("{{");
        let tag_open = remaining.find("{%");

        let use_tag = match (double_brace, tag_open) {
            (None, None) => {
                result.push_str(remaining);
                return Ok(PreprocessResult { template: result, injected });
            }
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (Some(db), Some(to)) => to < db,
        };

        if use_tag {
            // `{%` block — pass through unchanged until `%}`
            let to = tag_open.unwrap();
            result.push_str(&remaining[..to]);
            remaining = &remaining[to..];
            if let Some(end) = remaining.find("%}") {
                result.push_str(&remaining[..end + 2]);
                remaining = &remaining[end + 2..];
            } else {
                result.push_str(remaining);
                return Ok(PreprocessResult { template: result, injected });
            }
        } else {
            // `{{` expression block
            let db = double_brace.unwrap();
            result.push_str(&remaining[..db]);
            remaining = &remaining[db + 2..];

            if let Some(end) = remaining.find("}}") {
                let expr = &remaining[..end];
                let trimmed = expr.trim();

                let processed = if trimmed.ends_with('?') {
                    // Safe accessor: strip ?, transform from() calls (missing = empty), then apply default filter
                    let inner = trimmed[..trimmed.len() - 1].trim();
                    let transformed = transform_from_calls(inner, ctx, true, &mut injected)?;
                    format!("{{{{ {} | default(value=\"\") }}}}", transformed)
                } else if trimmed.ends_with('!') {
                    // Strict accessor: strip !, transform from() calls (missing = error), then check existence
                    let inner = trimmed[..trimmed.len() - 1].trim();
                    let transformed = transform_from_calls(inner, ctx, false, &mut injected)?;
                    if !ctx.var_exists(&transformed) {
                        return Err(StepError::Fail(format!(
                            "Required output '{}' is missing (strict access)",
                            inner
                        )));
                    }
                    format!("{{{{ {} }}}}", transformed)
                } else {
                    // Normal expression: transform from() calls (missing = error)
                    let transformed = transform_from_calls(trimmed, ctx, false, &mut injected)?;
                    format!("{{{{{}}}}}", if transformed == trimmed {
                        expr.to_string()
                    } else {
                        format!(" {} ", transformed)
                    })
                };

                result.push_str(&processed);
                remaining = &remaining[end + 2..];
            } else {
                // No closing `}}` — pass through literally
                result.push_str("{{");
            }
        }
    }

    Ok(PreprocessResult { template: result, injected })
}

/// Transform `from("step_name")` calls in an expression to `__from_<name>__` variables.
///
/// For each `from("name")` found:
/// - Looks up the step in `ctx`
/// - If found: injects step data under `__from_<name>__`
/// - If not found and `is_safe`: injects an empty sentinel object
/// - If not found and not safe: returns `StepError::Fail`
fn transform_from_calls(
    expr: &str,
    ctx: &Context,
    is_safe: bool,
    injected: &mut HashMap<String, serde_json::Value>,
) -> Result<String, StepError> {
    if !expr.contains("from(\"") {
        return Ok(expr.to_string());
    }

    let mut result = expr.to_string();
    let mut search_from = 0;

    while let Some(rel_pos) = result[search_from..].find("from(\"") {
        let abs_pos = search_from + rel_pos;
        let after_open = abs_pos + 6; // len of 'from("'

        let Some(close_quote) = result[after_open..].find('"') else {
            break;
        };

        let name = result[after_open..after_open + close_quote].to_string();
        let var_name = sanitize_step_name(&name);

        // Expect ')' right after the closing quote
        let end_of_call = after_open + close_quote + 1; // points to char after closing "
        let end_of_call = if result.as_bytes().get(end_of_call) == Some(&b')') {
            end_of_call + 1
        } else {
            end_of_call
        };

        // Look up step and inject
        match ctx.get_from_value(&name) {
            Some(val) => {
                injected.insert(var_name.clone(), val);
            }
            None if is_safe => {
                // Safe context: inject an empty sentinel with an empty output field
                injected
                    .entry(var_name.clone())
                    .or_insert_with(|| serde_json::json!({"output": null}));
            }
            None => {
                return Err(StepError::Fail(format!(
                    "Step '{}' not found in any scope",
                    name
                )));
            }
        }

        // Replace `from("name")` with `__from_name__` in the expression
        result = format!("{}{}{}", &result[..abs_pos], &var_name, &result[end_of_call..]);
        search_from = abs_pos + var_name.len();
    }

    Ok(result)
}

/// Sanitize a step name into a valid Tera variable name.
/// Converts `global-config` → `__from_global_config__`
fn sanitize_step_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    format!("__from_{}__", sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn empty_ctx() -> Context {
        Context::new("".to_string(), HashMap::new())
    }

    #[test]
    fn safe_accessor_replaced() {
        let ctx = empty_ctx();
        let result = preprocess_template("{{ missing? }}", &ctx).unwrap();
        assert_eq!(result.template, "{{ missing | default(value=\"\") }}");
    }

    #[test]
    fn strict_accessor_fails_when_missing() {
        let ctx = empty_ctx();
        let err = preprocess_template("{{ missing! }}", &ctx).unwrap_err();
        assert!(err.to_string().contains("missing"), "{err}");
        assert!(err.to_string().contains("strict access"), "{err}");
    }

    #[test]
    fn strict_accessor_passes_when_present() {
        use crate::steps::{CmdOutput, StepOutput};
        use std::time::Duration;
        let mut ctx = empty_ctx();
        ctx.store(
            "scan",
            StepOutput::Cmd(CmdOutput {
                stdout: "hello".to_string(),
                stderr: String::new(),
                exit_code: 0,
                duration: Duration::ZERO,
            }),
        );
        let result = preprocess_template("{{ scan.output! }}", &ctx).unwrap();
        assert_eq!(result.template, "{{ scan.output }}");
    }

    #[test]
    fn from_call_injects_variable() {
        use crate::steps::{CmdOutput, StepOutput};
        use std::time::Duration;
        let mut ctx = empty_ctx();
        ctx.store(
            "global-config",
            StepOutput::Cmd(CmdOutput {
                stdout: "prod".to_string(),
                stderr: String::new(),
                exit_code: 0,
                duration: Duration::ZERO,
            }),
        );
        let result = preprocess_template(r#"{{ from("global-config").output }}"#, &ctx).unwrap();
        assert!(result.template.contains("__from_global_config__"), "{}", result.template);
        assert!(result.injected.contains_key("__from_global_config__"));
    }

    #[test]
    fn from_call_missing_step_fails_in_normal_mode() {
        let ctx = empty_ctx();
        let err = preprocess_template(r#"{{ from("nonexistent").output }}"#, &ctx).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        assert!(err.to_string().contains("nonexistent"), "{err}");
    }

    #[test]
    fn from_call_missing_step_safe_in_accessor_mode() {
        let ctx = empty_ctx();
        // With ? suffix, missing step should NOT fail — returns empty
        let result = preprocess_template(r#"{{ from("nonexistent").output? }}"#, &ctx).unwrap();
        assert!(result.template.contains("default(value=\"\")"), "{}", result.template);
        assert!(result.injected.contains_key("__from_nonexistent__"));
    }

    #[test]
    fn plain_expressions_pass_through() {
        let ctx = empty_ctx();
        let result = preprocess_template("{{ target }}", &ctx).unwrap();
        assert_eq!(result.template, "{{ target }}");
    }

    #[test]
    fn tag_blocks_pass_through() {
        let ctx = empty_ctx();
        let tmpl = "{% if true %}yes{% endif %}";
        let result = preprocess_template(tmpl, &ctx).unwrap();
        assert_eq!(result.template, tmpl);
    }
}
