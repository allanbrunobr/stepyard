//! Harness-local Tera renderer for gate conditions and cross-step refs.
//!
//! PR 2 of Task #31. The v2 engine needs a minimal template pass so gate
//! conditions like `{{ steps.build.exit_code }} == 0` and cmd `command`
//! fields referencing `{{ target }}` or `{{ vars.X }}` resolve to concrete
//! strings before execution.
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

/// Immutable view of the data a template can reference.
///
/// Lifetime-tied to the engine call site so no cloning happens when we
/// only need to render one string. `steps` is rebuilt from the session
/// log in `Engine::resume`; `target` / `vars` come from [`RunContext`].
///
/// `scope` is intentionally absent — the field will appear in PR 3 with
/// `repeat`/`map`/`call` executors that actually bind it. Exposing it
/// now with nothing behind it would be the same contract-without-substance
/// anti-pattern as accepting `break`/`skip` gate actions in PR 2.
///
/// [`RunContext`]: crate::RunContext
#[derive(Debug, Serialize)]
pub struct RenderContext<'a> {
    pub steps: &'a HashMap<String, StepOutputSnapshot>,
    pub target: &'a str,
    pub vars: &'a HashMap<String, String>,
}

/// Render `template` against `ctx` using a throwaway Tera instance.
pub fn render(template: &str, ctx: &RenderContext<'_>) -> Result<String, RenderError> {
    let tera_ctx =
        tera::Context::from_serialize(ctx).map_err(|e| RenderError::Tera(e.to_string()))?;
    tera::Tera::one_off(template, &tera_ctx, false).map_err(|e| RenderError::Tera(e.to_string()))
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
        };

        let out = render("{{ vars.branch }}", &ctx).unwrap();
        assert_eq!(out, "main");
    }

    #[test]
    fn unknown_step_reference_is_an_error() {
        let (steps, target, vars) = ctx_with(HashMap::new(), "", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
        };

        let err = render("{{ steps.missing.exit_code }}", &ctx).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("template render failed"), "got: {msg}");
    }

    #[test]
    fn templateless_string_passes_through() {
        let (steps, target, vars) = ctx_with(HashMap::new(), "", HashMap::new());
        let ctx = RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
        };

        let out = render("echo hello", &ctx).unwrap();
        assert_eq!(out, "echo hello");
    }
}
