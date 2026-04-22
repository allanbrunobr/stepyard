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
#   this mock goes away without outliving the terminal event. SIGKILL
#   cannot be trapped, so no signal handlers are installed — `sleep`'s
#   default SIGTERM/SIGINT behavior (exit 143 / 130) is already correct
#   if any future code path sends a graceful signal before SIGKILL.

if [ -n "${MOCK_CLAUDE_ARGV_FILE:-}" ]; then
    : > "$MOCK_CLAUDE_ARGV_FILE"
    for arg in "$@"; do
        printf '%s\n' "$arg" >> "$MOCK_CLAUDE_ARGV_FILE"
    done
fi

cat >/dev/null
# `exec sleep` (not a bash loop + child sleep) so the direct process the
# harness spawned IS the sleep — `kill_on_drop(true)` only kills the
# immediate child, so a `while … do sleep` loop leaves the `sleep`
# grandchild alive after the bash wrapper dies. Replacing the shell with
# sleep via `exec` collapses the tree to a single process.
#
# 3600 (= 1 h) is BSD/Linux-portable; `sleep infinity` is GNU-only and
# BSD `sleep` (macOS default) rejects it. Any realistic test timeout is
# far below 3600s.
exec sleep 3600
