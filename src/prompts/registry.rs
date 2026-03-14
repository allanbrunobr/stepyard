use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::StepError;

#[derive(Debug, Clone, Deserialize)]
pub struct StackDef {
    pub parent: Option<String>,
    #[serde(default)]
    pub file_markers: Vec<String>,
    #[serde(default)]
    pub content_match: HashMap<String, String>,
    #[serde(default)]
    pub tools: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub detection_order: Vec<String>,
    pub stacks: HashMap<String, StackDef>,
}

impl Registry {
    pub fn from_file(path: &Path) -> Result<Self, StepError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| StepError::Fail(format!("Failed to read registry: {e}")))?;
        let registry: Registry = serde_yaml::from_str(&content).map_err(|e| {
            StepError::config("registry.yaml", format!("Invalid registry.yaml: {e}"))
        })?;
        Ok(registry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn parse_minimal_registry() {
        let yaml = r#"
version: 1
detection_order:
  - rust
stacks:
  rust:
    file_markers:
      - Cargo.toml
    tools:
      lint: "cargo clippy"
      test: "cargo test"
"#;
        let registry: Registry = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(registry.version, 1);
        assert_eq!(registry.detection_order, vec!["rust"]);
        let rust = registry.stacks.get("rust").unwrap();
        assert!(rust.file_markers.contains(&"Cargo.toml".to_string()));
        assert_eq!(rust.tools.get("lint").unwrap(), "cargo clippy");
    }

    #[test]
    fn from_file_reads_and_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "version: 1\ndetection_order:\n  - node\nstacks:\n  node:\n    file_markers:\n      - package.json\n"
        )
        .unwrap();
        let registry = Registry::from_file(&path).unwrap();
        assert_eq!(registry.detection_order, vec!["node"]);
    }

    #[test]
    fn from_file_missing_returns_error() {
        let err =
            Registry::from_file(std::path::Path::new("/nonexistent/registry.yaml")).unwrap_err();
        assert!(err.to_string().contains("Failed to read registry"));
    }
}
