#!/bin/bash
# Mock Claude CLI for testing — drains stdin to EOF, then emits streaming JSON.
# Draining is required: the real agent step pipes the prompt in via stdin and
# only then reads stdout. If this script exits before the write completes, the
# writer sees EPIPE (Broken pipe) and the test flakes on slow CI runners.

# Optional argv capture for integration tests. When MOCK_CLAUDE_ARGV_FILE is
# set, write each CLI argument on its own line to that path before doing any
# other work. This lets tests assert on the exact argv the harness spawned
# (e.g. "--fork-session --resume <id>" ordering) without threading the argv
# through the JSON stdout channel — the latter would conflate "what the CLI
# received" with "what the CLI decided to emit", and the whole point of the
# assertions is to catch harness-side drift, not mock-side.
if [ -n "${MOCK_CLAUDE_ARGV_FILE:-}" ]; then
    : > "$MOCK_CLAUDE_ARGV_FILE"
    for arg in "$@"; do
        printf '%s\n' "$arg" >> "$MOCK_CLAUDE_ARGV_FILE"
    done
fi

cat >/dev/null
printf '{"type":"assistant","content":"Processing your request..."}\n'
printf '{"type":"result","result":"Task completed successfully","session_id":"mock-session-123","usage":{"input_tokens":10,"output_tokens":20},"cost_usd":0.001}\n'
