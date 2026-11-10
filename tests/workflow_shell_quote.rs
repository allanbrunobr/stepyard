//! P1-2 (analysis 2026-11-09, SEG-RUST-02 / SEG-RUST-07) — deterministic gate
//! over the shipped workflows: every `{{ … }}` inside a `run:` command must
//! go through `| shell_quote`, because `run:` is executed with `sh -c` and the
//! values (`target` from the CLI/Slack, file names from the audited
//! repository, previous steps' stdout and LLM responses) are not trusted.
//!
//! Allowed without the filter, because they are not attacker-controlled
//! text: `stack.*` (commands the user configured on purpose, meant to be
//! executed), `scope.index` (an integer) and `steps.extract_issue_id.*` (the
//! output of `grep -oE '[0-9]+$'`, digits only).
//!
//! Heredocs fed from a template are refused outright: a value containing
//! the terminator on its own line ends the heredoc and runs the rest.

use std::path::Path;

fn expressions(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start..].find("}}") else {
            break;
        };
        out.push(rest[start + 2..start + len].trim().to_string());
        rest = &rest[start + len + 2..];
    }
    out
}

fn allowed_unquoted(expr: &str) -> bool {
    expr.starts_with("stack.")
        || expr == "scope.index"
        || expr.starts_with("steps.extract_issue_id.")
}

fn collect_runs(value: &serde_yaml::Value, out: &mut Vec<String>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                if k.as_str() == Some("run") {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                    }
                }
                collect_runs(v, out);
            }
        }
        serde_yaml::Value::Sequence(seq) => seq.iter().for_each(|v| collect_runs(v, out)),
        _ => {}
    }
}

fn violations_in(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).expect("read workflow");
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text).expect("parse workflow");
    let mut runs = Vec::new();
    collect_runs(&yaml, &mut runs);
    let mut bad = Vec::new();
    for run in &runs {
        for expr in expressions(run) {
            let quoted = expr.trim_end().ends_with("| shell_quote");
            if !quoted && !allowed_unquoted(&expr) {
                bad.push(format!(
                    "{}: `{{{{ {expr} }}}}` is not shell-quoted",
                    path.display()
                ));
            }
        }
        let mut in_heredoc: Option<String> = None;
        for line in run.lines() {
            let t = line.trim();
            if let Some(term) = &in_heredoc {
                if t == term {
                    in_heredoc = None;
                } else if t.contains("{{") && !allowed_unquoted(&expressions(t).join(" ")) {
                    bad.push(format!(
                        "{}: template inside a heredoc: {t}",
                        path.display()
                    ));
                }
            } else if let Some(pos) = t.find("<<") {
                let term: String = t[pos + 2..]
                    .trim_start_matches('-')
                    .trim()
                    .trim_matches(|c| c == '\'' || c == '"')
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !term.is_empty() {
                    in_heredoc = Some(term);
                }
            }
        }
    }
    bad
}

#[test]
fn shipped_workflows_shell_quote_every_interpolation_in_run_commands() {
    let mut bad = Vec::new();
    let mut checked = 0;
    for dir in ["workflows", "scripts/demo/project"] {
        for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                checked += 1;
                bad.extend(violations_in(&path));
            }
        }
    }
    assert!(
        checked >= 12,
        "expected the shipped workflows, found {checked}"
    );
    assert!(
        bad.is_empty(),
        "unquoted interpolations:\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_gate_flags_the_patterns_it_exists_for() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.yaml");
    std::fs::write(
        &path,
        "steps:\n  - name: a\n    type: cmd\n    run: \"gh issue view {{ target }}\"\n  - name: b\n    type: cmd\n    run: |\n      cat > f <<'EOF'\n      {{ steps.x.response | shell_quote }}\n      EOF\n  - name: c\n    type: cmd\n    run: \"cat -- {{ scope.value | shell_quote }} {{ stack.tools.test }}\"\n",
    )
    .unwrap();
    let bad = violations_in(&path);
    assert_eq!(bad.len(), 2, "{bad:#?}");
    assert!(bad[0].contains("{{ target }}"));
    assert!(bad[1].contains("heredoc"));
}

/// Every `run:` of the shipped workflows, rendered with dummy values (each
/// `{{ … }}` becomes one quoted word, as `shell_quote` would produce), must
/// parse as a POSIX shell script. Only `simple-test.yaml` was executed after
/// the P1-2 rewrite; this catches a broken quote or heredoc in the others.
/// The script goes through `sh -n` on stdin (syntax check, nothing runs).
#[test]
fn shipped_run_commands_are_valid_shell_after_rendering() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut checked = 0;
    for dir in ["workflows", "scripts/demo/project"] {
        for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let yaml: serde_yaml::Value =
                serde_yaml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let mut runs = Vec::new();
            collect_runs(&yaml, &mut runs);
            for run in runs {
                let mut rendered = String::new();
                let mut rest = run.as_str();
                while let Some(start) = rest.find("{{") {
                    let end = rest[start..].find("}}").expect("closed expression") + start + 2;
                    rendered.push_str(&rest[..start]);
                    rendered.push_str("'value'");
                    rest = &rest[end..];
                }
                rendered.push_str(rest);
                let mut child = Command::new("sh")
                    .arg("-n")
                    .stdin(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn sh -n");
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(rendered.as_bytes())
                    .unwrap();
                let out = child.wait_with_output().unwrap();
                assert!(
                    out.status.success(),
                    "{}: invalid shell after rendering:\n{rendered}\n{}",
                    path.display(),
                    String::from_utf8_lossy(&out.stderr)
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 30,
        "expected the shipped run commands, checked {checked}"
    );
}

/// The examples in README.md and docs/*.md are copied into real workflows:
/// a `run:` line inside a ```yaml block must shell-quote its interpolations
/// too (single-line `run: "…"` and `run: |` blocks).
#[test]
fn documentation_examples_shell_quote_run_commands() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("README.md")];
    for entry in std::fs::read_dir(root.join("docs")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("md")
            && !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("analise-")
            && path.file_name().unwrap() != "p1-progresso.md"
        {
            files.push(path);
        }
    }
    let mut bad = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        let mut in_yaml = false;
        let mut block_indent: Option<usize> = None;
        for (n, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                in_yaml = !in_yaml && trimmed.trim_start_matches('`').starts_with("yaml");
                block_indent = None;
                continue;
            }
            if !in_yaml {
                continue;
            }
            let indent = line.len() - trimmed.len();
            let mut check = |content: &str| {
                for expr in expressions(content) {
                    if !expr.trim_end().ends_with("| shell_quote") && !allowed_unquoted(&expr) {
                        bad.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                    }
                }
            };
            if let Some(base) = block_indent {
                if !trimmed.is_empty() && indent <= base {
                    block_indent = None;
                } else {
                    check(line);
                    continue;
                }
            }
            if let Some(rest) = trimmed.strip_prefix("run:") {
                if rest.trim_start().starts_with('|') || rest.trim_start().starts_with('>') {
                    block_indent = Some(indent);
                } else {
                    check(rest);
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "unquoted interpolations in documented run commands:\n{}",
        bad.join("\n")
    );
}
