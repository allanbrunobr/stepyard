use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::cli::display;
use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::StepDef;

use super::{retry::RetryConfig, AgentOutput, AgentStats, SandboxAwareExecutor, SharedSandbox, StepExecutor, StepOutput};

pub struct AgentExecutor;

impl AgentExecutor {
    /// Build the claude CLI args from step config
    pub(crate) fn build_args(config: &StepConfig, ctx: &Context) -> Result<Vec<String>, StepError> {
        let mut args: Vec<String> = vec![
            "-p".into(),
            "--verbose".into(),
            "--output-format".into(),
            "stream-json".into(),
        ];

        if let Some(model) = config.get_str("model") {
            args.extend(["--model".into(), model.into()]);
        }
        if let Some(sp) = config.get_str("system_prompt_append") {
            args.extend(["--append-system-prompt".into(), sp.into()]);
        }
        if config.get_str("permissions") == Some("skip") {
            args.push("--dangerously-skip-permissions".into());
        }

        // Session resume (Story 2.1)
        if let Some(resume_step) = config.get_str("resume") {
            let session_id = lookup_session_id(ctx, resume_step)?;
            args.extend(["--resume".into(), session_id]);
        }

        // Session fork (Story 2.2) — uses same --resume flag; Claude CLI creates new session
        if let Some(fork_step) = config.get_str("fork_session") {
            let session_id = lookup_session_id(ctx, fork_step)?;
            args.extend(["--resume".into(), session_id]);
        }

        Ok(args)
    }

    /// Parse stream-json output from Claude CLI
    fn parse_stream_json(line: &str, response: &mut String, session_id: &mut Option<String>, stats: &mut AgentStats) {
        if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) {
            match msg.get("type").and_then(|t| t.as_str()) {
                Some("result") => {
                    if let Some(r) = msg.get("result").and_then(|r| r.as_str()) {
                        *response = r.to_string();
                    }
                    *session_id =
                        msg.get("session_id").and_then(|s| s.as_str()).map(String::from);
                    if let Some(usage) = msg.get("usage") {
                        stats.input_tokens =
                            usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                        stats.output_tokens = usage
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                    }
                    if let Some(cost) = msg.get("cost_usd").and_then(|c| c.as_f64()) {
                        stats.cost_usd = cost;
                    }
                }
                Some("assistant") => {
                    if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                        display::agent_progress(content);
                    }
                }
                Some("tool_use") => {
                    if let Some(tool) = msg.get("tool").and_then(|t| t.as_str()) {
                        display::tool_use(tool, "");
                    }
                }
                _ => {}
            }
        }
    }

    /// Check if Claude CLI error output indicates a rate limit
    fn is_claude_cli_rate_limit_error(stderr: &str) -> bool {
        use super::retry::is_rate_limit_error_generic;
        is_rate_limit_error_generic(stderr)
    }

    /// Execute agent on the host with retry logic for rate limits
    async fn execute_on_host_with_retry(
        &self,
        prompt: &str,
        command: &str,
        args: &[String],
        timeout: Duration,
        retry_config: &RetryConfig,
    ) -> Result<StepOutput, StepError> {
        use super::retry::{calculate_backoff_delay, extract_retry_after_generic};
        use tokio::time::sleep;

        for attempt in 0..=retry_config.max_retries {
            let result = self.execute_on_host(prompt, command, args, timeout).await;

            match result {
                Ok(output) => return Ok(output),
                Err(err) => {
                    let error_text = err.to_string();
                    let is_rate_limit = Self::is_claude_cli_rate_limit_error(&error_text);

                    if is_rate_limit && attempt < retry_config.max_retries {
                        let retry_after = extract_retry_after_generic(&error_text);
                        let delay = calculate_backoff_delay(attempt, retry_config, retry_after);

                        tracing::warn!(
                            provider = "claude-cli",
                            attempt = attempt + 1,
                            delay_ms = delay.as_millis(),
                            "Claude CLI rate limit hit, retrying after delay"
                        );

                        sleep(delay).await;
                        continue;
                    }

                    // Either not a rate limit error, or retries exhausted
                    if is_rate_limit && attempt >= retry_config.max_retries {
                        return Err(StepError::RateLimitExhausted {
                            provider: "claude-cli".to_string(),
                            attempts: retry_config.max_retries + 1
                        });
                    }

                    return Err(err);
                }
            }
        }

        unreachable!("Loop should always return or continue")
    }

    /// Execute agent on the host (no sandbox)
    async fn execute_on_host(
        &self,
        prompt: &str,
        command: &str,
        args: &[String],
        timeout: Duration,
    ) -> Result<StepOutput, StepError> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| StepError::Fail(format!("Failed to spawn {command}: {e}")))?;

        // Send prompt via stdin
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await.map_err(|e| {
                StepError::Fail(format!("Failed to write prompt to stdin: {e}"))
            })?;
            drop(stdin);
        }

        // Parse streaming JSON from stdout
        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        let start = Instant::now();
        let mut response = String::new();
        let mut session_id = None;
        let mut stats = AgentStats::default();

        let parse_result = tokio::time::timeout(timeout, async {
            while let Ok(Some(line)) = lines.next_line().await {
                Self::parse_stream_json(&line, &mut response, &mut session_id, &mut stats);
            }
        })
        .await;

        if parse_result.is_err() {
            let _ = child.kill().await;
            return Err(StepError::Timeout(timeout));
        }

        let status = child.wait().await.map_err(|e| {
            StepError::Fail(format!("Failed to wait for claude process: {e}"))
        })?;

        if !status.success() && response.is_empty() {
            return Err(StepError::Fail(format!(
                "Claude Code exited with status {}",
                status.code().unwrap_or(-1)
            )));
        }

        stats.duration = start.elapsed();

        Ok(StepOutput::Agent(AgentOutput {
            response,
            session_id,
            stats,
        }))
    }

    /// Execute agent in sandbox with retry logic for rate limits
    async fn execute_in_sandbox_with_retry(
        &self,
        prompt: &str,
        command: &str,
        args: &[String],
        timeout: Duration,
        sandbox: &SharedSandbox,
        retry_config: &RetryConfig,
    ) -> Result<StepOutput, StepError> {
        use super::retry::{calculate_backoff_delay, extract_retry_after_generic};
        use tokio::time::sleep;

        for attempt in 0..=retry_config.max_retries {
            let result = self.execute_in_sandbox(prompt, command, args, timeout, sandbox).await;

            match result {
                Ok(output) => return Ok(output),
                Err(err) => {
                    let error_text = err.to_string();
                    let is_rate_limit = Self::is_claude_cli_rate_limit_error(&error_text);

                    if is_rate_limit && attempt < retry_config.max_retries {
                        let retry_after = extract_retry_after_generic(&error_text);
                        let delay = calculate_backoff_delay(attempt, retry_config, retry_after);

                        tracing::warn!(
                            provider = "claude-cli",
                            attempt = attempt + 1,
                            delay_ms = delay.as_millis(),
                            "Claude CLI rate limit hit in sandbox, retrying after delay"
                        );

                        sleep(delay).await;
                        continue;
                    }

                    // Either not a rate limit error, or retries exhausted
                    if is_rate_limit && attempt >= retry_config.max_retries {
                        return Err(StepError::RateLimitExhausted {
                            provider: "claude-cli".to_string(),
                            attempts: retry_config.max_retries + 1
                        });
                    }

                    return Err(err);
                }
            }
        }

        unreachable!("Loop should always return or continue")
    }

    /// Execute agent inside a Docker sandbox container
    async fn execute_in_sandbox(
        &self,
        prompt: &str,
        command: &str,
        args: &[String],
        timeout: Duration,
        sandbox: &SharedSandbox,
    ) -> Result<StepOutput, StepError> {
        let sb = sandbox.as_ref().ok_or_else(|| {
            StepError::Fail("Sandbox reference is None but sandbox execution was requested".into())
        })?;

        let start = Instant::now();

        // Escape the prompt for shell embedding
        let escaped_prompt = prompt.replace('\'', "'\\''");

        // Build the full command to run inside the container:
        // echo '<prompt>' | claude -p --output-format stream-json ...
        let args_str = args.join(" ");
        // Set HOME for the minion user so Claude CLI finds its config
        let sandbox_cmd = format!(
            "export HOME=/home/minion && echo '{}' | {} {}",
            escaped_prompt, command, args_str
        );

        let sb_guard = sb.lock().await;
        let sb_output = tokio::time::timeout(timeout, sb_guard.run_command_as_user(&sandbox_cmd, "minion"))
            .await
            .map_err(|_| StepError::Timeout(timeout))?
            .map_err(|e| StepError::Fail(format!("Sandbox agent execution failed: {e}")))?;

        // Parse the stream-json output (line by line)
        let mut response = String::new();
        let mut session_id = None;
        let mut stats = AgentStats::default();

        for line in sb_output.stdout.lines() {
            Self::parse_stream_json(line, &mut response, &mut session_id, &mut stats);
        }

        if sb_output.exit_code != 0 && response.is_empty() {
            return Err(StepError::Fail(format!(
                "Claude Code in sandbox exited with status {}: {}",
                sb_output.exit_code,
                sb_output.stderr.trim()
            )));
        }

        stats.duration = start.elapsed();

        Ok(StepOutput::Agent(AgentOutput {
            response,
            session_id,
            stats,
        }))
    }
}

fn lookup_session_id(ctx: &Context, step_name: &str) -> Result<String, StepError> {
    ctx.get_step(step_name)
        .and_then(|out| {
            if let StepOutput::Agent(a) = out {
                a.session_id.clone()
            } else {
                None
            }
        })
        .ok_or_else(|| StepError::Fail(format!("session not found for step '{}'", step_name)))
}

#[async_trait]
impl StepExecutor for AgentExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        self.execute_sandboxed(step, config, ctx, &None).await
    }
}

#[async_trait]
impl SandboxAwareExecutor for AgentExecutor {
    async fn execute_sandboxed(
        &self,
        step: &StepDef,
        config: &StepConfig,
        ctx: &Context,
        sandbox: &SharedSandbox,
    ) -> Result<StepOutput, StepError> {
        let prompt_template = step
            .prompt
            .as_ref()
            .ok_or_else(|| StepError::Fail("agent step missing 'prompt' field".into()))?;

        let prompt = ctx.render_template(prompt_template)?;
        let command = config.get_str("command").unwrap_or("claude");
        let timeout = config
            .get_duration("timeout")
            .unwrap_or(Duration::from_secs(600));
        let args = Self::build_args(config, ctx)?;

        // Parse retry configuration for rate limit handling
        let retry_config = RetryConfig::from_config(config);

        if sandbox.is_some() {
            self.execute_in_sandbox_with_retry(&prompt, command, &args, timeout, sandbox, &retry_config).await
        } else {
            self.execute_on_host_with_retry(&prompt, command, &args, timeout, &retry_config).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepConfig;
    use crate::engine::context::Context;
    use crate::workflow::schema::StepType;
    use std::collections::HashMap;

    fn agent_step(prompt: &str) -> StepDef {
        StepDef {
            name: "test".to_string(),
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
    async fn agent_mock_claude() {
        let mock_script = format!("{}/tests/fixtures/mock_claude.sh", env!("CARGO_MANIFEST_DIR"));

        // Make executable (in case git checkout lost exec bit)
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        let step = agent_step("test prompt");
        let mut values = HashMap::new();
        values.insert(
            "command".to_string(),
            serde_json::Value::String(mock_script.clone()),
        );
        let config = StepConfig { values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = AgentExecutor.execute(&step, &config, &ctx).await.unwrap();
        if let StepOutput::Agent(out) = result {
            assert_eq!(out.response, "Task completed successfully");
            assert_eq!(out.session_id.as_deref(), Some("mock-session-123"));
            assert_eq!(out.stats.input_tokens, 10);
            assert_eq!(out.stats.output_tokens, 20);
        } else {
            panic!("Expected Agent output");
        }
    }

    #[tokio::test]
    async fn resume_missing_step_returns_error() {
        let step = agent_step("test prompt");
        let mut values = HashMap::new();
        values.insert("resume".to_string(), serde_json::Value::String("nonexistent".to_string()));
        let config = StepConfig { values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = AgentExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("session not found for step 'nonexistent'"),
            "Unexpected error: {}",
            err
        );
    }

    #[test]
    fn build_args_resume_adds_flag() {
        use crate::steps::{AgentOutput, AgentStats, StepOutput};

        let mut ctx = Context::new(String::new(), HashMap::new());
        ctx.store(
            "analyze",
            StepOutput::Agent(AgentOutput {
                response: "result".to_string(),
                session_id: Some("sess-123".to_string()),
                stats: AgentStats::default(),
            }),
        );

        let mut values = HashMap::new();
        values.insert("resume".to_string(), serde_json::Value::String("analyze".to_string()));
        let config = StepConfig { values };

        let args = AgentExecutor::build_args(&config, &ctx).unwrap();
        let resume_idx = args.iter().position(|a| a == "--resume").expect("--resume not found");
        assert_eq!(args[resume_idx + 1], "sess-123");
    }

    #[tokio::test]
    async fn fork_session_missing_step_returns_error() {
        let step = agent_step("test prompt");
        let mut values = HashMap::new();
        values.insert(
            "fork_session".to_string(),
            serde_json::Value::String("nonexistent".to_string()),
        );
        let config = StepConfig { values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = AgentExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("session not found for step 'nonexistent'"),
            "Unexpected error: {}",
            err
        );
    }

    #[test]
    fn build_args_fork_session_adds_resume_flag() {
        use crate::steps::{AgentOutput, AgentStats, StepOutput};

        let mut ctx = Context::new(String::new(), HashMap::new());
        ctx.store(
            "analyze",
            StepOutput::Agent(AgentOutput {
                response: "result".to_string(),
                session_id: Some("sess-fork-456".to_string()),
                stats: AgentStats::default(),
            }),
        );

        let mut values = HashMap::new();
        values.insert(
            "fork_session".to_string(),
            serde_json::Value::String("analyze".to_string()),
        );
        let config = StepConfig { values };

        let args = AgentExecutor::build_args(&config, &ctx).unwrap();
        let resume_idx = args.iter().position(|a| a == "--resume").expect("--resume not found");
        assert_eq!(args[resume_idx + 1], "sess-fork-456");
    }

    #[test]
    fn claude_cli_rate_limit_error_detection() {
        assert!(AgentExecutor::is_claude_cli_rate_limit_error("Error: HTTP 429 Too Many Requests"));
        assert!(AgentExecutor::is_claude_cli_rate_limit_error("rate limit exceeded for your key"));
        assert!(AgentExecutor::is_claude_cli_rate_limit_error("Too many requests, please slow down"));
        assert!(AgentExecutor::is_claude_cli_rate_limit_error("Quota exceeded"));
        assert!(!AgentExecutor::is_claude_cli_rate_limit_error("Connection timeout"));
        assert!(!AgentExecutor::is_claude_cli_rate_limit_error("Internal server error"));
    }

    #[tokio::test]
    async fn agent_retry_configuration_from_step_config() {
        let mut values = HashMap::new();
        values.insert("max_retries".to_string(), serde_json::Value::Number(5.into()));
        values.insert("retry_base_delay_ms".to_string(), serde_json::Value::Number(2000.into()));
        values.insert("retry_max_delay_ms".to_string(), serde_json::Value::Number(10000.into()));
        let config = StepConfig { values };

        let retry_config = RetryConfig::from_config(&config);
        assert_eq!(retry_config.max_retries, 5);
        assert_eq!(retry_config.base_delay_ms, 2000);
        assert_eq!(retry_config.max_delay_ms, 10000);
    }

    #[tokio::test]
    async fn agent_mock_claude_rate_limited() {
        // Create a mock script that simulates rate limiting followed by success
        let mock_script_content = r#"#!/bin/bash
# Mock Claude CLI that returns rate limit error on first call, success on second
COUNTER_FILE="/tmp/mock_claude_counter_$$"

if [ ! -f "$COUNTER_FILE" ]; then
    echo "0" > "$COUNTER_FILE"
fi

COUNTER=$(<"$COUNTER_FILE")
COUNTER=$((COUNTER + 1))
echo "$COUNTER" > "$COUNTER_FILE"

if [ "$COUNTER" -eq "1" ]; then
    echo "Error: HTTP 429 Too Many Requests - Rate limit exceeded" >&2
    exit 1
else
    # Success response
    echo '{"type":"result","result":"Task completed after retry","session_id":"mock-retry-session","usage":{"input_tokens":15,"output_tokens":25},"cost_usd":0.001}'
    rm -f "$COUNTER_FILE"
    exit 0
fi
"#;

        let temp_dir = std::env::temp_dir();
        let mock_script = temp_dir.join(format!("mock_claude_retry_{}.sh", std::process::id()));
        std::fs::write(&mock_script, mock_script_content).unwrap();

        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        let step = agent_step("test retry prompt");
        let mut values = HashMap::new();
        values.insert(
            "command".to_string(),
            serde_json::Value::String(mock_script.to_string_lossy().to_string()),
        );
        values.insert("max_retries".to_string(), serde_json::Value::Number(3.into()));
        values.insert("retry_base_delay_ms".to_string(), serde_json::Value::Number(10.into())); // Fast for testing
        let config = StepConfig { values };
        let ctx = Context::new(String::new(), HashMap::new());

        let start_time = std::time::Instant::now();
        let result = AgentExecutor.execute(&step, &config, &ctx).await.unwrap();
        let elapsed = start_time.elapsed();

        if let StepOutput::Agent(out) = result {
            assert_eq!(out.response, "Task completed after retry");
            assert_eq!(out.session_id.as_deref(), Some("mock-retry-session"));
            assert_eq!(out.input_tokens, 15);
            assert_eq!(out.output_tokens, 25);
        } else {
            panic!("Expected Agent output");
        }

        // Should have taken at least 10ms (one retry delay)
        assert!(elapsed >= Duration::from_millis(5), "Should have delayed for retry");

        // Cleanup
        let _ = std::fs::remove_file(&mock_script);
    }

    #[tokio::test]
    async fn agent_sandbox_aware_no_sandbox_uses_host() {
        let mock_script = format!("{}/tests/fixtures/mock_claude.sh", env!("CARGO_MANIFEST_DIR"));

        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&mock_script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_script, perms).unwrap();

        let step = agent_step("test prompt");
        let mut values = HashMap::new();
        values.insert(
            "command".to_string(),
            serde_json::Value::String(mock_script),
        );
        let config = StepConfig { values };
        let ctx = Context::new(String::new(), HashMap::new());

        // With sandbox=None, should fall back to host execution
        let result = AgentExecutor
            .execute_sandboxed(&step, &config, &ctx, &None)
            .await
            .unwrap();
        assert!(matches!(result, StepOutput::Agent(_)));
    }
}
