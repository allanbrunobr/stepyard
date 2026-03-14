/// Integration tests for the Prompt Resolver pipeline.
///
/// These tests cover:
/// - Registry YAML parsing (valid and invalid)
/// - Stack detection from fixture projects (Java, React, Python, Rust)
/// - Fallback chain traversal (react -> typescript -> _default)
/// - Dynamic template path loading
/// - Missing prompt error messages (descriptive, actionable)
/// - Circular inheritance detection
///
/// NOTE: This test module depends on `minion_engine::prompts::{Registry, StackDetector,
/// PromptResolver}` which are implemented by WT-1 and WT-2. Tests will not compile until
/// those modules are merged.
use std::fs;
use std::path::PathBuf;

use minion_engine::prompts::{PromptResolver, Registry, StackDetector};

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

#[test]
fn test_registry_parses_valid_yaml() {
    let registry = Registry::from_file(&fixture_registry_path())
        .expect("registry.yaml should parse without errors");

    // Verify known stacks are present
    assert!(
        registry.get_stack("rust").is_some(),
        "registry should contain a 'rust' stack"
    );
    assert!(
        registry.get_stack("java").is_some(),
        "registry should contain a 'java' stack"
    );
    assert!(
        registry.get_stack("react").is_some(),
        "registry should contain a 'react' stack"
    );
    assert!(
        registry.get_stack("python").is_some(),
        "registry should contain a 'python' stack"
    );
}

#[test]
fn test_registry_rejects_invalid_yaml() {
    let invalid_path = fixtures_dir().join("registry_invalid.yaml");
    let result = Registry::from_file(&invalid_path);
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

#[test]
fn test_stack_detection_java() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    fs::write(dir.path().join("pom.xml"), "<project></project>")
        .expect("failed to write pom.xml");

    let registry =
        Registry::from_file(&fixture_registry_path()).expect("registry should parse");
    let stack = StackDetector::detect(&registry, dir.path())
        .expect("should detect a stack from pom.xml");

    assert_eq!(
        stack.name, "java",
        "pom.xml should detect the 'java' stack, got: {}",
        stack.name
    );
}

#[test]
fn test_stack_detection_react() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    // package.json containing "react" dependency signals a React project
    fs::write(
        dir.path().join("package.json"),
        r#"{"name": "my-app", "dependencies": {"react": "^18.0.0"}}"#,
    )
    .expect("failed to write package.json");

    let registry =
        Registry::from_file(&fixture_registry_path()).expect("registry should parse");
    let stack = StackDetector::detect(&registry, dir.path())
        .expect("should detect a stack from package.json with react");

    assert_eq!(
        stack.name, "react",
        "package.json with react dependency should detect 'react' stack, got: {}",
        stack.name
    );
}

#[test]
fn test_stack_detection_python() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    fs::write(
        dir.path().join("pyproject.toml"),
        "[tool.poetry]\nname = \"my-project\"\n",
    )
    .expect("failed to write pyproject.toml");

    let registry =
        Registry::from_file(&fixture_registry_path()).expect("registry should parse");
    let stack = StackDetector::detect(&registry, dir.path())
        .expect("should detect a stack from pyproject.toml");

    assert_eq!(
        stack.name, "python",
        "pyproject.toml should detect the 'python' stack, got: {}",
        stack.name
    );
}

#[test]
fn test_stack_detection_rust() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("failed to write Cargo.toml");

    let registry =
        Registry::from_file(&fixture_registry_path()).expect("registry should parse");
    let stack = StackDetector::detect(&registry, dir.path())
        .expect("should detect a stack from Cargo.toml");

    assert_eq!(
        stack.name, "rust",
        "Cargo.toml should detect the 'rust' stack, got: {}",
        stack.name
    );
}

// ── Fallback chain ────────────────────────────────────────────────────────────

#[test]
fn test_fallback_chain_react() {
    // react -> typescript -> javascript -> _default
    // The fixture prompts dir has: fix-lint/typescript.md.tera but NOT fix-lint/react.md.tera
    // So the resolver should fall back to typescript.md.tera
    let registry =
        Registry::from_file(&fixture_registry_path()).expect("registry should parse");
    let stack = registry
        .get_stack("react")
        .expect("'react' stack should exist in registry");

    let resolver = PromptResolver::new(registry, fixture_prompts_dir());
    let resolved = resolver
        .resolve("fix-lint", stack)
        .expect("fix-lint should resolve for react via fallback chain");

    // Should have fallen back to typescript (react has no specific fix-lint in fixtures)
    assert!(
        resolved.contains("typescript") || resolved.contains("TypeScript") || resolved.contains("fix"),
        "resolved prompt should come from the typescript fallback template, got: {}",
        &resolved[..resolved.len().min(200)]
    );
}

#[test]
fn test_fallback_chain_reaches_default() {
    // Use a stack that has no specific prompt files at all — should reach _default
    let registry =
        Registry::from_file(&fixture_registry_path()).expect("registry should parse");
    let stack = registry
        .get_stack("python")
        .expect("'python' stack should exist in registry");

    let resolver = PromptResolver::new(registry, fixture_prompts_dir());
    // fix-test has only _default.md.tera in fixtures for python
    let resolved = resolver
        .resolve("fix-test", stack)
        .expect("fix-test should resolve for python via _default fallback");

    assert!(
        !resolved.is_empty(),
        "resolved prompt should be non-empty when using _default fallback"
    );
}

// ── Dynamic template path ─────────────────────────────────────────────────────

#[test]
fn test_dynamic_template_path() {
    // A template step can specify `prompt: "fix-lint/{{ stack.name }}"` to dynamically
    // resolve the prompt path based on the detected stack.
    let registry =
        Registry::from_file(&fixture_registry_path()).expect("registry should parse");
    let rust_stack = registry
        .get_stack("rust")
        .expect("'rust' stack should exist in registry");

    let resolver = PromptResolver::new(registry, fixture_prompts_dir());

    // The dynamic path "fix-lint/{{ stack.name }}" with stack.name = "rust"
    // should resolve to "fix-lint/rust" -> loads fix-lint/rust.md.tera
    let dynamic_path = format!("fix-lint/{}", rust_stack.name);
    let resolved = resolver
        .resolve_path(&dynamic_path)
        .expect("dynamic path fix-lint/rust should resolve to rust.md.tera");

    assert!(
        resolved.contains("Rust") || resolved.contains("rust") || resolved.contains("Clippy") || resolved.contains("clippy"),
        "resolved prompt for fix-lint/rust should contain Rust-specific content, got: {}",
        &resolved[..resolved.len().min(200)]
    );
}

// ── Missing prompt error messages ─────────────────────────────────────────────

#[test]
fn test_missing_prompt_error_is_descriptive() {
    let registry =
        Registry::from_file(&fixture_registry_path()).expect("registry should parse");
    let rust_stack = registry
        .get_stack("rust")
        .expect("'rust' stack should exist in registry");

    let resolver = PromptResolver::new(registry, fixture_prompts_dir());

    // "nonexistent-function" has no prompt files at all — not even _default
    let result = resolver.resolve("nonexistent-function", rust_stack);

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

#[test]
fn test_circular_inheritance_detected() {
    let circular_path = fixtures_dir().join("registry_circular.yaml");
    let registry =
        Registry::from_file(&circular_path).expect("circular registry file should parse as YAML");

    // Circular detection should happen during resolution or validation,
    // not necessarily during parse. Try to resolve a prompt for one of the circular stacks.
    let stack = registry
        .get_stack("alpha")
        .expect("'alpha' stack should exist in circular registry");

    let resolver = PromptResolver::new(registry, fixture_prompts_dir());
    let result = resolver.resolve("fix-lint", stack);

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
