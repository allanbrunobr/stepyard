#!/bin/bash
# Mock Claude CLI for testing - ignores stdin, outputs streaming JSON
printf '{"type":"assistant","content":"Processing your request..."}\n'
printf '{"type":"result","result":"Task completed successfully","session_id":"mock-session-123","usage":{"input_tokens":10,"output_tokens":20},"cost_usd":0.001}\n'
