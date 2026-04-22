#!/bin/bash
# Mock Claude CLI that hangs forever — used by agent_replay.rs tests that
# pin cancel / SIGTERM / step-timeout behavior.
#
# Behavior:
# - Optionally capture argv the same way mock_claude.sh does, so cancel /
#   timeout tests can still assert the harness built the right argv (e.g.
#   `--fork-session --resume <id>`) before dropping the child.
# - Drain stdin so the harness's stdin write doesn't block on a full pipe.
# - Emit NOTHING on stdout and sleep forever. The harness spawns with
#   `.kill_on_drop(true)`, so when the outer `tokio::select!` drops the
#   exec future on cancel / signal / timeout, Tokio sends SIGKILL and
#   this mock goes away without outliving the terminal event.
# - Trap SIGTERM / SIGINT with a non-zero exit so stray graceful signals
#   still terminate cleanly. SIGKILL (the default path) cannot be trapped;
#   these traps are belt-and-suspenders for any future code path that
#   opts into graceful shutdown first.

if [ -n "${MOCK_CLAUDE_ARGV_FILE:-}" ]; then
    : > "$MOCK_CLAUDE_ARGV_FILE"
    for arg in "$@"; do
        printf '%s\n' "$arg" >> "$MOCK_CLAUDE_ARGV_FILE"
    done
fi

trap 'exit 143' SIGTERM
trap 'exit 130' SIGINT

cat >/dev/null
# `sleep infinity` is GNU-only; BSD `sleep` (macOS default) rejects it.
# A big fixed number plus a loop keeps the mock alive past any realistic
# test timeout on every platform CI runs on.
while :; do sleep 3600; done
