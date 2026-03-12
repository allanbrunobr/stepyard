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

use super::{AgentOutput, AgentStats, StepExecutor, StepOutput};

pub struct AgentExecutor;

#[async_trait]
impl StepExecutor for AgentExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        config: &StepConfig,
        ctx: &Context,
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

        let mut child = Command::new(command)
            .args(&args)
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
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) {
                    match msg.get("type").and_then(|t| t.as_str()) {
                        Some("result") => {
                            if let Some(r) = msg.get("result").and_then(|r| r.as_str()) {
                                response = r.to_string();
                            }
                            session_id =
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
}
