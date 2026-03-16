/// Template definitions for `minion init`
pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub content: &'static str,
}

pub const TEMPLATES: &[Template] = &[
    Template {
        name: "blank",
        description: "Minimal workflow skeleton",
        content: BLANK,
    },
    Template {
        name: "fix-issue",
        description: "Fetch a GitHub issue, plan and implement a fix, open a PR",
        content: FIX_ISSUE,
    },
    Template {
        name: "code-review",
        description: "Run an AI-powered code review on a branch or PR",
        content: CODE_REVIEW,
    },
    Template {
        name: "security-audit",
        description: "Perform a security audit on the codebase",
        content: SECURITY_AUDIT,
    },
];

pub fn get(name: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.name == name)
}

pub fn names() -> Vec<&'static str> {
    TEMPLATES.iter().map(|t| t.name).collect()
}

// ─── Templates ───────────────────────────────────────────────────────────────

const BLANK: &str = r#"name: {name}
version: 1
description: "Short description of what this workflow does"

# Optional: global config applied to all steps
config:
  global:
    timeout: 300s
  cmd:
    fail_on_error: true

steps:
  - name: hello
    type: cmd
    run: "echo 'Hello, {{ target }}!'"
"#;

const FIX_ISSUE: &str = r#"name: {name}
version: 1
description: "Fetch a GitHub issue, plan & implement a fix, open a PR"

config:
  global:
    timeout: 600s
  agent:
    model: claude-sonnet-4-20250514
    permissions: skip
  cmd:
    fail_on_error: true

scopes:
  lint_loop:
    steps:
      - name: lint
        type: cmd
        run: "cargo clippy --fix --allow-dirty 2>&1 || true"
      - name: check_lint
        type: gate
        condition: "{{ steps.lint.exit_code == 0 }}"
        on_pass: break
    outputs: "{{ steps.lint.stdout }}"

steps:
  - name: fetch_issue
    type: cmd
    run: "gh issue view {{ target }} --json title,body,labels"

  - name: find_files
    type: cmd
    run: "git ls-files | head -50"

  - name: plan
    type: agent
    prompt: |
      You are a senior engineer. Analyze this GitHub issue and plan a fix.
      Issue: {{ steps.fetch_issue.stdout }}
      Files: {{ steps.find_files.stdout }}
      Provide a concise implementation plan.

  - name: implement
    type: agent
    prompt: |
      Implement the fix described in this plan:
      {{ steps.plan.response }}
      Make the minimal changes needed. Do not add unrelated improvements.

  - name: lint_fix
    type: repeat
    scope: lint_loop
    max_iterations: 3

  - name: create_branch
    type: cmd
    run: "git checkout -b fix/issue-{{ target }}"

  - name: commit
    type: cmd
    run: "git add -A && git commit -m 'fix: resolve issue #{{ target }}'"

  - name: open_pr
    type: cmd
    run: |
      gh pr create \
        --title "fix: resolve issue #{{ target }}" \
        --body "Closes #{{ target }}" \
        --head "fix/issue-{{ target }}"
"#;

const CODE_REVIEW: &str = r#"name: {name}
version: 1
description: "AI-powered code review on a branch or PR"

config:
  global:
    timeout: 300s
  agent:
    model: claude-sonnet-4-20250514
    permissions: skip

steps:
  - name: get_diff
    type: cmd
    run: "git diff main...HEAD"

  - name: get_files
    type: cmd
    run: "git diff --name-only main...HEAD"

  - name: review
    type: agent
    prompt: |
      You are a senior engineer performing a code review.

      Changed files:
      {{ steps.get_files.stdout }}

      Diff:
      {{ steps.get_diff.stdout }}

      Review for:
      1. Logic errors and bugs
      2. Security vulnerabilities
      3. Performance issues
      4. Code quality and readability
      5. Missing tests

      Be specific. Reference line numbers where possible.

  - name: summary
    type: cmd
    run: |
      echo "=== Code Review Complete ==="
      echo "{{ steps.review.response }}"
"#;

const SECURITY_AUDIT: &str = r#"name: {name}
version: 1
description: "Security audit of the codebase"

config:
  global:
    timeout: 600s
  agent:
    model: claude-sonnet-4-20250514
    permissions: skip
  cmd:
    fail_on_error: false

steps:
  - name: list_files
    type: cmd
    run: "git ls-files | grep -E '\\.(rs|py|js|ts|go|java|rb|php)$' | head -100"

  - name: check_deps
    type: cmd
    run: "cargo audit 2>&1 || echo 'cargo audit not available'"

  - name: find_secrets
    type: cmd
    run: |
      grep -rn \
        -e 'password\s*=' \
        -e 'secret\s*=' \
        -e 'api_key\s*=' \
        -e 'token\s*=' \
        --include='*.rs' --include='*.toml' . 2>/dev/null | head -20 || echo "No obvious secrets found"

  - name: audit
    type: agent
    prompt: |
      You are a security engineer performing a security audit.

      Project files:
      {{ steps.list_files.stdout }}

      Dependency audit:
      {{ steps.check_deps.stdout }}

      Potential secrets scan:
      {{ steps.find_secrets.stdout }}

      Identify:
      1. Hardcoded secrets or credentials
      2. Injection vulnerabilities (SQL, command, path traversal)
      3. Insecure dependencies
      4. Missing input validation
      5. Improper error handling that leaks info

      Provide a severity-ranked list of findings with remediation steps.

  - name: report
    type: cmd
    run: |
      echo "=== Security Audit Report ==="
      echo "{{ steps.audit.response }}"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_template_names_are_unique() {
        let mut names = names();
        let original_len = names.len();
        names.dedup();
        assert_eq!(names.len(), original_len, "template names must be unique");
    }

    #[test]
    fn get_blank_template() {
        let t = get("blank").expect("blank template should exist");
        assert_eq!(t.name, "blank");
        assert!(!t.content.is_empty());
    }

    #[test]
    fn get_nonexistent_template_returns_none() {
        assert!(get("does-not-exist").is_none());
    }

    #[test]
    fn all_templates_contain_name_placeholder() {
        for t in TEMPLATES {
            assert!(
                t.content.contains("{name}"),
                "template '{}' must contain {{name}} placeholder",
                t.name
            );
        }
    }

    #[test]
    fn init_creates_valid_yaml_for_each_template() {
        use crate::workflow::parser;
        use crate::workflow::validator;

        for t in TEMPLATES {
            let content = t.content.replace("{name}", "test-workflow");
            let wf = parser::parse_str(&content)
                .unwrap_or_else(|e| panic!("template '{}' failed to parse: {e}", t.name));
            let errors = validator::validate(&wf);
            assert!(
                errors.is_empty(),
                "template '{}' has validation errors: {:?}",
                t.name,
                errors
            );
        }
    }
}
