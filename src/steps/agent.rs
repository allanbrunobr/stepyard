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

use super::{AgentOutput, AgentStats, SandboxAwareExecutor, SharedSandbox, StepExecutor, StepOutput};

pub struct AgentExecutor;

impl AgentExecutor {
    /// Build the claude CLI args from step config
    fn build_args(config: &StepConfig) -> Vec<String> {
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

        args
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
        let sandbox_cmd = format!(
            "echo '{}' | {} {}",
            escaped_prompt, command, args_str
        );

        let sb_guard = sb.lock().await;
        let sb_output = tokio::time::timeout(timeout, sb_guard.run_command(&sandbox_cmd))
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
        let args = Self::build_args(config);

        if sandbox.is_some() {
            self.execute_in_sandbox(&prompt, command, &args, timeout, sandbox).await
        } else {
            self.execute_on_host(&prompt, command, &args, timeout).await
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
