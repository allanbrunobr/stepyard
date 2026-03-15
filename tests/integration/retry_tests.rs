use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::test;

use workflow_engine::config::StepConfig;
use workflow_engine::engine::context::Context;
use workflow_engine::error::StepError;
use workflow_engine::steps::{agent::AgentExecutor, StepExecutor, StepOutput};
use workflow_engine::workflow::schema::{StepDef, StepType};

fn make_agent_step(prompt: &str) -> StepDef {
    StepDef {
        name: "test_agent".to_string(),
        step_type: StepType::Agent,
        run: None,
        prompt: Some(prompt.to_string()),
        condition: None,
        on_pass: None,
        on_fail: None,
        message: None,
        scope: None,
        max_iterations: None,
        initial_value: None,
        items: None,
        parallel: None,
        steps: None,
        config: HashMap::new(),
        outputs: None,
        output_type: None,
        async_exec: None,
    }
}

#[tokio::test]
async fn test_agent_retry_on_rate_limit() {
    // Create a mock Claude CLI script that fails with rate limit error on first attempt,
    // succeeds on second attempt
    let mock_script_content = r#"#!/bin/bash
# Mock Claude CLI for testing retry behavior
COUNTER_FILE="/tmp/retry_test_counter_$$"

# Initialize counter
if [ ! -f "$COUNTER_FILE" ]; then
    echo "0" > "$COUNTER_FILE"
fi

# Read and increment counter
COUNTER=$(<"$COUNTER_FILE")
COUNTER=$((COUNTER + 1))
echo "$COUNTER" > "$COUNTER_FILE"

if [ "$COUNTER" -eq "1" ]; then
    # First call: simulate rate limit error
    echo "Error: HTTP 429 Too Many Requests - Rate limit exceeded. Please retry after 1 second." >&2
    exit 1
elif [ "$COUNTER" -eq "2" ]; then
    # Second call: success
    echo '{"type":"result","result":"Success after retry","session_id":"retry-session-123","usage":{"input_tokens":20,"output_tokens":30},"cost_usd":0.002}'
    rm -f "$COUNTER_FILE"  # Cleanup
    exit 0
else
    # Shouldn't reach here in this test
    echo "Unexpected call count: $COUNTER" >&2
    rm -f "$COUNTER_FILE"
    exit 1
fi
"#;

    // Create temporary script file
    let temp_dir = std::env::temp_dir();
    let script_path = temp_dir.join(format!("mock_claude_retry_integration_{}.sh", std::process::id()));
    std::fs::write(&script_path, mock_script_content).expect("Failed to write mock script");

    // Make script executable
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script_path).expect("Failed to get file metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("Failed to set permissions");

    // Configure agent step with retry settings
    let step = make_agent_step("Test retry behavior");
    let mut config_values = HashMap::new();
    config_values.insert(
        "command".to_string(),
        serde_json::Value::String(script_path.to_string_lossy().to_string()),
    );
    config_values.insert("max_retries".to_string(), serde_json::Value::Number(3.into()));
    config_values.insert("retry_base_delay_ms".to_string(), serde_json::Value::Number(50.into())); // Fast for testing
    config_values.insert("retry_max_delay_ms".to_string(), serde_json::Value::Number(200.into()));

    let config = StepConfig { values: config_values };
    let ctx = Context::new(String::new(), HashMap::new());

    // Execute and measure time
    let start_time = Instant::now();
    let result = AgentExecutor.execute(&step, &config, &ctx).await;
    let elapsed = start_time.elapsed();

    // Verify success
    assert!(result.is_ok(), "Expected successful execution after retry");

    if let Ok(StepOutput::Agent(output)) = result {
        assert_eq!(output.response, "Success after retry");
        assert_eq!(output.session_id.as_deref(), Some("retry-session-123"));
        assert_eq!(output.stats.input_tokens, 20);
        assert_eq!(output.stats.output_tokens, 30);

        // Should have taken at least 50ms due to retry delay
        assert!(
            elapsed >= Duration::from_millis(40),
            "Expected delay for retry, but elapsed time was {:?}", elapsed
        );
    } else {
        panic!("Expected AgentOutput");
    }

    // Cleanup
    let _ = std::fs::remove_file(&script_path);
}

#[tokio::test]
async fn test_agent_retry_exhaustion() {
    // Create a mock script that always returns rate limit error
    let mock_script_content = r#"#!/bin/bash
# Always return rate limit error
echo "Error: HTTP 429 Too Many Requests - Rate limit exceeded continuously" >&2
exit 1
"#;

    let temp_dir = std::env::temp_dir();
    let script_path = temp_dir.join(format!("mock_claude_exhaustion_{}.sh", std::process::id()));
    std::fs::write(&script_path, mock_script_content).expect("Failed to write mock script");

    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script_path).expect("Failed to get metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("Failed to set permissions");

    let step = make_agent_step("Test retry exhaustion");
    let mut config_values = HashMap::new();
    config_values.insert(
        "command".to_string(),
        serde_json::Value::String(script_path.to_string_lossy().to_string()),
    );
    config_values.insert("max_retries".to_string(), serde_json::Value::Number(2.into()));
    config_values.insert("retry_base_delay_ms".to_string(), serde_json::Value::Number(10.into())); // Very fast for testing

    let config = StepConfig { values: config_values };
    let ctx = Context::new(String::new(), HashMap::new());

    let result = AgentExecutor.execute(&step, &config, &ctx).await;

    // Verify that we get RateLimitExhausted error
    assert!(result.is_err(), "Expected failure after retry exhaustion");

    if let Err(StepError::RateLimitExhausted { provider, attempts }) = result {
        assert_eq!(provider, "claude-cli");
        assert_eq!(attempts, 3); // max_retries(2) + 1
    } else {
        panic!("Expected RateLimitExhausted error, got {:?}", result);
    }

    // Cleanup
    let _ = std::fs::remove_file(&script_path);
}

#[tokio::test]
async fn test_agent_no_retry_on_non_rate_limit_error() {
    // Create a mock script that returns a non-rate-limit error
    let mock_script_content = r#"#!/bin/bash
# Return a non-rate-limit error
echo "Error: Internal server error" >&2
exit 1
"#;

    let temp_dir = std::env::temp_dir();
    let script_path = temp_dir.join(format!("mock_claude_no_retry_{}.sh", std::process::id()));
    std::fs::write(&script_path, mock_script_content).expect("Failed to write mock script");

    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script_path).expect("Failed to get metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("Failed to set permissions");

    let step = make_agent_step("Test no retry on non-rate-limit error");
    let mut config_values = HashMap::new();
    config_values.insert(
        "command".to_string(),
        serde_json::Value::String(script_path.to_string_lossy().to_string()),
    );
    config_values.insert("max_retries".to_string(), serde_json::Value::Number(3.into()));
    config_values.insert("retry_base_delay_ms".to_string(), serde_json::Value::Number(50.into()));

    let config = StepConfig { values: config_values };
    let ctx = Context::new(String::new(), HashMap::new());

    let start_time = Instant::now();
    let result = AgentExecutor.execute(&step, &config, &ctx).await;
    let elapsed = start_time.elapsed();

    // Should fail immediately without retry
    assert!(result.is_err(), "Expected immediate failure");
    assert!(
        elapsed < Duration::from_millis(30),
        "Should fail immediately without retry delay, elapsed: {:?}",
        elapsed
    );

    // Should NOT be a RateLimitExhausted error
    if let Err(err) = result {
        assert!(!matches!(err, StepError::RateLimitExhausted { .. }));
    }

    // Cleanup
    let _ = std::fs::remove_file(&script_path);
}