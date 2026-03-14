use std::path::{Path, PathBuf};

use crate::error::StepError;

use super::detector::StackInfo;

pub struct PromptResolver;

impl PromptResolver {
    /// Resolve the prompt file path for a given function and stack.
    ///
    /// Resolution order:
    /// 1. `prompts/<function>/<stack_name>.md.tera`
    /// 2. `prompts/<function>/<parent>.md.tera` (for each parent in chain)
    /// 3. `prompts/<function>/_default.md.tera`
    pub fn resolve(
        function: &str,
        stack_info: &StackInfo,
        prompts_dir: &Path,
    ) -> Result<PathBuf, StepError> {
        // Check stack-specific file first
        let path = prompts_dir
            .join(function)
            .join(format!("{}.md.tera", stack_info.name));
        if path.exists() {
            return Ok(path);
        }

        // Walk parent chain
        for parent in &stack_info.parent_chain {
            let path = prompts_dir
                .join(function)
                .join(format!("{}.md.tera", parent));
            if path.exists() {
                return Ok(path);
            }
        }

        // Fallback to _default
        let default_path = prompts_dir.join(function).join("_default.md.tera");
        if default_path.exists() {
            return Ok(default_path);
        }

        Err(StepError::Fail(format!(
            "No prompt for {function}/{} -- create prompts/{function}/{}.md.tera or prompts/{function}/_default.md.tera",
            stack_info.name, stack_info.name
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::detector::StackInfo;
    use std::collections::HashMap;

    fn stack_info(name: &str, parents: Vec<&str>) -> StackInfo {
        StackInfo {
            name: name.to_string(),
            parent_chain: parents.into_iter().map(|s| s.to_string()).collect(),
            tools: HashMap::new(),
        }
    }

    #[test]
    fn resolves_stack_specific_file() {
        let dir = tempfile::tempdir().unwrap();
        let fn_dir = dir.path().join("fix-lint");
        std::fs::create_dir_all(&fn_dir).unwrap();
        let expected = fn_dir.join("rust.md.tera");
        std::fs::write(&expected, "rust prompt").unwrap();

        let info = stack_info("rust", vec![]);
        let resolved = PromptResolver::resolve("fix-lint", &info, dir.path()).unwrap();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn falls_back_to_parent() {
        let dir = tempfile::tempdir().unwrap();
        let fn_dir = dir.path().join("fix-lint");
        std::fs::create_dir_all(&fn_dir).unwrap();
        let parent_file = fn_dir.join("base.md.tera");
        std::fs::write(&parent_file, "base prompt").unwrap();

        // No rust.md.tera, has base.md.tera
        let info = stack_info("rust", vec!["base"]);
        let resolved = PromptResolver::resolve("fix-lint", &info, dir.path()).unwrap();
        assert_eq!(resolved, parent_file);
    }

    #[test]
    fn falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let fn_dir = dir.path().join("fix-lint");
        std::fs::create_dir_all(&fn_dir).unwrap();
        let default_file = fn_dir.join("_default.md.tera");
        std::fs::write(&default_file, "default prompt").unwrap();

        // No rust.md.tera, no base.md.tera, has _default.md.tera
        let info = stack_info("rust", vec!["base"]);
        let resolved = PromptResolver::resolve("fix-lint", &info, dir.path()).unwrap();
        assert_eq!(resolved, default_file);
    }

    #[test]
    fn returns_error_when_no_file_found() {
        let dir = tempfile::tempdir().unwrap();
        let info = stack_info("rust", vec![]);
        let err = PromptResolver::resolve("fix-lint", &info, dir.path()).unwrap_err();
        assert!(err.to_string().contains("No prompt for fix-lint/rust"));
        assert!(err.to_string().contains("_default.md.tera"));
    }

    #[test]
    fn prefers_stack_over_parent_when_both_exist() {
        let dir = tempfile::tempdir().unwrap();
        let fn_dir = dir.path().join("fix-lint");
        std::fs::create_dir_all(&fn_dir).unwrap();
        std::fs::write(fn_dir.join("rust.md.tera"), "rust prompt").unwrap();
        std::fs::write(fn_dir.join("base.md.tera"), "base prompt").unwrap();

        let info = stack_info("rust", vec!["base"]);
        let resolved = PromptResolver::resolve("fix-lint", &info, dir.path()).unwrap();
        assert_eq!(resolved, fn_dir.join("rust.md.tera"));
    }
}
