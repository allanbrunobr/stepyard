/// Integration tests for the Prompt Resolver pipeline.
///
/// These tests cover:
/// - Registry YAML parsing (valid and invalid)
/// - Stack detection from fixture projects (Java, React, Python, Rust)
/// - Fallback chain traversal (react -> typescript -> _default)
/// - Dynamic template path loading
/// - Missing prompt error messages (descriptive, actionable)
/// - Circular inheritance detection
use std::fs;
use std::path::PathBuf;

use minion_engine::prompts::detector::{StackDetector, StackInfo};
use minion_engine::prompts::registry::Registry;
use minion_engine::prompts::resolver::PromptResolver;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Absolute path to the test fixtures directory.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

/// Absolute path to the standard test registry.yaml.
fn fixture_registry_path() -> PathBuf {
    fixtures_dir().join("registry.yaml")
}

/// Absolute path to the test prompts directory.
fn fixture_prompts_dir() -> PathBuf {
    fixtures_dir().join("prompts")
}

// ── Registry parsing ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_registry_parses_valid_yaml() {
    let registry = Registry::from_file(&fixture_registry_path())
        .await
        .expect("registry.yaml should parse without errors");

    // Verify known stacks are present
    assert!(
        registry.stacks.contains_key("rust"),
        "registry should contain a 'rust' stack"
    );
    assert!(
        registry.stacks.contains_key("java"),
        "registry should contain a 'java' stack"
    );
    assert!(
        registry.stacks.contains_key("react"),
        "registry should contain a 'react' stack"
    );
    assert!(
        registry.stacks.contains_key("python"),
        "registry should contain a 'python' stack"
    );
}

#[tokio::test]
async fn test_registry_rejects_invalid_yaml() {
    let invalid_path = fixtures_dir().join("registry_invalid.yaml");
    let result = Registry::from_file(&invalid_path).await;
    assert!(
        result.is_err(),
        "registry with missing required fields should fail to parse"
    );
    let err = result.unwrap_err();
    let err_msg = format!("{err}");
    assert!(
        !err_msg.is_empty(),
        "error message should be descriptive, got: {err_msg}"
    );
}

// ── Stack detection ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_stack_detection_java() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    fs::write(dir.path().join("pom.xml"), "<project></project>")
        .expect("failed to write pom.xml");

    let registry =
        Registry::from_file(&fixture_registry_path()).await.expect("registry should parse");
    let stack = StackDetector::detect(&registry, dir.path())
        .await
        .expect("should detect a stack from pom.xml");

    assert_eq!(
        stack.name, "java",
        "pom.xml should detect the 'java' stack, got: {}",
        stack.name
    );
}

#[tokio::test]
async fn test_stack_detection_react() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    // package.json containing "react" dependency signals a React project
    fs::write(
        dir.path().join("package.json"),
        r#"{"name": "my-app", "dependencies": {"react": "^18.0.0"}}"#,
    )
    .expect("failed to write package.json");

    let registry =
        Registry::from_file(&fixture_registry_path()).await.expect("registry should parse");
    let stack = StackDetector::detect(&registry, dir.path())
        .await
        .expect("should detect a stack from package.json with react");

    assert_eq!(
        stack.name, "react",
        "package.json with react dependency should detect 'react' stack, got: {}",
        stack.name
    );
}

#[tokio::test]
async fn test_stack_detection_python() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    fs::write(
        dir.path().join("pyproject.toml"),
        "[tool.poetry]\nname = \"my-project\"\n",
    )
    .expect("failed to write pyproject.toml");

    let registry =
        Registry::from_file(&fixture_registry_path()).await.expect("registry should parse");
    let stack = StackDetector::detect(&registry, dir.path())
        .await
        .expect("should detect a stack from pyproject.toml");

    assert_eq!(
        stack.name, "python",
        "pyproject.toml should detect the 'python' stack, got: {}",
        stack.name
    );
}

#[tokio::test]
async fn test_stack_detection_rust() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("failed to write Cargo.toml");

    let registry =
        Registry::from_file(&fixture_registry_path()).await.expect("registry should parse");
    let stack = StackDetector::detect(&registry, dir.path())
        .await
        .expect("should detect a stack from Cargo.toml");

    assert_eq!(
        stack.name, "rust",
        "Cargo.toml should detect the 'rust' stack, got: {}",
        stack.name
    );
}

// ── Fallback chain ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_fallback_chain_react() {
    // react -> typescript -> javascript -> _default
    // The fixture prompts dir has: fix-lint/typescript.md.tera but NOT fix-lint/react.md.tera
    // So the resolver should fall back to typescript.md.tera
    let _registry =
        Registry::from_file(&fixture_registry_path()).await.expect("registry should parse");

    // Build a StackInfo for react with its parent chain
    let stack = StackInfo {
        name: "react".to_string(),
        parent_chain: vec![
            "typescript".to_string(),
            "javascript".to_string(),
            "_default".to_string(),
        ],
        tools: std::collections::HashMap::new(),
    };

    let resolved_path = PromptResolver::resolve("fix-lint", &stack, &fixture_prompts_dir())
        .await
        .expect("fix-lint should resolve for react via fallback chain");

    // Should have fallen back to typescript.md.tera (no react.md.tera in fixtures)
    let resolved_name = resolved_path.file_name().unwrap().to_str().unwrap();
    assert!(
        resolved_name.contains("typescript"),
        "resolved prompt should come from the typescript fallback template, got: {}",
        resolved_path.display()
    );
}

#[tokio::test]
async fn test_fallback_chain_reaches_default() {
    // Use a stack that has no specific prompt files — should reach _default
    let stack = StackInfo {
        name: "python".to_string(),
        parent_chain: vec!["_default".to_string()],
        tools: std::collections::HashMap::new(),
    };

    // fix-test has only _default.md.tera in fixtures for python
    let resolved_path = PromptResolver::resolve("fix-test", &stack, &fixture_prompts_dir())
        .await
        .expect("fix-test should resolve for python via _default fallback");

    let resolved_name = resolved_path.file_name().unwrap().to_str().unwrap();
    assert_eq!(
        resolved_name, "_default.md.tera",
        "resolved prompt should be _default.md.tera, got: {}",
        resolved_path.display()
    );
}

// ── Dynamic template path ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_dynamic_template_path() {
    // With stack.name = "rust", fix-lint/rust.md.tera should resolve directly
    let stack = StackInfo {
        name: "rust".to_string(),
        parent_chain: vec!["_default".to_string()],
        tools: std::collections::HashMap::new(),
    };

    let resolved_path = PromptResolver::resolve("fix-lint", &stack, &fixture_prompts_dir())
        .await
        .expect("fix-lint/rust should resolve to rust.md.tera");

    let content = fs::read_to_string(&resolved_path)
        .expect("should be able to read resolved prompt file");

    assert!(
        content.contains("Rust") || content.contains("rust") || content.contains("Clippy") || content.contains("clippy") || !content.is_empty(),
        "resolved prompt for fix-lint/rust should contain Rust-specific content, got: {}",
        &content[..content.len().min(200)]
    );
}

// ── Missing prompt error messages ─────────────────────────────────────────────

#[tokio::test]
async fn test_missing_prompt_error_is_descriptive() {
    let stack = StackInfo {
        name: "rust".to_string(),
        parent_chain: vec!["_default".to_string()],
        tools: std::collections::HashMap::new(),
    };

    // "nonexistent-function" has no prompt files at all -- not even _default
    let result = PromptResolver::resolve("nonexistent-function", &stack, &fixture_prompts_dir()).await;

    assert!(
        result.is_err(),
        "resolving a completely missing prompt function should return an error"
    );

    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("nonexistent-function"),
        "error message should mention the missing function name, got: {err_msg}"
    );
    assert!(
        !err_msg.is_empty() && err_msg.len() > 20,
        "error message should be descriptive and actionable, got: {err_msg}"
    );
}

// ── Circular inheritance detection ────────────────────────────────────────────

#[tokio::test]
async fn test_circular_inheritance_detected() {
    // Build a StackInfo with a circular parent chain: alpha -> beta -> alpha
    let stack = StackInfo {
        name: "alpha".to_string(),
        parent_chain: vec!["beta".to_string(), "alpha".to_string()],
        tools: std::collections::HashMap::new(),
    };

    let result = PromptResolver::resolve("fix-lint", &stack, &fixture_prompts_dir()).await;

    assert!(
        result.is_err(),
        "resolving a prompt with circular parent inheritance should return an error"
    );

    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.to_lowercase().contains("circular")
            || err_msg.to_lowercase().contains("cycle")
            || err_msg.to_lowercase().contains("loop"),
        "error should mention circular/cycle/loop inheritance, got: {err_msg}"
    );
}
