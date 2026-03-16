#!/usr/bin/env bash
# =============================================================================
# Minion Engine — Evaluate (autoresearch)
#
# Avaliação fixa para o loop autoresearch.
# NÃO modifique este arquivo — é o juiz.
#
# Usage:
#   ./evaluate.sh              # avaliação completa
#   ./evaluate.sh --fast       # só testes (sem benchmarks)
#
# Output: score único no final (maior = melhor)
# =============================================================================

set -euo pipefail

FAST=false
if [[ "${1:-}" == "--fast" ]]; then
    FAST=true
fi

# Diretório do projeto
PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$PROJECT_DIR"

# Contadores
TOTAL_POINTS=0
MAX_POINTS=0
DETAILS=""

# Helper: adiciona pontos
add_score() {
    local name="$1"
    local points="$2"
    local max="$3"
    local detail="$4"
    TOTAL_POINTS=$((TOTAL_POINTS + points))
    MAX_POINTS=$((MAX_POINTS + max))
    DETAILS="${DETAILS}${name}: ${points}/${max} (${detail})\n"
}

# ─────────────────────────────────────────────────────────────────────────────
# 1. COMPILAÇÃO (compila sem erros?)
# ─────────────────────────────────────────────────────────────────────────────

echo "=== [1/6] Compilation ===" >&2
COMPILE_START=$(python3 -c "import time; print(int(time.time()*1000))")
if cargo build --release 2>/dev/null; then
    COMPILE_END=$(python3 -c "import time; print(int(time.time()*1000))")
    COMPILE_MS=$((COMPILE_END - COMPILE_START))
    add_score "compilation" 10 10 "OK (${COMPILE_MS}ms)"
    COMPILE_OK=true
else
    COMPILE_MS=0
    add_score "compilation" 0 10 "FAILED"
    COMPILE_OK=false
fi

# Se não compila, nada mais funciona
if [ "$COMPILE_OK" = false ]; then
    echo "---"
    echo "score:              0.000000"
    echo "compilation:        FAILED"
    echo "tests_passed:       0"
    echo "tests_total:        0"
    echo "warnings:           0"
    echo "clippy_warnings:    0"
    echo "binary_size_mb:     0.0"
    echo "compile_time_ms:    0"
    echo "total_time_s:       0.0"
    exit 0
fi

# ─────────────────────────────────────────────────────────────────────────────
# 2. TESTES (quantos passam?)
# ─────────────────────────────────────────────────────────────────────────────

echo "=== [2/6] Tests ===" >&2
TEST_OUTPUT=$(cargo test 2>&1 || true)

# Parse test results: "test result: ok. 17 passed; 0 failed; 0 ignored"
TESTS_PASSED=0
TESTS_FAILED=0
TESTS_TOTAL=0

while IFS= read -r line; do
    if [[ "$line" =~ "test result:" ]]; then
        passed=$(echo "$line" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' || echo 0)
        failed=$(echo "$line" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' || echo 0)
        TESTS_PASSED=$((TESTS_PASSED + passed))
        TESTS_FAILED=$((TESTS_FAILED + failed))
    fi
done <<< "$TEST_OUTPUT"

TESTS_TOTAL=$((TESTS_PASSED + TESTS_FAILED))

if [ "$TESTS_TOTAL" -gt 0 ] && [ "$TESTS_FAILED" -eq 0 ]; then
    add_score "tests" 30 30 "${TESTS_PASSED}/${TESTS_TOTAL} passed"
elif [ "$TESTS_TOTAL" -gt 0 ]; then
    # Pontuação proporcional
    TEST_SCORE=$((30 * TESTS_PASSED / TESTS_TOTAL))
    add_score "tests" "$TEST_SCORE" 30 "${TESTS_PASSED}/${TESTS_TOTAL} passed (${TESTS_FAILED} failed)"
else
    add_score "tests" 0 30 "no tests found"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 3. WARNINGS do compilador (menos = melhor)
# ─────────────────────────────────────────────────────────────────────────────

echo "=== [3/6] Compiler Warnings ===" >&2
WARNING_OUTPUT=$(cargo build 2>&1 || true)
WARNINGS=$(echo "$WARNING_OUTPUT" | grep -c "^warning\[" || true)

if [ "$WARNINGS" -eq 0 ]; then
    add_score "warnings" 15 15 "0 warnings"
elif [ "$WARNINGS" -le 5 ]; then
    add_score "warnings" 10 15 "${WARNINGS} warnings"
elif [ "$WARNINGS" -le 15 ]; then
    add_score "warnings" 5 15 "${WARNINGS} warnings"
else
    add_score "warnings" 0 15 "${WARNINGS} warnings"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 4. CLIPPY (lint quality)
# ─────────────────────────────────────────────────────────────────────────────

echo "=== [4/6] Clippy ===" >&2
CLIPPY_OUTPUT=$(cargo clippy --all-targets 2>&1 || true)
CLIPPY_WARNINGS=$(echo "$CLIPPY_OUTPUT" | grep -c "^warning:" || true)

if [ "$CLIPPY_WARNINGS" -eq 0 ]; then
    add_score "clippy" 15 15 "0 warnings"
elif [ "$CLIPPY_WARNINGS" -le 5 ]; then
    add_score "clippy" 10 15 "${CLIPPY_WARNINGS} warnings"
elif [ "$CLIPPY_WARNINGS" -le 15 ]; then
    add_score "clippy" 5 15 "${CLIPPY_WARNINGS} warnings"
else
    add_score "clippy" 0 15 "${CLIPPY_WARNINGS} warnings"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 5. TAMANHO DO BINÁRIO (menor = melhor)
# ─────────────────────────────────────────────────────────────────────────────

echo "=== [5/6] Binary Size ===" >&2
BINARY="target/release/minion"
if [ -f "$BINARY" ]; then
    BINARY_SIZE=$(stat -f%z "$BINARY" 2>/dev/null || stat --format=%s "$BINARY" 2>/dev/null || echo 0)
    BINARY_SIZE_MB=$(echo "scale=1; $BINARY_SIZE / 1048576" | bc)

    # Target: < 15MB = full points, < 30MB = partial, > 30MB = 0
    if (( $(echo "$BINARY_SIZE_MB < 15" | bc -l) )); then
        add_score "binary_size" 10 10 "${BINARY_SIZE_MB}MB (< 15MB)"
    elif (( $(echo "$BINARY_SIZE_MB < 30" | bc -l) )); then
        add_score "binary_size" 5 10 "${BINARY_SIZE_MB}MB (< 30MB)"
    else
        add_score "binary_size" 0 10 "${BINARY_SIZE_MB}MB (> 30MB)"
    fi
else
    BINARY_SIZE_MB="0.0"
    add_score "binary_size" 0 10 "binary not found"
fi

# ─────────────────────────────────────────────────────────────────────────────
# 6. WORKFLOW VALIDATION (todos os YAML são válidos?)
# ─────────────────────────────────────────────────────────────────────────────

echo "=== [6/6] Workflow Validation ===" >&2
WORKFLOW_DIR="$PROJECT_DIR/workflows"
VALID_WORKFLOWS=0
TOTAL_WORKFLOWS=0
INVALID_WORKFLOWS=""

if [ -d "$WORKFLOW_DIR" ]; then
    for wf in "$WORKFLOW_DIR"/*.yaml; do
        if [ -f "$wf" ]; then
            TOTAL_WORKFLOWS=$((TOTAL_WORKFLOWS + 1))
            wf_name=$(basename "$wf")
            if "$BINARY" validate "$wf" >/dev/null 2>&1; then
                VALID_WORKFLOWS=$((VALID_WORKFLOWS + 1))
            else
                INVALID_WORKFLOWS="${INVALID_WORKFLOWS} ${wf_name}"
            fi
        fi
    done
fi

if [ "$TOTAL_WORKFLOWS" -gt 0 ] && [ "$VALID_WORKFLOWS" -eq "$TOTAL_WORKFLOWS" ]; then
    add_score "workflows" 20 20 "${VALID_WORKFLOWS}/${TOTAL_WORKFLOWS} valid"
elif [ "$TOTAL_WORKFLOWS" -gt 0 ]; then
    WF_SCORE=$((20 * VALID_WORKFLOWS / TOTAL_WORKFLOWS))
    add_score "workflows" "$WF_SCORE" 20 "${VALID_WORKFLOWS}/${TOTAL_WORKFLOWS} valid (invalid:${INVALID_WORKFLOWS})"
else
    add_score "workflows" 0 20 "no workflows found"
fi

# ─────────────────────────────────────────────────────────────────────────────
# SCORE FINAL
# ─────────────────────────────────────────────────────────────────────────────

TOTAL_END=$(date +%s%N 2>/dev/null || date +%s)
if [ "$MAX_POINTS" -gt 0 ]; then
    SCORE=$(python3 -c "print(f'{$TOTAL_POINTS/$MAX_POINTS:.6f}')")
else
    SCORE="0.000000"
fi

TOTAL_TIME_S=$(echo "scale=1; $COMPILE_MS / 1000" | bc 2>/dev/null || echo "0.0")

# Output no formato autoresearch
echo "---"
echo "score:              $SCORE"
echo "compilation:        $([ "$COMPILE_OK" = true ] && echo 'OK' || echo 'FAILED')"
echo "tests_passed:       $TESTS_PASSED"
echo "tests_total:        $TESTS_TOTAL"
echo "tests_failed:       $TESTS_FAILED"
echo "warnings:           $WARNINGS"
echo "clippy_warnings:    $CLIPPY_WARNINGS"
echo "binary_size_mb:     $BINARY_SIZE_MB"
echo "compile_time_ms:    $COMPILE_MS"
echo "workflows_valid:    $VALID_WORKFLOWS"
echo "workflows_total:    $TOTAL_WORKFLOWS"
echo "total_time_s:       $TOTAL_TIME_S"
