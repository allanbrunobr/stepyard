use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::StepError;

// Re-export StackInfo from the canonical definition in detector.rs
pub use super::detector::StackInfo;

/// Resolves a prompt file for a given (function, stack) pair using the
/// ADR-02 fallback chain algorithm:
///
/// 1. `prompts/{function}/{stack.name}.md.tera`
/// 2. Walk `stack.parent_chain` in order
/// 3. `prompts/{function}/_default.md.tera`
/// 4. Return `StepError::Fail` with an actionable message
pub struct PromptResolver;

impl PromptResolver {
    /// Resolve the prompt file path for `function` given `stack`.
    ///
    /// All file-existence checks are async via `tokio::fs::metadata`.
    pub async fn resolve(
        function: &str,
        stack: &StackInfo,
        prompts_dir: &Path,
    ) -> Result<PathBuf, StepError> {
        // Build ordered list of candidates: stack name then each parent
        let mut candidates: Vec<&str> = Vec::new();
        candidates.push(&stack.name);
        for parent in &stack.parent_chain {
            candidates.push(parent.as_str());
        }

        // Detect circular references before doing any I/O
        let mut seen: HashSet<&str> = HashSet::new();
        let mut chain_display: Vec<&str> = Vec::new();
        for name in &candidates {
            if !seen.insert(name) {
                chain_display.push(name);
                return Err(StepError::Fail(format!(
                    "Circular parent chain detected: {}. Check registry.yaml parent fields.",
                    candidates.join(" -> ")
                )));
            }
            chain_display.push(name);
        }

        // Walk the candidate list and return the first file that exists
        for name in &candidates {
            let path = prompts_dir.join(function).join(format!("{}.md.tera", name));
            if tokio::fs::metadata(&path).await.is_ok() {
                return Ok(path);
            }
        }

        // Fall back to _default.md.tera
        let default_path = prompts_dir.join(function).join("_default.md.tera");
        if tokio::fs::metadata(&default_path).await.is_ok() {
            return Ok(default_path);
        }

        // Nothing found — return actionable error
        Err(StepError::Fail(format!(
            "No prompt for {}/{} — create prompts/{}/{}.md.tera or prompts/{}/_default.md.tera",
            function, stack.name, function, stack.name, function
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::fs;

    fn make_stack(name: &str, parents: &[&str]) -> StackInfo {
        StackInfo {
            name: name.to_string(),
            parent_chain: parents.iter().map(|s| s.to_string()).collect(),
            tools: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn direct_match_returns_correct_path() {
        let tmp = tempfile::tempdir().unwrap();
        let prompts_dir = tmp.path();

        fs::create_dir_all(prompts_dir.join("fix-lint"))
            .await
            .unwrap();
        let expected = prompts_dir.join("fix-lint").join("react.md.tera");
        fs::write(&expected, "# fix-lint for react").await.unwrap();

        let stack = make_stack("react", &["typescript", "javascript"]);
        let result = PromptResolver::resolve("fix-lint", &stack, prompts_dir).await;

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), expected);
    }

    #[tokio::test]
    async fn fallback_to_parent_when_direct_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let prompts_dir = tmp.path();

        fs::create_dir_all(prompts_dir.join("fix-lint"))
            .await
            .unwrap();
        // No react.md.tera, but typescript.md.tera exists
        let expected = prompts_dir.join("fix-lint").join("typescript.md.tera");
        fs::write(&expected, "# fix-lint for typescript")
            .await
            .unwrap();

        let stack = make_stack("react", &["typescript", "javascript"]);
        let result = PromptResolver::resolve("fix-lint", &stack, prompts_dir).await;

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), expected);
    }

    #[tokio::test]
    async fn fallback_to_default_when_no_stack_match() {
        let tmp = tempfile::tempdir().unwrap();
        let prompts_dir = tmp.path();

        fs::create_dir_all(prompts_dir.join("fix-lint"))
            .await
            .unwrap();
        let default = prompts_dir.join("fix-lint").join("_default.md.tera");
        fs::write(&default, "# fix-lint default").await.unwrap();

        let stack = make_stack("react", &["typescript", "javascript"]);
        let result = PromptResolver::resolve("fix-lint", &stack, prompts_dir).await;

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert_eq!(result.unwrap(), default);
    }

    #[tokio::test]
    async fn missing_prompt_returns_descriptive_error() {
        let tmp = tempfile::tempdir().unwrap();
        let prompts_dir = tmp.path();

        // No files created at all
        let stack = make_stack("react", &["typescript", "javascript"]);
        let result = PromptResolver::resolve("fix-lint", &stack, prompts_dir).await;

        assert!(result.is_err(), "Expected Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("No prompt for fix-lint/react"),
            "Error should mention function and stack: {msg}"
        );
        assert!(
            msg.contains("_default.md.tera"),
            "Error should suggest _default.md.tera: {msg}"
        );
    }

    #[tokio::test]
    async fn circular_parent_chain_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let prompts_dir = tmp.path();

        // A -> B -> A (circular)
        let stack = make_stack("a", &["b", "a"]);
        let result = PromptResolver::resolve("fix-lint", &stack, prompts_dir).await;

        assert!(result.is_err(), "Expected Err for circular chain");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Circular parent chain detected"),
            "Error should mention circular chain: {msg}"
        );
        assert!(
            msg.contains("registry.yaml"),
            "Error should mention registry.yaml: {msg}"
        );
    }
}
