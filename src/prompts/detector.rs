use std::collections::HashMap;
use std::path::Path;

use crate::error::StepError;
use crate::prompts::registry::{Registry, StackDef};

#[derive(Debug, Clone)]
pub struct StackInfo {
    pub name: String,
    pub parent_chain: Vec<String>,
    pub tools: HashMap<String, String>,
}

pub struct StackDetector;

impl StackDetector {
    /// Detect the technology stack for the given workspace path.
    ///
    /// Follows `detection_order` in the registry (most specific first).
    /// Returns the first fully matching stack as a [`StackInfo`].
    pub async fn detect(
        registry: &Registry,
        workspace_path: &Path,
    ) -> Result<StackInfo, StepError> {
        let mut checked_markers: Vec<String> = Vec::new();

        for stack_name in &registry.detection_order {
            let stack_def = match registry.stacks.get(stack_name) {
                Some(def) => def,
                None => continue,
            };

            // Skip stacks with neither file_markers nor content_match (e.g. _default)
            if stack_def.file_markers.is_empty() && stack_def.content_match.is_empty() {
                continue;
            }

            // Check file_markers: any ONE matching is sufficient
            if !stack_def.file_markers.is_empty() {
                let mut any_marker_found = false;
                for marker in &stack_def.file_markers {
                    checked_markers.push(marker.clone());
                    if tokio::fs::metadata(workspace_path.join(marker))
                        .await
                        .is_ok()
                    {
                        any_marker_found = true;
                        break;
                    }
                }
                if !any_marker_found {
                    continue;
                }
            }

            // Check content_match: ALL entries must match
            if !Self::content_matches(stack_def, workspace_path).await {
                continue;
            }

            // This stack matches — build and return StackInfo
            return Ok(Self::build_stack_info(stack_name, registry));
        }

        let markers_list = checked_markers.join(", ");
        Err(StepError::Fail(format!(
            "Could not detect project stack in '{}'. Checked markers: [{}]. \
             Create prompts/registry.yaml with your stack definition.",
            workspace_path.display(),
            markers_list
        )))
    }

    /// Returns true if ALL content_match patterns satisfy the workspace files.
    async fn content_matches(stack_def: &StackDef, workspace_path: &Path) -> bool {
        for (filename, pattern) in &stack_def.content_match {
            match tokio::fs::read_to_string(workspace_path.join(filename)).await {
                Ok(content) if content.contains(pattern.as_str()) => {}
                _ => return false,
            }
        }
        true
    }

    /// Build a [`StackInfo`] by walking the parent chain and merging tools.
    fn build_stack_info(name: &str, registry: &Registry) -> StackInfo {
        // Walk parent chain from child to root
        let mut parent_chain: Vec<String> = Vec::new();
        let mut current = registry.stacks.get(name).and_then(|s| s.parent.as_deref());
        while let Some(parent_name) = current {
            parent_chain.push(parent_name.to_string());
            current = registry
                .stacks
                .get(parent_name)
                .and_then(|s| s.parent.as_deref());
        }

        // Merge tools root-first so child overrides parent
        let mut full_chain: Vec<&str> = vec![name];
        full_chain.extend(parent_chain.iter().map(String::as_str));
        full_chain.reverse(); // root -> child

        let mut tools: HashMap<String, String> = HashMap::new();
        for stack_name in &full_chain {
            if let Some(stack_def) = registry.stacks.get(*stack_name) {
                tools.extend(stack_def.tools.clone());
            }
        }

        StackInfo {
            name: name.to_string(),
            parent_chain,
            tools,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::registry::{Registry, StackDef};
    use std::io::Write as _;
    use tempfile::tempdir;

    fn make_registry() -> Registry {
        let mut stacks = HashMap::new();

        stacks.insert(
            "_default".to_string(),
            StackDef {
                parent: None,
                file_markers: vec![],
                content_match: HashMap::new(),
                tools: {
                    let mut t = HashMap::new();
                    t.insert("lint".to_string(), "echo 'no linter'".to_string());
                    t.insert("test".to_string(), "echo 'no test'".to_string());
                    t.insert("build".to_string(), "echo 'no build'".to_string());
                    t
                },
            },
        );

        stacks.insert(
            "rust".to_string(),
            StackDef {
                parent: Some("_default".to_string()),
                file_markers: vec!["Cargo.toml".to_string()],
                content_match: HashMap::new(),
                tools: {
                    let mut t = HashMap::new();
                    t.insert(
                        "lint".to_string(),
                        "cargo clippy -- -D warnings".to_string(),
                    );
                    t.insert("test".to_string(), "cargo test".to_string());
                    t.insert("build".to_string(), "cargo build --release".to_string());
                    t
                },
            },
        );

        stacks.insert(
            "java".to_string(),
            StackDef {
                parent: Some("_default".to_string()),
                file_markers: vec!["pom.xml".to_string(), "build.gradle".to_string()],
                content_match: HashMap::new(),
                tools: {
                    let mut t = HashMap::new();
                    t.insert("test".to_string(), "mvn test".to_string());
                    t.insert("build".to_string(), "mvn package -DskipTests".to_string());
                    t
                },
            },
        );

        stacks.insert(
            "java-spring".to_string(),
            StackDef {
                parent: Some("java".to_string()),
                file_markers: vec!["pom.xml".to_string()],
                content_match: {
                    let mut m = HashMap::new();
                    m.insert("pom.xml".to_string(), "spring-boot".to_string());
                    m
                },
                tools: {
                    let mut t = HashMap::new();
                    t.insert(
                        "test".to_string(),
                        "mvn test -Dspring.profiles.active=test".to_string(),
                    );
                    t
                },
            },
        );

        stacks.insert(
            "javascript".to_string(),
            StackDef {
                parent: Some("_default".to_string()),
                file_markers: vec!["package.json".to_string()],
                content_match: HashMap::new(),
                tools: {
                    let mut t = HashMap::new();
                    t.insert("test".to_string(), "npm test".to_string());
                    t
                },
            },
        );

        Registry {
            version: 1,
            detection_order: vec![
                "java-spring".to_string(),
                "java".to_string(),
                "javascript".to_string(),
                "rust".to_string(),
            ],
            stacks,
        }
    }

    #[tokio::test]
    async fn test_detect_rust_project() {
        let dir = tempdir().unwrap();
        std::fs::File::create(dir.path().join("Cargo.toml")).unwrap();

        let registry = make_registry();
        let result = StackDetector::detect(&registry, dir.path()).await.unwrap();

        assert_eq!(result.name, "rust");
        assert_eq!(result.parent_chain, vec!["_default"]);
    }

    #[tokio::test]
    async fn test_detect_java_spring_project() {
        let dir = tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("pom.xml")).unwrap();
        f.write_all(b"<project><parent><artifactId>spring-boot-starter-parent</artifactId></parent></project>")
            .unwrap();

        let registry = make_registry();
        let result = StackDetector::detect(&registry, dir.path()).await.unwrap();

        assert_eq!(result.name, "java-spring");
    }

    #[tokio::test]
    async fn test_detection_order_java_spring_before_java() {
        let dir = tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("pom.xml")).unwrap();
        f.write_all(b"<project>spring-boot</project>").unwrap();

        let registry = make_registry();
        let result = StackDetector::detect(&registry, dir.path()).await.unwrap();

        // java-spring comes before java in detection_order, should be detected first
        assert_eq!(result.name, "java-spring");
    }

    #[tokio::test]
    async fn test_content_match_failure_falls_through_to_less_specific_stack() {
        let dir = tempdir().unwrap();
        // pom.xml exists but does NOT contain "spring-boot" -> java-spring fails, java matches
        let mut f = std::fs::File::create(dir.path().join("pom.xml")).unwrap();
        f.write_all(b"<project><groupId>com.example</groupId></project>")
            .unwrap();

        let registry = make_registry();
        let result = StackDetector::detect(&registry, dir.path()).await.unwrap();

        assert_eq!(result.name, "java");
    }

    #[tokio::test]
    async fn test_no_stack_detected_returns_step_error_fail() {
        let dir = tempdir().unwrap();
        // Empty directory — no markers

        let registry = make_registry();
        let result = StackDetector::detect(&registry, dir.path()).await;

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Could not detect project stack"),
            "Expected error message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_parent_chain_and_tool_merging_for_rust() {
        let dir = tempdir().unwrap();
        std::fs::File::create(dir.path().join("Cargo.toml")).unwrap();

        let registry = make_registry();
        let result = StackDetector::detect(&registry, dir.path()).await.unwrap();

        assert_eq!(result.parent_chain, vec!["_default"]);
        // Rust tools should override _default tools
        assert_eq!(result.tools.get("test").unwrap(), "cargo test");
        assert_eq!(
            result.tools.get("lint").unwrap(),
            "cargo clippy -- -D warnings"
        );
        assert_eq!(result.tools.get("build").unwrap(), "cargo build --release");
        // "build" key was in _default too but rust overrides it
    }

    #[tokio::test]
    async fn test_java_spring_parent_chain() {
        let dir = tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("pom.xml")).unwrap();
        f.write_all(b"spring-boot").unwrap();

        let registry = make_registry();
        let result = StackDetector::detect(&registry, dir.path()).await.unwrap();

        // java-spring -> java -> _default
        assert_eq!(result.parent_chain, vec!["java", "_default"]);
        // java-spring test overrides java's test
        assert_eq!(
            result.tools.get("test").unwrap(),
            "mvn test -Dspring.profiles.active=test"
        );
    }
}
