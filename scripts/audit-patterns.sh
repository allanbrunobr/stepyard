#!/usr/bin/env bash
# Round 3 Story 5 — pattern audits (blocking).
#
# Audit gates are described in CONTRIBUTING.md ("Audit gates").
# Re-copy here verbatim — if architecture.md changes, update both sides in lockstep
# (there is no import mechanism from the spec doc to this script).
#
# Why `--type rust`: every regex below also matches its own literal form inside the
# architecture spec (the doc literally contains the pattern it describes), so an
# unrestricted run finds spec-self-matches. Restricting the corpus to Rust source
# makes the audit measure production code only.
#
# Exit codes:
#   0 — all gates clean.
#   1 — at least one gate matched in Rust source.
#   2 — ripgrep not on PATH.
#
# G4 (emit-before-IO) runs as a separate `cargo xtask audit-emit-before-io` step
# because it needs an AST walker, not a regex; G6 (Send-future compile check) is
# enforced by per-crate `assert_send` tests at compile time and has no runtime
# gate here.

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

# Patterns live in variables so the self-test below runs exactly what the
# gates run. G1/G2 match the macros with or without the `tracing::` prefix:
# `use tracing::info; info!(…)` used to slip through (TEST-06).
TRACING_MACRO='(?:\btracing::|(?<![\w:]))(info|warn|error|debug|trace)(_span)?!'
G1_PATTERN="${TRACING_MACRO}"'\s*\([^)]*(\benv\s*=|[?%]env\b)'
G2_PATTERN="${TRACING_MACRO}"'\s*\([^)]*\b(api_key|secret|token|password|credential)\b\s*=\s*%'
G3_PATTERN='Command::new\(\s*"(sh|bash|zsh|dash)"\s*\)[\s\S]{0,200}\.arg\(\s*"-c"\s*\)'
G5_PATTERN='(TerminationReason|SandboxError)::Other\s*\('
RULE7A_PATTERN='^\s*(?!//).*tokio::time::sleep\s*\('
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

# --- Self-test (TEST-06) --------------------------------------------------------
# Each gate must match a known-bad sample in scripts/audit-fixtures/ (the
# `.fixture` extension keeps them out of the `--type rust` scans). A pattern
# that stops matching its sample would turn the gate into a silent pass.
FIXTURES="$(dirname "$0")/audit-fixtures"
self_test() {
    local gate="$1" pattern="$2" fixture="$3"
    if ! "${RG}" -U --pcre2 -q "${pattern}" "${FIXTURES}/${fixture}"; then
        echo "FAIL self-test: ${gate} does not match ${fixture}" >&2
        BLOCKING_FAILED=1
    fi
}
self_test G1 "${G1_PATTERN}" g1_qualified.rs.fixture
self_test G1 "${G1_PATTERN}" g1_unqualified.rs.fixture
self_test G2 "${G2_PATTERN}" g2_qualified.rs.fixture
self_test G2 "${G2_PATTERN}" g2_unqualified.rs.fixture
self_test G3 "${G3_PATTERN}" g3_sh_c.rs.fixture
self_test G5 "${G5_PATTERN}" g5_other.rs.fixture
self_test Rule7a "${RULE7A_PATTERN}" rule7a_sleep.rs.fixture
if [[ "${BLOCKING_FAILED}" -eq 0 ]]; then
    echo "== self-test: every gate matches its known-bad sample =="
    echo
fi

# --- G1: env-value leak guard (blocking) --------------------------------------
# Matches `tracing::info!( … env = … )` or `… %env` / `… ?env` shapes where the
# full env map's Display/Debug is about to be rendered (leaks values).
run_gate \
    "G1" "env-value leak in tracing macros" "block" \
    "${G1_PATTERN}"

# --- G2: secret field guard (blocking) ----------------------------------------
# Matches `tracing::…!( … api_key = %… )` and friends — renders a secret via
# Display. Allowed shape is `field = ?<value>` (Debug, which for our types
# redacts) or omitting the field entirely.
run_gate \
    "G2" "secret field rendered via Display in tracing macros" "block" \
    "${G2_PATTERN}"

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
    "${G3_PATTERN}" \
    '!**/injection_negative.rs'

# --- G5: Other(...) construction outside classifier (blocking) ----------------
# Only `*_errors.rs` modules are allowed to construct `SandboxError::Other` or
# `TerminationReason::Other` — the discipline from Story 4. This graduated from
# warning-only after the false-positive baseline was burned down.
run_gate \
    "G5" "Other(...) construction outside classifier modules" "block" \
    "${G5_PATTERN}" \
    '!**/*_errors.rs'

# --- Rule 7a: real sleep in integration tests (blocking) ----------------------
# Timing-sensitive async tests should use virtual time (`tokio::time::pause` +
# `advance`) or deterministic handshakes instead of real sleeps. Scope is
# intentionally limited to `tests/**/*.rs`; scanning `#[cfg(test)]` blocks
# inside production modules needs a real parser or a separate allowlist pass.
run_gate \
    "Rule7a" "tokio::time::sleep in integration tests" "block" \
    "${RULE7A_PATTERN}" \
    '**/tests/**/*.rs'

# --- Summary ------------------------------------------------------------------
if [[ "${BLOCKING_FAILED}" -ne 0 ]]; then
    echo "audit-patterns.sh: one or more blocking gates failed." >&2
    exit 1
fi
echo "audit-patterns.sh: all blocking gates clean."
exit 0
