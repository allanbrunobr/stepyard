#!/usr/bin/env bash
# Round 3 Story 5 — pattern audits (G1, G2, G3 blocking + G5/Rule7a warning-only).
#
# Source-of-truth regexes: _bmad-output/sandcastle-features/architecture.md §G-gates.
# Re-copy here verbatim — if architecture.md changes, update both sides in lockstep
# (there is no import mechanism from the spec doc to this script).
#
# Why `--type rust`: every regex below also matches its own literal form inside the
# architecture spec (the doc literally contains the pattern it describes), so an
# unrestricted run finds spec-self-matches. Restricting the corpus to Rust source
# makes the audit measure production code only.
#
# Exit codes:
#   0 — all blocking gates clean (warnings are informational, never fail CI).
#   1 — at least one of G1/G2/G3 matched in Rust source.
#   2 — ripgrep not on PATH.
#
# G4 (emit-before-IO) runs as a separate `cargo xtask audit-emit-before-io` step
# because it needs an AST walker, not a regex; G6 (Send-future compile check) is
# enforced by per-crate `assert_send` tests at compile time and has no runtime
# gate here. Rule 7a is warning-only while the existing real-sleep baseline is
# burned down in follow-up PRs.

set -uo pipefail

# Resolve ripgrep once: CI installs it as /usr/bin/rg, dev shells usually have
# /usr/local/bin/rg (Homebrew) or /opt/homebrew/bin/rg (Apple Silicon). Failing
# loudly here is better than a silent "no matches" that hides a broken audit.
if ! command -v rg >/dev/null 2>&1; then
    echo "audit-patterns.sh: ripgrep (rg) not found on PATH" >&2
    echo "  CI: add 'sudo apt-get install -y ripgrep' before running this script" >&2
    echo "  Dev: 'brew install ripgrep' or equivalent" >&2
    exit 2
fi

RG=$(command -v rg)
BLOCKING_FAILED=0

# run_gate <gate-id> <description> <mode: block|warn> <pattern> [extra-glob]
#
# `extra-glob` is a single ripgrep --glob argument (typically an exclusion like
# '!**/foo.rs'). Omitted ⇒ no glob filter. `-U` enables multiline so
# `[\s\S]{0,200}` spans newlines (G3's distance check); `--pcre2` is required
# for `[\s\S]` over line breaks; `--type rust` excludes .md/.yml spec files
# that would otherwise self-match.
run_gate() {
    local gate="$1"
    local desc="$2"
    local mode="$3"
    local pattern="$4"
    local extra_glob="${5:-}"

    local cmd=("${RG}" -U --pcre2 --type rust -n "${pattern}")
    if [[ -n "${extra_glob}" ]]; then
        cmd+=(--glob "${extra_glob}")
    fi

    if [[ "${mode}" == "warn" ]]; then
        echo "== ${gate} — ${desc} (warning-only) =="
    else
        echo "== ${gate} — ${desc} =="
    fi

    if "${cmd[@]}"; then
        if [[ "${mode}" == "warn" ]]; then
            echo "WARN: ${gate} matched — review above. Not blocking." >&2
        else
            echo "FAIL: ${gate} matched above in Rust source — fix before merge." >&2
            BLOCKING_FAILED=1
        fi
    else
        echo "OK: 0 matches."
    fi
    echo
}

# --- G1: env-value leak guard (blocking) --------------------------------------
# Matches `tracing::info!( … env = … )` or `… %env` / `… ?env` shapes where the
# full env map's Display/Debug is about to be rendered (leaks values).
run_gate \
    "G1" "env-value leak in tracing macros" "block" \
    'tracing::(info|warn|error|debug|trace)(_span)?!\s*\([^)]*(\benv\s*=|[?%]env\b)'

# --- G2: secret field guard (blocking) ----------------------------------------
# Matches `tracing::…!( … api_key = %… )` and friends — renders a secret via
# Display. Allowed shape is `field = ?<value>` (Debug, which for our types
# redacts) or omitting the field entirely.
run_gate \
    "G2" "secret field rendered via Display in tracing macros" "block" \
    'tracing::(info|warn|error|debug|trace)(_span)?!\s*\([^)]*\b(api_key|secret|token|password|credential)\b\s*=\s*%'

# --- G3: shell-string joining guard (blocking) --------------------------------
# `Command::new("sh").arg("-c")` is the string-concat anti-pattern — untrusted
# input interpolated into a shell string. Argv form (separate `.arg()` per token
# or `.args([...])` without `-c`) is the allowed shape. `/bin/sh` is NOT in the
# alternation on purpose: the existing docker.rs workspace-copy path uses that
# literal with a hardcoded pipe_cmd, which is a classified-safe carveout.
# Allowlist-by-convention: `**/injection_negative.rs` (reserved filename for
# future negative-control tests that deliberately construct the forbidden shape
# and assert it is rejected by validation). The file does not exist yet; the
# glob exclusion is forward-compatible so we don't have to churn this script
# when the test lands.
run_gate \
    "G3" "shell-string joining (Command::new(shell) + -c)" "block" \
    'Command::new\(\s*"(sh|bash|zsh|dash)"\s*\)[\s\S]{0,200}\.arg\(\s*"-c"\s*\)' \
    '!**/injection_negative.rs'

# --- G5: Other(...) construction outside classifier (warning-only) ------------
# Only `*_errors.rs` modules are allowed to construct `SandboxError::Other` or
# `TerminationReason::Other` — the discipline from Story 4. Two known false
# positives today that we deliberately accept (documented in the Story 5 PR):
#   * src/main.rs — the inline `//` doc comment on the sanitize_human call
#     cites `SandboxError::Other(String)` as prose, not a construction site.
#   * crates/stepyard-core/src/error.rs — a unit test that asserts the Display
#     impl of `TerminationReason::Other("disk full")`. Test-only construction is
#     fine; a future refinement can scope this warning to non-test code.
run_gate \
    "G5" "Other(...) construction outside classifier modules" "warn" \
    '(TerminationReason|SandboxError)::Other\s*\(' \
    '!**/*_errors.rs'

# --- Rule 7a: real sleep in integration tests (warning-only) ------------------
# Timing-sensitive async tests should use virtual time (`tokio::time::pause` +
# `advance`) instead of real sleeps. Existing integration tests still contain a
# baseline of real sleeps, so this starts warning-only. Scope is intentionally
# limited to `tests/**/*.rs`; scanning `#[cfg(test)]` blocks inside production
# modules needs a real parser or a separate allowlist pass.
run_gate \
    "Rule7a" "tokio::time::sleep in integration tests" "warn" \
    '^\s*(?!//).*tokio::time::sleep\s*\(' \
    '**/tests/**/*.rs'

# --- Summary ------------------------------------------------------------------
if [[ "${BLOCKING_FAILED}" -ne 0 ]]; then
    echo "audit-patterns.sh: one or more blocking gates failed." >&2
    exit 1
fi
echo "audit-patterns.sh: all blocking gates clean."
exit 0
