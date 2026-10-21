//! Harness-local Tera renderer for gate conditions and cross-step refs.
//!
//! PR 2 of Task #31. The v2 engine needs a minimal template pass so gate
//! conditions like `{{ steps.build.exit_code }} == 0` resolve to concrete
//! strings before evaluation. It renders gate conditions, top-level and
//! scoped `cmd` commands, template steps, and agent/chat prompts.
//! `env` values are not rendered.
//!
//! # Scope
//!
//! Deliberately narrow: bare [`tera::Tera::one_off`], no held engine
//! instance, no custom filters, no macro registration. The v1 template
//! module (`src/engine/template.rs`) carries the richer surface
//! (`?`/`!` accessors, `from("name")`, `prompts.*`) — porting those lives
//! in later PRs of Task #31.
//!
//! # Why not put Tera in `stepyard-core`
//!
//! `stepyard-core` is the IO-free contract crate. Tera is runtime
//! machinery (template parsing, regex, string allocation) and the
//! engine's rendering needs are session-shaped (outputs map rebuilt
//! from the log). Keeping the renderer here — and not behind a
//! `&Tera` field on `HarnessConfig` — preserves the crate-boundary
//! layering called out in the architecture doc.

use std::collections::HashMap;

use serde::Serialize;
use stepyard_core::StepOutputSnapshot;

/// Template rendering errors surfaced to the engine.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// Tera parse/render failure. The inner string is Tera's own error
    /// chain, flattened for stable display in `StepFailed` events.
    #[error("template render failed: {0}")]
    Tera(String),
}

/// Iteration-local view exposed to templates rendered inside a
/// `call` / `repeat` / `map` scope body (PR 3 of Task #31). Bound to
/// `{{ scope.value }}` and `{{ scope.index }}` — v1 parity.
///
/// `value` is `serde_json::Value` (not `String`) so map iterations over
/// a JSON array expose the raw element to Tera (e.g. `{{ scope.value.id }}`
/// when the item is an object). For `call` / `repeat` the value holds
/// the container step's `initial_value:` seed, or `null` when the seed
/// is absent.
#[derive(Debug, Serialize)]
pub struct ScopeView {
    pub value: serde_json::Value,
    pub index: u32,
}

/// Immutable view of the data a template can reference.
///
/// Lifetime-tied to the engine call site so no cloning happens when we
/// only need to render one string. `steps` is rebuilt from the session
/// log in `Engine::resume`; `target` / `vars` come from [`RunContext`].
/// `scope` is populated by the scope runner (PR 3) when rendering a
/// cmd command or gate condition inside a `call` / `repeat` / `map`
/// body — `None` for top-level steps.
///
/// [`RunContext`]: crate::RunContext
#[derive(Debug, Serialize)]
pub struct RenderContext<'a> {
    pub steps: &'a HashMap<String, StepOutputSnapshot>,
    pub target: &'a str,
    pub vars: &'a HashMap<String, String>,
    /// Active scope frame. Absent for top-level steps (no `scope.value`
    /// / `scope.index` visible to the template).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeView>,
}

/// Template-facing view of one step's output.
///
/// Unlike [`StepOutputSnapshot`] (whose wire form skips empty strings),
/// every field is always present so `{{ steps.x.stderr }}` renders as an
/// empty string instead of failing when the step printed nothing.
/// `response` mirrors `stdout`: v2 stores agent/chat replies as `stdout`,
/// while v1 workflows reference `{{ steps.x.response }}`.
#[derive(Debug, Serialize)]
struct StepView<'a> {
    stdout: &'a str,
    stderr: &'a str,
    exit_code: i32,
    response: &'a str,
}

/// Cheap pre-check: does `s` contain any Tera delimiter? Callers use it
/// to skip rendering plain shell commands entirely.
pub fn contains_template_markers(s: &str) -> bool {
    s.contains("{{") || s.contains("{%") || s.contains("{#")
}

/// Render `template` against `ctx` using a throwaway Tera instance.
///
/// Exposed names: `steps`, `target`, `vars`, `args` (alias of `vars`,
/// v1 parity for `{{ args.KEY }}`) and, inside scope bodies, `scope`.
pub fn render(template: &str, ctx: &RenderContext<'_>) -> Result<String, RenderError> {
    let steps: HashMap<&str, StepView<'_>> = ctx
        .steps
        .iter()
        .map(|(name, snap)| {
            (
                name.as_str(),
                StepView {
                    stdout: &snap.stdout,
                    stderr: &snap.stderr,
                    exit_code: snap.exit_code,
                    response: &snap.stdout,
                },
            )
        })
        .collect();
    let mut tera_ctx = tera::Context::new();
    tera_ctx.insert("steps", &steps);
    tera_ctx.insert("target", ctx.target);
    tera_ctx.insert("vars", ctx.vars);
    tera_ctx.insert("args", ctx.vars);
    if let Some(scope) = &ctx.scope {
        tera_ctx.insert("scope", scope);
    }
    tera::Tera::one_off(template, &tera_ctx, false).map_err(|e| RenderError::Tera(error_chain(&e)))
}

/// Flatten an error and its `source()` chain into one line. Tera's top
/// error is only "Failed to render '__tera_one_off'"; the useful detail
/// (for example "Variable `MODE` not found") lives in the sources.
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(stdout: &str, exit: i32) -> StepOutputSnapshot {
        StepOutputSnapshot {
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: exit,
        }
    }

    fn ctx_with(
        steps: HashMap<String, StepOutputSnapshot>,
        target: &str,
        vars: HashMap<String, String>,
    ) -> (
        HashMap<String, StepOutputSnapshot>,
        String,
        HashMap<String, String>,
    ) {
        (steps, target.into(), vars)
    }

    #[test]
    fn renders_step_exit_code_reference() {
        let mut steps = HashMap::new();
        steps.insert("build".into(), snap("ok\n", 0));
        let (steps, target, vars) = ctx_with(steps, "prod", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };

        let out = render("{{ steps.build.exit_code }}", &ctx).unwrap();
        assert_eq!(out, "0");
    }

    #[test]
    fn renders_step_stdout_reference() {
        let mut steps = HashMap::new();
        steps.insert("greet".into(), snap("hello", 0));
        let (steps, target, vars) = ctx_with(steps, "", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };

        let out = render("msg={{ steps.greet.stdout }}", &ctx).unwrap();
        assert_eq!(out, "msg=hello");
    }

    #[test]
    fn renders_target() {
        let (steps, target, vars) = ctx_with(HashMap::new(), "staging-01", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };

        let out = render("deploy to {{ target }}", &ctx).unwrap();
        assert_eq!(out, "deploy to staging-01");
    }

    #[test]
    fn renders_vars() {
        let mut vars = HashMap::new();
        vars.insert("branch".into(), "main".into());
        let (steps, target, vars) = ctx_with(HashMap::new(), "", vars);
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };

        let out = render("{{ vars.branch }}", &ctx).unwrap();
        assert_eq!(out, "main");
    }

    #[test]
    fn response_aliases_stdout_and_empty_fields_render() {
        let mut steps = HashMap::new();
        steps.insert("review".into(), snap("LGTM", 0));
        let (steps, target, vars) = ctx_with(steps, "", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };

        let out = render(
            "{{ steps.review.response }}|{{ steps.review.stderr }}|",
            &ctx,
        )
        .unwrap();
        assert_eq!(out, "LGTM||");
    }

    #[test]
    fn args_aliases_vars() {
        let mut vars = HashMap::new();
        vars.insert("MODE".into(), "strict".into());
        let (steps, target, vars) = ctx_with(HashMap::new(), "", vars);
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };

        let out = render("{{ args.MODE }} {{ args.days | default(value='7') }}", &ctx).unwrap();
        assert_eq!(out, "strict 7");
    }

    #[test]
    fn template_marker_detection() {
        assert!(contains_template_markers("echo {{ target }}"));
        assert!(contains_template_markers("{% if x %}y{% endif %}"));
        assert!(!contains_template_markers("echo hello && ls"));
    }

    #[test]
    fn unknown_step_reference_is_an_error() {
        let (steps, target, vars) = ctx_with(HashMap::new(), "", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };

        let err = render("{{ steps.missing.exit_code }}", &ctx).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("template render failed"), "got: {msg}");
        assert!(
            msg.contains("steps.missing.exit_code"),
            "cause missing: {msg}"
        );
    }

    #[test]
    fn templateless_string_passes_through() {
        let (steps, target, vars) = ctx_with(HashMap::new(), "", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };

        let out = render("echo hello", &ctx).unwrap();
        assert_eq!(out, "echo hello");
    }

    #[test]
    fn scope_bindings_render() {
        // PR 3 of #31: scope.value + scope.index must reach templates.
        let (steps, target, vars) = ctx_with(HashMap::new(), "", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: Some(ScopeView {
                value: serde_json::json!("hello"),
                index: 3,
            }),
        };

        let out = render("{{ scope.value }} @ {{ scope.index }}", &ctx).unwrap();
        assert_eq!(out, "hello @ 3");
    }

    #[test]
    fn scope_value_can_hold_object_and_index_fields() {
        // Map iterations over a JSON array of objects expose
        // scope.value.<field> directly (no string coercion).
        let (steps, target, vars) = ctx_with(HashMap::new(), "", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: Some(ScopeView {
                value: serde_json::json!({ "name": "alpha", "port": 8080 }),
                index: 0,
            }),
        };

        let out = render("{{ scope.value.name }}:{{ scope.value.port }}", &ctx).unwrap();
        assert_eq!(out, "alpha:8080");
    }
}
