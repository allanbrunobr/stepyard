use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::StepError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Registry {
    pub version: u32,
    pub detection_order: Vec<String>,
    pub stacks: HashMap<String, StackDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub content_match: HashMap<String, String>,
    #[serde(default)]
    pub tools: HashMap<String, String>,
}

impl Registry {
    /// Load and parse a registry YAML file.
    pub async fn from_file(path: &Path) -> Result<Registry, StepError> {
        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StepError::Fail(format!(
                    "Registry file not found: '{}': No such file",
                    path.display()
                ))
            } else {
                StepError::Fail(format!(
                    "Failed to read registry file '{}': {}",
                    path.display(),
                    e
                ))
            }
        })?;

        let registry: Registry = serde_yaml::from_str(&content).map_err(|e| StepError::Config {
            field: "registry.yaml".into(),
            message: e.to_string(),
        })?;

        Ok(registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    const MINIMAL_YAML: &str = r#"
version: 1
detection_order:
  - rust
  - python

stacks:
  _default:
    tools:
      lint: "echo 'no linter configured'"
      test: "echo 'no test runner configured'"
      build: "echo 'no build configured'"
      install: "echo 'no installer configured'"

  rust:
    parent: _default
    file_markers: ["Cargo.toml"]
    tools:
      lint: "cargo clippy -- -D warnings"
      test: "cargo test"
      build: "cargo build --release"
      install: "cargo fetch"

  python:
    parent: _default
    file_markers: ["pyproject.toml", "setup.py", "requirements.txt"]
    tools:
      lint: "ruff check ."
      test: "pytest"
      build: "python -m build"
      install: "pip install -r requirements.txt"
"#;

    #[tokio::test]
    async fn test_successful_yaml_parsing() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(MINIMAL_YAML.as_bytes()).unwrap();

        let registry = Registry::from_file(tmp.path()).await.unwrap();

        assert_eq!(registry.version, 1);
        assert_eq!(registry.detection_order, vec!["rust", "python"]);
        assert!(registry.stacks.contains_key("_default"));
        assert!(registry.stacks.contains_key("rust"));
        assert!(registry.stacks.contains_key("python"));
    }

    #[tokio::test]
    async fn test_stackdef_fields_deserialize_correctly() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(MINIMAL_YAML.as_bytes()).unwrap();

        let registry = Registry::from_file(tmp.path()).await.unwrap();

        let rust_stack = registry.stacks.get("rust").unwrap();
        assert_eq!(rust_stack.parent, Some("_default".to_string()));
        assert_eq!(rust_stack.file_markers, vec!["Cargo.toml"]);
        assert!(rust_stack.content_match.is_empty());
        assert_eq!(rust_stack.tools.get("test").unwrap(), "cargo test");
        assert_eq!(
            rust_stack.tools.get("lint").unwrap(),
            "cargo clippy -- -D warnings"
        );
    }

    #[tokio::test]
    async fn test_default_stack_has_no_parent_and_no_file_markers() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(MINIMAL_YAML.as_bytes()).unwrap();

        let registry = Registry::from_file(tmp.path()).await.unwrap();

        let default_stack = registry.stacks.get("_default").unwrap();
        assert!(default_stack.parent.is_none());
        assert!(default_stack.file_markers.is_empty());
        assert_eq!(
            default_stack.tools.get("lint").unwrap(),
            "echo 'no linter configured'"
        );
    }

    #[tokio::test]
    async fn test_missing_file_returns_step_error_fail() {
        let path = Path::new("/nonexistent/path/registry.yaml");
        let result = Registry::from_file(path).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Registry file not found"),
            "Expected 'Registry file not found' in: {msg}"
        );
        assert!(
            msg.contains("No such file"),
            "Expected 'No such file' in: {msg}"
        );
    }

    #[tokio::test]
    async fn test_invalid_yaml_returns_step_error_config() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"version: 1\nstacks: [this is not valid yaml mapping: {{{")
            .unwrap();

        let result = Registry::from_file(tmp.path()).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            StepError::Config { field, .. } => {
                assert_eq!(field, "registry.yaml");
            }
            other => panic!("Expected StepError::Config, got: {other:?}"),
        }
    }
}
