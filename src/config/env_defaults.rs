//! `.stepyard/defaults.yaml` env/vars loader (Story 3.3, FR10; Story 5.4, FR19).
//!
//! This module owns ONLY the env-vars and template-vars layer of the project-root defaults
//! file. The existing [`crate::config::defaults`] module handles the
//! agent/chat/global `WorkflowConfig` layers — different concern, different
//! type, intentionally kept separate so neither leaks into the other.
//!
//! # Contract
//!
//! * Missing file → `Ok(Defaults::default())` (NOT an error; the file is
//!   optional).
//! * Malformed YAML → `Err(DefaultsError::Parse { path, source })`.
//! * I/O error (permission denied, etc.) → `Err(DefaultsError::Io { path, source })`.
//!
//! The cascade resolver in Story 3.4 will overlay these values below
//! workflow-level and step-level env and above host `${VAR}` substitution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The parsed shape of `.stepyard/defaults.yaml` for env injection and
/// workflow-template variables.
///
/// Only the `env:` and `vars:` fields are consumed by this loader. Other
/// fields in the same file are tolerated because `serde_yaml` ignores
/// unknown keys by default.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Defaults {
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub vars: HashMap<String, String>,
}

/// Errors produced by [`load_defaults`]. Uses `thiserror` per NFR21 — this
/// is library code, not binary glue.
#[derive(Debug, thiserror::Error)]
pub enum DefaultsError {
    #[error("failed to read defaults file at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse defaults file at {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
}

/// Load `.stepyard/defaults.yaml` env defaults from `path`.
///
/// Returns [`Defaults::default()`] when `path` does not exist — a missing
/// file is not an error because defaults are opt-in per project (AC).
pub fn load_defaults(path: &Path) -> Result<Defaults, DefaultsError> {
    if !path.exists() {
        return Ok(Defaults::default());
    }
    let contents = std::fs::read_to_string(path).map_err(|source| DefaultsError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed: Defaults =
        serde_yaml::from_str(&contents).map_err(|source| DefaultsError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_fixture(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).expect("create fixture");
        f.write_all(body.as_bytes()).expect("write fixture");
        path
    }

    #[test]
    fn load_defaults_yaml_returns_env_pairs() {
        let dir = TempDir::new().expect("tempdir");
        let path = write_fixture(&dir, "defaults.yaml", "env:\n  FOO: bar\n  BAZ: qux\n");
        let defaults = load_defaults(&path).expect("load");
        let mut expected = HashMap::new();
        expected.insert("FOO".to_string(), "bar".to_string());
        expected.insert("BAZ".to_string(), "qux".to_string());
        assert_eq!(defaults.env, expected);
        assert!(defaults.vars.is_empty());
    }

    #[test]
    fn load_defaults_yaml_returns_template_vars() {
        let dir = TempDir::new().expect("tempdir");
        let path = write_fixture(
            &dir,
            "defaults.yaml",
            "vars:\n  PROJECT: stepyard\n  MSG: hello\n",
        );
        let defaults = load_defaults(&path).expect("load");
        let mut expected = HashMap::new();
        expected.insert("PROJECT".to_string(), "stepyard".to_string());
        expected.insert("MSG".to_string(), "hello".to_string());
        assert_eq!(defaults.vars, expected);
        assert!(defaults.env.is_empty());
    }

    #[test]
    fn missing_file_returns_empty_defaults() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("does-not-exist.yaml");
        let defaults = load_defaults(&path).expect("missing → Ok");
        assert_eq!(defaults, Defaults::default());
        assert!(defaults.env.is_empty());
    }

    #[test]
    fn malformed_yaml_returns_parse_error_with_path() {
        let dir = TempDir::new().expect("tempdir");
        // `env:` with a scalar instead of a mapping trips serde_yaml.
        let path = write_fixture(&dir, "broken.yaml", "env: not-a-mapping\n");
        let err = load_defaults(&path).expect_err("malformed → Err");
        match err {
            DefaultsError::Parse { path: p, .. } => assert_eq!(p, path),
            other => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn empty_env_key_still_parses() {
        // A defaults file with NO env field → empty map, not an error.
        let dir = TempDir::new().expect("tempdir");
        let path = write_fixture(&dir, "no-env.yaml", "other: field\n");
        let defaults = load_defaults(&path).expect("no env field → Ok");
        assert!(defaults.env.is_empty());
    }
}
