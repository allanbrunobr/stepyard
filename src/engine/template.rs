// Template preprocessing: handles ?, !, and from("name") syntax before Tera rendering

use crate::engine::context::Context;
use crate::error::StepError;

/// Pre-process a template string before passing to Tera:
/// - `{{ expr? }}` → `{{ expr | default(value="") }}`  (safe accessor)
/// - `{{ expr! }}` → validate existence, then `{{ expr }}`  (strict accessor)
/// - `from("name")` → `from(name="name")`  (positional → named arg for Tera)
pub fn preprocess_template(template: &str, ctx: &Context) -> Result<String, StepError> {
    let mut result = String::with_capacity(template.len());
    let mut remaining = template;

    while !remaining.is_empty() {
        let double_brace = remaining.find("{{");
        let tag_open = remaining.find("{%");

        // Which delimiter comes first?
        let use_tag = match (double_brace, tag_open) {
            (None, None) => {
                result.push_str(remaining);
                return Ok(result);
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
                return Ok(result);
            }
        } else {
            // `{{` expression block
            let db = double_brace.unwrap();
            result.push_str(&remaining[..db]);
            remaining = &remaining[db + 2..];

            if let Some(end) = remaining.find("}}") {
                let expr = &remaining[..end];
                let trimmed = expr.trim();

                if trimmed.ends_with('?') {
                    // Safe accessor: {{ var? }} → {{ var | default(value="") }}
                    let var_name = trimmed[..trimmed.len() - 1].trim();
                    result.push_str("{{ ");
                    result.push_str(var_name);
                    result.push_str(" | default(value=\"\") }}");
                } else if trimmed.ends_with('!') {
                    // Strict accessor: {{ var! }} — check existence first
                    let var_name = trimmed[..trimmed.len() - 1].trim();
                    if !ctx.var_exists(var_name) {
                        return Err(StepError::Fail(format!(
                            "Required output '{}' is missing (strict access)",
                            var_name
                        )));
                    }
                    result.push_str("{{ ");
                    result.push_str(var_name);
                    result.push_str(" }}");
                } else {
                    // Normal expression: convert from("name") → from(name="name")
                    let processed = rewrite_from_calls(expr);
                    result.push_str("{{");
                    result.push_str(&processed);
                    result.push_str("}}");
                }

                remaining = &remaining[end + 2..];
            } else {
                // No closing `}}` — pass through literally
                result.push_str("{{");
            }
        }
    }

    Ok(result)
}

/// Convert positional `from("step_name")` calls to named-arg `from(name="step_name")`.
///
/// Tera functions only accept named arguments, so `from("name")` must become
/// `from(name="name")` before rendering.
fn rewrite_from_calls(expr: &str) -> String {
    expr.replace("from(\"", "from(name=\"")
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
        assert_eq!(result, "{{ missing | default(value=\"\") }}");
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
        // "scan" exists — strict access on "scan.output" should succeed
        let result = preprocess_template("{{ scan.output! }}", &ctx).unwrap();
        assert_eq!(result, "{{ scan.output }}");
    }

    #[test]
    fn from_call_rewritten() {
        let ctx = empty_ctx();
        let result = preprocess_template(r#"{{ from("global-config").output }}"#, &ctx).unwrap();
        assert_eq!(result, r#"{{ from(name="global-config").output }}"#);
    }

    #[test]
    fn plain_expressions_pass_through() {
        let ctx = empty_ctx();
        let result = preprocess_template("{{ target }}", &ctx).unwrap();
        assert_eq!(result, "{{ target }}");
    }

    #[test]
    fn tag_blocks_pass_through() {
        let ctx = empty_ctx();
        let tmpl = "{% if true %}yes{% endif %}";
        let result = preprocess_template(tmpl, &ctx).unwrap();
        assert_eq!(result, tmpl);
    }
}
