#!/bin/bash
# Mock Claude CLI for testing — drains stdin to EOF, then emits streaming JSON.
# Draining is required: the real agent step pipes the prompt in via stdin and
# only then reads stdout. If this script exits before the write completes, the
# writer sees EPIPE (Broken pipe) and the test flakes on slow CI runners.
cat >/dev/null
printf '{"type":"assistant","content":"Processing your request..."}\n'
printf '{"type":"result","result":"Task completed successfully","session_id":"mock-session-123","usage":{"input_tokens":10,"output_tokens":20},"cost_usd":0.001}\n'
