//! P1-2 (analysis 2026-11-09, SEG-RUST-07) — negative control through a real
//! `sh -c`. Lives in a file named `injection_negative.rs` because
//! `scripts/audit-patterns.sh` (gate G3) reserves that name for tests that
//! build the forbidden `sh -c` shape on purpose.
//!
//! A repository file named `$(touch pwned).py; touch pwned2` — the
//! security-audit `cat {{ scope.value }}` case — must be read as a file name
//! when the template uses `| shell_quote`, and the control proves the same
//! value IS executed without the filter.

use std::collections::HashMap;
use std::process::Command;

use stepyard_harness::{render, RenderContext, ScopeView};

fn render_with_scope(template: &str, scope_value: &str) -> String {
    let steps = HashMap::new();
    let vars = HashMap::new();
    let ctx = RenderContext {
        steps: &steps,
        target: "",
        vars: &vars,
        scope: Some(ScopeView {
            value: serde_json::Value::String(scope_value.into()),
            index: 0,
        }),
    };
    render(template, &ctx).expect("render")
}

fn sh(cmd: &str, dir: &std::path::Path) -> std::process::Output {
    Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .output()
        .expect("run sh")
}

#[test]
fn shell_quoted_file_name_with_command_substitution_is_not_executed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let hostile = "$(touch pwned).py; touch pwned2";
    std::fs::write(dir.path().join(hostile), "content").expect("seed file");

    let out = sh(
        &render_with_scope("cat -- {{ scope.value | shell_quote }}", hostile),
        dir.path(),
    );
    assert!(out.status.success(), "cat failed: {out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "content");
    assert!(!dir.path().join("pwned").exists(), "$(…) was executed");
    assert!(!dir.path().join("pwned2").exists(), "; was executed");

    // Control: the same template WITHOUT the filter does execute it.
    let _ = sh(
        &render_with_scope("cat -- {{ scope.value }}", hostile),
        dir.path(),
    );
    assert!(
        dir.path().join("pwned").exists() && dir.path().join("pwned2").exists(),
        "control: the unquoted form is expected to be injectable"
    );
}
