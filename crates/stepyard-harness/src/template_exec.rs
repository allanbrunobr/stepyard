//! Template step executor.
//!
//! PR 4 of Task #31. A `template` step reads a `.md.tera` file from the
//! workflow's `prompts_dir`, renders it against the harness render
//! context, and emits the rendered text as the step's `stdout` in a
//! `StepCompleted` event. No sandbox, no tokio-select — pure in-process
//! file read + Tera evaluation, replay-safe via the log like every other
//! step.
//!
//! # Scope
//!
//! * Two-pass render matching v1 `src/steps/template_step.rs`: first the
//!   `prompt` field (if set) is rendered to a path basename, then the
//!   file content is rendered.
//! * Output shape is unified with cmd: `{ stdout, stderr: "", exit_code: 0 }`.
//!   Cross-step refs (`{{ steps.tmpl.stdout }}`) work without a new
//!   variant or event-schema change.
//! * Path traversal guardrail: the rendered prompt must be a relative
//!   path with no `..` components. Subdirectories (`fix-lint/react`)
//!   stay allowed — v1 parity — but `../../etc/passwd` is rejected.

use std::path::{Component, Path, PathBuf};

use crate::render::{render, RenderError};

#[derive(Debug, thiserror::Error)]
pub(crate) enum TemplateExecError {
    #[error("template prompt render failed: {0}")]
    PromptRender(String),

    #[error(
        "template prompt `{prompt}` rejected: must be a relative path \
         (no absolute paths, no `..` components)"
    )]
    UnsafePath { prompt: String },

    #[error("template file `{path}` not found: {error}")]
    FileNotFound { path: String, error: String },

    #[error("template content render failed: {0}")]
    ContentRender(String),
}

impl From<RenderError> for TemplateExecError {
    fn from(e: RenderError) -> Self {
        TemplateExecError::ContentRender(e.to_string())
    }
}

/// Resolve a template's prompt field to a file path under `prompts_dir`,
/// rejecting absolute paths and `..` components at the lexical level.
pub(crate) fn resolve_template_path(
    prompts_dir: &Path,
    rendered_prompt: &str,
) -> Result<PathBuf, TemplateExecError> {
    let as_path = Path::new(rendered_prompt);
    if as_path.is_absolute() {
        return Err(TemplateExecError::UnsafePath {
            prompt: rendered_prompt.to_string(),
        });
    }
    for comp in as_path.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(TemplateExecError::UnsafePath {
                    prompt: rendered_prompt.to_string(),
                });
            }
        }
    }
    Ok(prompts_dir.join(format!("{rendered_prompt}.md.tera")))
}

async fn canonicalize_template_path(
    prompts_dir: &Path,
    path: &Path,
) -> Result<PathBuf, TemplateExecError> {
    let resolved =
        tokio::fs::canonicalize(path)
            .await
            .map_err(|e| TemplateExecError::FileNotFound {
                path: path.display().to_string(),
                error: e.to_string(),
            })?;
    let base = tokio::fs::canonicalize(prompts_dir).await.map_err(|e| {
        TemplateExecError::FileNotFound {
            path: prompts_dir.display().to_string(),
            error: e.to_string(),
        }
    })?;
    if !resolved.starts_with(&base) {
        return Err(TemplateExecError::UnsafePath {
            prompt: path.display().to_string(),
        });
    }
    Ok(resolved)
}

/// Render a template step: resolve its file path, read it, render the
/// content. Returns the rendered text on success.
pub(crate) async fn render_template(
    prompts_dir: &Path,
    prompt_field: Option<&str>,
    fallback_name: &str,
    ctx: &crate::render::RenderContext<'_>,
) -> Result<String, TemplateExecError> {
    let basename = match prompt_field {
        Some(p) => render(p, ctx).map_err(|e| TemplateExecError::PromptRender(e.to_string()))?,
        None => fallback_name.to_string(),
    };
    let path = resolve_template_path(prompts_dir, &basename)?;
    let safe_path = canonicalize_template_path(prompts_dir, &path).await?;
    let contents = tokio::fs::read_to_string(&safe_path).await.map_err(|e| {
        TemplateExecError::FileNotFound {
            path: safe_path.display().to_string(),
            error: e.to_string(),
        }
    })?;
    let rendered = render(&contents, ctx)?;
    Ok(rendered)
}

/// Default value when the workflow omits `prompts_dir:`. Matches v1
/// (`src/engine/context.rs:58`).
pub(crate) const DEFAULT_PROMPTS_DIR: &str = "prompts";

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use stepyard_core::StepOutputSnapshot;

    fn empty_ctx() -> (
        HashMap<String, StepOutputSnapshot>,
        String,
        HashMap<String, String>,
    ) {
        (HashMap::new(), String::new(), HashMap::new())
    }

    #[test]
    fn relative_basename_resolves_under_prompts_dir() {
        let p = resolve_template_path(Path::new("prompts"), "greet").unwrap();
        assert_eq!(p, PathBuf::from("prompts/greet.md.tera"));
    }

    #[test]
    fn subdirectory_is_allowed() {
        let p = resolve_template_path(Path::new("prompts"), "fix-lint/react").unwrap();
        assert_eq!(p, PathBuf::from("prompts/fix-lint/react.md.tera"));
    }

    #[test]
    fn absolute_path_rejected() {
        let err = resolve_template_path(Path::new("prompts"), "/etc/passwd").unwrap_err();
        assert!(
            matches!(err, TemplateExecError::UnsafePath { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn parent_dir_rejected() {
        let err = resolve_template_path(Path::new("prompts"), "../secret").unwrap_err();
        assert!(
            matches!(err, TemplateExecError::UnsafePath { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn nested_parent_dir_rejected() {
        let err = resolve_template_path(Path::new("prompts"), "sub/../../etc/passwd").unwrap_err();
        assert!(
            matches!(err, TemplateExecError::UnsafePath { .. }),
            "got {err:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escape_rejected_after_canonicalize() {
        let tmp = tempfile::tempdir().unwrap();
        let prompts = tmp.path().join("prompts");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&prompts).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md.tera"), "leaked").unwrap();
        std::os::unix::fs::symlink(&outside, prompts.join("link")).unwrap();

        let (steps, target, vars) = empty_ctx();
        let ctx = crate::render::RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };
        let err = render_template(&prompts, Some("link/secret"), "unused", &ctx)
            .await
            .unwrap_err();
        assert!(
            matches!(err, TemplateExecError::UnsafePath { .. }),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn renders_tera_against_context() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("greet.md.tera"), "Hi {{ target }}!").unwrap();

        let (steps, target, vars) = (HashMap::new(), "world".to_string(), HashMap::new());
        let ctx = crate::render::RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };
        let out = render_template(tmp.path(), None, "greet", &ctx).await.unwrap();
        assert_eq!(out, "Hi world!");
    }

    #[tokio::test]
    async fn missing_file_returns_structured_error() {
        let tmp = tempfile::tempdir().unwrap();
        let (steps, target, vars) = empty_ctx();
        let ctx = crate::render::RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };
        let err = render_template(tmp.path(), None, "nonexistent", &ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, TemplateExecError::FileNotFound { .. }));
    }

    #[tokio::test]
    async fn dynamic_prompt_renders_basename_first() {
        let tmp = tempfile::tempdir().unwrap();
        let subdir = tmp.path().join("fix-lint");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(subdir.join("react.md.tera"), "{{ target }}/react").unwrap();

        let mut vars = HashMap::new();
        vars.insert("stack".into(), "react".into());
        let steps = HashMap::new();
        let target = "myapp".to_string();
        let ctx = crate::render::RenderContext {
            steps: &steps,
            target: &target,
            vars: &vars,
            scope: None,
        };
        let out = render_template(
            tmp.path(),
            Some("fix-lint/{{ vars.stack }}"),
            "unused",
            &ctx,
        )
        .await
        .unwrap();
        assert_eq!(out, "myapp/react");
    }
}
