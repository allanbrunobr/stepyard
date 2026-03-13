use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::process::Command;

use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::StepDef;

use super::{CmdOutput, SandboxAwareExecutor, SharedSandbox, StepExecutor, StepOutput};

pub struct CmdExecutor;

#[async_trait]
impl StepExecutor for CmdExecutor {
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
impl SandboxAwareExecutor for CmdExecutor {
    async fn execute_sandboxed(
        &self,
        step: &StepDef,
        config: &StepConfig,
        ctx: &Context,
        sandbox: &SharedSandbox,
    ) -> Result<StepOutput, StepError> {
        let run_template = step
            .run
            .as_ref()
            .ok_or_else(|| StepError::Fail("cmd step missing 'run' field".into()))?;

        let command = ctx.render_template(run_template)?;
        let timeout = config
            .get_duration("timeout")
            .unwrap_or(Duration::from_secs(60));
        let fail_on_error = config.get_bool("fail_on_error");

        let start = Instant::now();

        // ── Sandbox path: run inside Docker container ─────────────────────
        let result = if let Some(sb) = sandbox {
            let sb_guard = sb.lock().await;
            let sb_output = tokio::time::timeout(timeout, sb_guard.run_command(&command))
                .await
                .map_err(|_| StepError::Timeout(timeout))?
                .map_err(|e| StepError::Fail(format!("Sandbox command failed: {e}")))?;

            CmdOutput {
                stdout: sb_output.stdout,
                stderr: sb_output.stderr,
                exit_code: sb_output.exit_code,
                duration: start.elapsed(),
            }
        } else {
            // ── Host path: run directly on host ───────────────────────────
            let shell = config.get_str("shell").unwrap_or("/bin/bash");
            let working_dir = config.get_str("working_directory").map(String::from);

            let mut cmd = Command::new(shell);
            cmd.arg("-c").arg(&command);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());

            if let Some(dir) = &working_dir {
                cmd.current_dir(dir);
            }

            let output = tokio::time::timeout(timeout, cmd.output())
                .await
                .map_err(|_| StepError::Timeout(timeout))?
                .map_err(|e| StepError::Fail(format!("Failed to spawn command: {e}")))?;

            CmdOutput {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code().unwrap_or(-1),
                duration: start.elapsed(),
            }
        };

        if fail_on_error && result.exit_code != 0 {
            return Err(StepError::Fail(format!(
                "Command failed (exit {}): {}",
                result.exit_code,
                result.stderr.trim()
            )));
        }

        Ok(StepOutput::Cmd(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn empty_step(run: &str) -> StepDef {
        StepDef {
            name: "test".to_string(),
            step_type: crate::workflow::schema::StepType::Cmd,
            run: Some(run.to_string()),
            prompt: None,
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
    async fn cmd_echo() {
        let step = empty_step("echo hello");
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = CmdExecutor.execute(&step, &config, &ctx).await.unwrap();
        assert_eq!(result.text().trim(), "hello");
        assert_eq!(result.exit_code(), 0);
    }

    #[tokio::test]
    async fn cmd_echo_via_sandbox_aware_no_sandbox() {
        // When sandbox is None, SandboxAwareExecutor falls back to host execution
        let step = empty_step("echo sandbox_test");
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = CmdExecutor
            .execute_sandboxed(&step, &config, &ctx, &None)
            .await
            .unwrap();
        assert_eq!(result.text().trim(), "sandbox_test");
    }

    #[tokio::test]
    async fn cmd_exit_nonzero_without_fail_on_error() {
        let step = empty_step("exit 42");
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = CmdExecutor.execute(&step, &config, &ctx).await.unwrap();
        assert_eq!(result.exit_code(), 42);
    }

    #[tokio::test]
    async fn cmd_exit_nonzero_with_fail_on_error() {
        let step = empty_step("exit 1");
        let mut values = HashMap::new();
        values.insert(
            "fail_on_error".to_string(),
            serde_json::Value::Bool(true),
        );
        let config = StepConfig { values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = CmdExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cmd_timeout() {
        let step = empty_step("sleep 10");
        let mut values = HashMap::new();
        values.insert(
            "timeout".to_string(),
            serde_json::Value::String("100ms".to_string()),
        );
        let config = StepConfig { values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = CmdExecutor.execute(&step, &config, &ctx).await;
        assert!(matches!(result, Err(crate::error::StepError::Timeout(_))));
    }

    #[tokio::test]
    async fn cmd_working_directory() {
        let step = empty_step("pwd");
        let mut values = HashMap::new();
        values.insert(
            "working_directory".to_string(),
            serde_json::Value::String("/tmp".to_string()),
        );
        let config = StepConfig { values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = CmdExecutor.execute(&step, &config, &ctx).await.unwrap();
        // /tmp resolves to /private/tmp on macOS, so check contains "tmp"
        assert!(result.text().contains("tmp"));
    }
}
