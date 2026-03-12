# Minion Engine — Example Workflows

A catalog of ready-to-use workflow patterns.

---

## 1. Hello World

```yaml
name: hello-world
description: "Simplest possible workflow"
steps:
  - name: greet
    type: cmd
    run: "echo 'Hello, {{ target }}!'"
```

```bash
minion execute hello-world.yaml -- World
```

---

## 2. Fix a GitHub Issue

Full automation: fetch → plan → implement → lint → test → PR.

```bash
minion execute workflows/fix-issue.yaml -- 247 --verbose
```

See [`workflows/fix-issue.yaml`](../workflows/fix-issue.yaml) for the full workflow.

---

## 3. Code Review

```yaml
name: code-review
description: "AI-powered code review on current branch"
config:
  agent:
    model: claude-sonnet-4-20250514
    permissions: skip
steps:
  - name: diff
    type: cmd
    run: "git diff main...HEAD"
  - name: review
    type: agent
    prompt: |
      Review this diff for bugs, security issues, and code quality:
      {{ steps.diff.stdout }}
```

```bash
minion execute code-review.yaml
```

---

## 4. Retry Loop with Gate

Run `npm test` up to 3 times, stopping on success.

```yaml
name: test-with-retry
scopes:
  test_loop:
    steps:
      - name: test
        type: cmd
        run: "npm test"
        config:
          fail_on_error: false
      - name: check
        type: gate
        condition: "{{ steps.test.exit_code == 0 }}"
        on_pass: break
        on_fail: continue
steps:
  - name: run_tests
    type: repeat
    scope: test_loop
    max_iterations: 3
```

---

## 5. Parallel Quality Checks

Run lint, tests, and format check concurrently.

```yaml
name: quality-gate
steps:
  - name: checks
    type: parallel
    steps:
      - name: lint
        type: cmd
        run: "cargo clippy -- -D warnings"
      - name: test
        type: cmd
        run: "cargo test"
      - name: fmt
        type: cmd
        run: "cargo fmt -- --check"
  - name: report
    type: cmd
    run: "echo 'All quality checks passed'"
```

---

## 6. Map Over Files

Process each changed file individually.

```yaml
name: per-file-review
scopes:
  review_file:
    steps:
      - name: review
        type: agent
        prompt: "Review this file for issues: {{ scope.value }}"
steps:
  - name: list_changed
    type: cmd
    run: "git diff --name-only HEAD~1"
  - name: review_each
    type: map
    scope: review_file
    items: "{{ steps.list_changed.stdout | replace(from='\n', to=',') }}"
    parallel: 1
```

---

## 7. Security Audit

```yaml
name: security-audit
config:
  agent:
    model: claude-sonnet-4-20250514
    permissions: skip
  cmd:
    fail_on_error: false
steps:
  - name: list_files
    type: cmd
    run: "git ls-files | grep -E '\\.(rs|py|js|ts)$'"
  - name: audit_deps
    type: cmd
    run: "cargo audit 2>&1 || echo 'skipped'"
  - name: audit
    type: agent
    prompt: |
      Security audit of these files:
      {{ steps.list_files.stdout }}

      Dependency scan:
      {{ steps.audit_deps.stdout }}

      Find: hardcoded secrets, injection vulnerabilities, insecure dependencies.
```

---

## 8. Conditional Branching with Gate Skip

Skip optional steps based on a flag file.

```yaml
name: conditional-deploy
steps:
  - name: check_flag
    type: cmd
    run: "test -f .skip-deploy && echo 1 || echo 0"
    config:
      fail_on_error: false
  - name: skip_if_flagged
    type: gate
    condition: "{{ steps.check_flag.stdout | trim == '0' }}"
    on_fail: skip
    message: "Deploy skipped by .skip-deploy flag"
  - name: deploy
    type: cmd
    run: "./scripts/deploy.sh"
```

---

## 9. 4-Layer Config Override

```yaml
name: multi-config
config:
  global:
    timeout: 60s
  cmd:
    fail_on_error: true
  patterns:
    "^optional_.*":
      fail_on_error: false
steps:
  - name: required_step
    type: cmd
    run: "must-succeed"
  - name: optional_cleanup
    type: cmd
    run: "maybe-fails"
    config:
      timeout: 10s    # step-level override
```

---

## 10. Using `minion init`

Generate a scaffold from a built-in template:

```bash
# Create a blank workflow
minion init my-workflow

# Create from a specific template
minion init my-security-scan --template security-audit

# List all available templates
minion init --help
```
