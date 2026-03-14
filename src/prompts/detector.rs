use std::collections::HashMap;
use std::path::Path;

use crate::error::StepError;

use super::registry::Registry;

#[derive(Debug, Clone)]
pub struct StackInfo {
    pub name: String,
    pub parent_chain: Vec<String>,
    pub tools: HashMap<String, String>,
}

pub struct StackDetector;

impl StackDetector {
    pub fn detect(registry: &Registry, workspace: &Path) -> Result<StackInfo, StepError> {
        for stack_name in &registry.detection_order {
            if let Some(stack_def) = registry.stacks.get(stack_name) {
                let found = stack_def
                    .file_markers
                    .iter()
                    .any(|marker| workspace.join(marker).exists());

                if found {
                    let mut tools = HashMap::new();
                    let mut parent_chain = Vec::new();

                    // Collect from _default first (lowest priority)
                    if let Some(default_def) = registry.stacks.get("_default") {
                        tools.extend(default_def.tools.clone());
                    }

                    // Walk parent chain (collect names)
                    let mut current_name = stack_name.as_str();
                    loop {
                        if let Some(def) = registry.stacks.get(current_name) {
                            if let Some(ref parent) = def.parent {
                                parent_chain.push(parent.clone());
                                current_name = parent;
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }

                    // Apply tools from parent chain (most general first)
                    for parent_name in parent_chain.iter().rev() {
                        if let Some(parent_def) = registry.stacks.get(parent_name.as_str()) {
                            tools.extend(parent_def.tools.clone());
                        }
                    }

                    // Apply stack's own tools last (highest priority)
                    tools.extend(stack_def.tools.clone());

                    return Ok(StackInfo {
                        name: stack_name.clone(),
                        parent_chain,
                        tools,
                    });
                }
            }
        }

        Err(StepError::Fail("Could not detect project stack".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompts::registry::{Registry, StackDef};
    use std::collections::HashMap;

    fn make_registry(stacks: Vec<(&str, StackDef)>, detection_order: Vec<&str>) -> Registry {
        Registry {
            version: 1,
            detection_order: detection_order.into_iter().map(|s| s.to_string()).collect(),
            stacks: stacks
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    fn rust_def(markers: Vec<&str>, tools: Vec<(&str, &str)>) -> StackDef {
        StackDef {
            parent: None,
            file_markers: markers.into_iter().map(|s| s.to_string()).collect(),
            content_match: HashMap::new(),
            tools: tools
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn detects_rust_stack_by_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let registry = make_registry(
            vec![(
                "rust",
                rust_def(
                    vec!["Cargo.toml"],
                    vec![("lint", "cargo clippy"), ("test", "cargo test")],
                ),
            )],
            vec!["rust"],
        );

        let info = StackDetector::detect(&registry, dir.path()).unwrap();
        assert_eq!(info.name, "rust");
        assert_eq!(info.tools.get("lint").unwrap(), "cargo clippy");
        assert_eq!(info.tools.get("test").unwrap(), "cargo test");
    }

    #[test]
    fn returns_error_when_no_markers_match() {
        let dir = tempfile::tempdir().unwrap();

        let registry = make_registry(
            vec![("rust", rust_def(vec!["Cargo.toml"], vec![]))],
            vec!["rust"],
        );

        let err = StackDetector::detect(&registry, dir.path()).unwrap_err();
        assert!(err.to_string().contains("Could not detect"));
    }

    #[test]
    fn parent_chain_tools_are_inherited() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();

        let base_def = StackDef {
            parent: None,
            file_markers: vec![],
            content_match: HashMap::new(),
            tools: [("lint", "eslint ."), ("build", "npm run build")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let node_def = StackDef {
            parent: Some("base".to_string()),
            file_markers: vec!["package.json".to_string()],
            content_match: HashMap::new(),
            tools: [("test", "npm test")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };

        let registry = make_registry(vec![("base", base_def), ("node", node_def)], vec!["node"]);

        let info = StackDetector::detect(&registry, dir.path()).unwrap();
        assert_eq!(info.name, "node");
        // Inherited from parent
        assert_eq!(info.tools.get("lint").unwrap(), "eslint .");
        assert_eq!(info.tools.get("build").unwrap(), "npm run build");
        // Own tool
        assert_eq!(info.tools.get("test").unwrap(), "npm test");
        assert_eq!(info.parent_chain, vec!["base"]);
    }

    #[test]
    fn default_stack_tools_applied_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let default_def = StackDef {
            parent: None,
            file_markers: vec![],
            content_match: HashMap::new(),
            tools: [("install", "echo install")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let rust_def = StackDef {
            parent: None,
            file_markers: vec!["Cargo.toml".to_string()],
            content_match: HashMap::new(),
            tools: [("test", "cargo test")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };

        let registry = make_registry(
            vec![("_default", default_def), ("rust", rust_def)],
            vec!["rust"],
        );

        let info = StackDetector::detect(&registry, dir.path()).unwrap();
        // _default tool should be present
        assert_eq!(info.tools.get("install").unwrap(), "echo install");
        // Own tool overrides or adds
        assert_eq!(info.tools.get("test").unwrap(), "cargo test");
    }
}
