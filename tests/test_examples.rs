//! Integration tests for examples to ensure they remain functional

use std::process::Command;

#[test]
fn test_rust_sample_example_runs() {
    let output = Command::new("cargo")
        .args(["run", "--example", "rust_sample"])
        .output()
        .expect("Failed to run rust_sample example");

    assert!(output.status.success(),
        "rust_sample example failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Verify key outputs are present
    assert!(stdout.contains("Average Calculation Examples"));
    assert!(stdout.contains("Is NaN: true"));
    assert!(stdout.contains("Alternative Implementations"));
}

#[test]
fn test_rust_sample_example_tests() {
    let output = Command::new("cargo")
        .args(["test", "--example", "rust_sample"])
        .output()
        .expect("Failed to test rust_sample example");

    assert!(output.status.success(),
        "rust_sample example tests failed with stderr: {}",
        String::from_utf8_lossy(&output.stderr));
}