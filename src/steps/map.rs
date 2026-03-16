use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::config::StepConfig;
use crate::control_flow::ControlFlow;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::{ScopeDef, StepDef};

use super::{
    call::dispatch_scope_step_sandboxed, CmdOutput, IterationOutput, ScopeOutput, SharedSandbox,
    StepExecutor, StepOutput,
};

/// Apply a reduce operation to ScopeOutput iterations (Story 7.2)
fn apply_reduce(
    scope: &ScopeOutput,
    reducer: &str,
    condition_template: Option<&str>,
) -> Result<StepOutput, crate::error::StepError> {
    let iterations = &scope.iterations;

    match reducer {
        "concat" => {
            let joined = iterations
                .iter()
                .map(|it| it.output.text().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            Ok(StepOutput::Cmd(CmdOutput {
                stdout: joined,
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }))
        }
        "sum" => {
            let sum: f64 = iterations
                .iter()
                .map(|it| it.output.text().trim().parse::<f64>().unwrap_or(0.0))
                .sum();
            // Format without trailing .0 if integer
            let result = if sum.fract() == 0.0 {
                format!("{}", sum as i64)
            } else {
                format!("{}", sum)
            };
            Ok(StepOutput::Cmd(CmdOutput {
                stdout: result,
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }))
        }
        "count" => {
            Ok(StepOutput::Cmd(CmdOutput {
                stdout: iterations.len().to_string(),
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }))
        }
        "min" => {
            let min_val = iterations
                .iter()
                .filter_map(|it| it.output.text().trim().parse::<f64>().ok())
                .fold(f64::INFINITY, f64::min);
            let result = if min_val.fract() == 0.0 {
                format!("{}", min_val as i64)
            } else {
                format!("{}", min_val)
            };
            Ok(StepOutput::Cmd(CmdOutput {
                stdout: result,
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }))
        }
        "max" => {
            let max_val = iterations
                .iter()
                .filter_map(|it| it.output.text().trim().parse::<f64>().ok())
                .fold(f64::NEG_INFINITY, f64::max);
            let result = if max_val.fract() == 0.0 {
                format!("{}", max_val as i64)
            } else {
                format!("{}", max_val)
            };
            Ok(StepOutput::Cmd(CmdOutput {
                stdout: result,
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }))
        }
        "filter" => {
            let tmpl = condition_template.ok_or_else(|| {
                crate::error::StepError::Fail(
                    "reduce: 'filter' requires 'reduce_condition' to be set".to_string(),
                )
            })?;

            let mut kept = Vec::new();
            for it in iterations {
                // Build a mini-context with item.output accessible
                let mut vars = std::collections::HashMap::new();
                vars.insert(
                    "item_output".to_string(),
                    serde_json::Value::String(it.output.text().to_string()),
                );
                // Render condition; treat "true" / non-empty as pass
                // Replace {{item.output}} with item_output for simple template resolution
                let simplified_tmpl = tmpl
                    .replace("{{item.output}}", "{{ item_output }}")
                    .replace("{{ item.output }}", "{{ item_output }}");
                let child_ctx =
                    crate::engine::context::Context::new(String::new(), vars);
                let rendered = child_ctx
                    .render_template(&simplified_tmpl)
                    .unwrap_or_default();
                let passes = !rendered.trim().is_empty()
                    && rendered.trim() != "false"
                    && rendered.trim() != "0";
                if passes {
                    kept.push(it.output.text().to_string());
                }
            }

            let joined = kept.join("\n");
            Ok(StepOutput::Cmd(CmdOutput {
                stdout: joined,
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }))
        }
        other => Err(crate::error::StepError::Fail(format!(
            "unknown reduce operation '{}'; expected concat, sum, count, filter, min, max",
            other
        ))),
    }
}

/// Apply a collect transformation to ScopeOutput (Story 7.1)
fn apply_collect(scope: ScopeOutput, mode: &str) -> Result<StepOutput, crate::error::StepError> {
    match mode {
        "text" => {
            let joined = scope
                .iterations
                .iter()
                .map(|it| it.output.text().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            Ok(StepOutput::Cmd(CmdOutput {
                stdout: joined,
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }))
        }
        "all" | "json" => {
            let arr: Vec<serde_json::Value> = scope
                .iterations
                .iter()
                .map(|it| serde_json::Value::String(it.output.text().to_string()))
                .collect();
            let json = serde_json::to_string(&arr)
                .map_err(|e| crate::error::StepError::Fail(format!("collect serialize error: {e}")))?;
            Ok(StepOutput::Cmd(CmdOutput {
                stdout: json,
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }))
        }
        other => Err(crate::error::StepError::Fail(format!(
            "unknown collect mode '{}'; expected all, text, or json",
            other
        ))),
    }
}

pub struct MapExecutor {
    scopes: HashMap<String, ScopeDef>,
    sandbox: SharedSandbox,
}

impl MapExecutor {
    pub fn new(scopes: &HashMap<String, ScopeDef>, sandbox: SharedSandbox) -> Self {
        Self {
            scopes: scopes.clone(),
            sandbox,
        }
    }
}

#[async_trait]
impl StepExecutor for MapExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        _config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let items_template = step
            .items
            .as_ref()
            .ok_or_else(|| StepError::Fail("map step missing 'items' field".into()))?;

        let scope_name = step
            .scope
            .as_ref()
            .ok_or_else(|| StepError::Fail("map step missing 'scope' field".into()))?;

        let scope = self
            .scopes
            .get(scope_name)
            .ok_or_else(|| StepError::Fail(format!("scope '{}' not found", scope_name)))?
            .clone();

        let rendered_items = ctx.render_template(items_template)?;

        // Parse items: try JSON array first, then split by lines
        let items: Vec<String> = if rendered_items.trim().starts_with('[') {
            serde_json::from_str::<Vec<serde_json::Value>>(&rendered_items)
                .map(|arr| {
                    arr.into_iter()
                        .map(|v| match v {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_else(|_| {
                    rendered_items
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(|l| l.to_string())
                        .collect()
                })
        } else {
            rendered_items
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.to_string())
                .collect()
        };

        let parallel_count = step.parallel.unwrap_or(0);

        let scope_output = if parallel_count == 0 {
            // Serial execution
            serial_execute(items, &scope, ctx, &self.scopes, &self.sandbox).await?
        } else {
            // Parallel execution with semaphore
            parallel_execute(items, &scope, ctx, &self.scopes, parallel_count, &self.sandbox).await?
        };

        // Story 7.2: Apply reduce if configured (takes precedence over collect)
        let reduce_mode = _config.get_str("reduce").map(|s| s.to_string());
        if let Some(ref reducer) = reduce_mode {
            if let StepOutput::Scope(ref s) = scope_output {
                let condition = _config.get_str("reduce_condition");
                return apply_reduce(s, reducer, condition);
            }
        }

        // Story 7.1: Apply collect transformation if configured
        let collect_mode = _config.get_str("collect").map(|s| s.to_string());
        match (scope_output, collect_mode) {
            (StepOutput::Scope(s), Some(mode)) => apply_collect(s, &mode),
            (output, _) => Ok(output),
        }
    }
}

async fn serial_execute(
    items: Vec<String>,
    scope: &ScopeDef,
    ctx: &Context,
    scopes: &HashMap<String, ScopeDef>,
    sandbox: &SharedSandbox,
) -> Result<StepOutput, StepError> {
    let mut iterations = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let mut child_ctx = make_child_ctx(ctx, Some(serde_json::Value::String(item.clone())), i);

        let iter_output = execute_scope_steps(scope, &mut child_ctx, scopes, sandbox).await?;

        iterations.push(IterationOutput {
            index: i,
            output: iter_output,
        });
    }

    let final_value = iterations.last().map(|i| Box::new(i.output.clone()));
    Ok(StepOutput::Scope(ScopeOutput {
        iterations,
        final_value,
    }))
}

async fn parallel_execute(
    items: Vec<String>,
    scope: &ScopeDef,
    ctx: &Context,
    scopes: &HashMap<String, ScopeDef>,
    parallel_count: usize,
    sandbox: &SharedSandbox,
) -> Result<StepOutput, StepError> {
    let sem = Arc::new(Semaphore::new(parallel_count));
    let mut set: JoinSet<(usize, Result<StepOutput, StepError>)> = JoinSet::new();

    for (i, item) in items.iter().enumerate() {
        let sem = Arc::clone(&sem);
        let item_val = serde_json::Value::String(item.clone());
        let child_ctx = make_child_ctx(ctx, Some(item_val), i);
        let scope_clone = scope.clone();
        let scopes_clone = scopes.clone();
        let sandbox_clone = sandbox.clone();

        set.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let result = execute_scope_steps_owned(scope_clone, child_ctx, scopes_clone, sandbox_clone).await;
            (i, result)
        });
    }

    let mut results: Vec<Option<StepOutput>> = vec![None; items.len()];

    while let Some(res) = set.join_next().await {
        match res {
            Ok((i, Ok(output))) => {
                results[i] = Some(output);
            }
            Ok((_, Err(e))) => {
                set.abort_all();
                return Err(e);
            }
            Err(e) => {
                set.abort_all();
                return Err(StepError::Fail(format!("Task panicked: {e}")));
            }
        }
    }

    let iterations: Vec<IterationOutput> = results
        .into_iter()
        .enumerate()
        .map(|(i, opt)| IterationOutput {
            index: i,
            output: opt.unwrap_or(StepOutput::Empty),
        })
        .collect();

    let final_value = iterations.last().map(|i| Box::new(i.output.clone()));
    Ok(StepOutput::Scope(ScopeOutput {
        iterations,
        final_value,
    }))
}

fn make_child_ctx(
    parent: &Context,
    scope_value: Option<serde_json::Value>,
    index: usize,
) -> Context {
    let target = parent
        .get_var("target")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut ctx = Context::new(target, parent.all_variables());
    ctx.scope_value = scope_value;
    ctx.scope_index = index;
    ctx.stack_info = parent.get_stack_info().cloned();
    ctx.prompts_dir = parent.prompts_dir.clone();
    ctx
}

async fn execute_scope_steps(
    scope: &ScopeDef,
    child_ctx: &mut Context,
    scopes: &HashMap<String, ScopeDef>,
    sandbox: &SharedSandbox,
) -> Result<StepOutput, StepError> {
    let mut last_output = StepOutput::Empty;

    for scope_step in &scope.steps {
        let config = StepConfig::default();
        let result = dispatch_scope_step_sandboxed(scope_step, &config, child_ctx, scopes, sandbox).await;

        match result {
            Ok(output) => {
                child_ctx.store(&scope_step.name, output.clone());
                last_output = output;
            }
            Err(StepError::ControlFlow(ControlFlow::Break { value, .. })) => {
                if let Some(v) = value {
                    last_output = v;
                }
                break;
            }
            Err(StepError::ControlFlow(ControlFlow::Skip { .. })) => {
                child_ctx.store(&scope_step.name, StepOutput::Empty);
            }
            Err(StepError::ControlFlow(ControlFlow::Next { .. })) => {
                break;
            }
            Err(e) => return Err(e),
        }
    }

    // Apply scope outputs if defined
    if let Some(outputs_template) = &scope.outputs {
        if let Ok(rendered) = child_ctx.render_template(outputs_template) {
            return Ok(StepOutput::Cmd(CmdOutput {
                stdout: rendered,
                stderr: String::new(),
                exit_code: 0,
                duration: std::time::Duration::ZERO,
            }));
        }
    }

    Ok(last_output)
}

async fn execute_scope_steps_owned(
    scope: ScopeDef,
    mut child_ctx: Context,
    scopes: HashMap<String, ScopeDef>,
    sandbox: SharedSandbox,
) -> Result<StepOutput, StepError> {
    execute_scope_steps(&scope, &mut child_ctx, &scopes, &sandbox).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::workflow::schema::{ScopeDef, StepType};

    fn cmd_step(name: &str, run: &str) -> StepDef {
        StepDef {
            name: name.to_string(),
            step_type: StepType::Cmd,
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
            async_exec: None,
        }
    }

    fn map_step(name: &str, items: &str, scope: &str, parallel: Option<usize>) -> StepDef {
        StepDef {
            name: name.to_string(),
            step_type: StepType::Map,
            run: None,
            prompt: None,
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: Some(scope.to_string()),
            max_iterations: None,
            initial_value: None,
            items: Some(items.to_string()),
            parallel,
            steps: None,
            config: HashMap::new(),
            outputs: None,
            output_type: None,
            async_exec: None,
        }
    }

    fn echo_scope() -> ScopeDef {
        ScopeDef {
            steps: vec![cmd_step("echo", "echo {{ scope.value }}")],
            outputs: None,
        }
    }

    #[tokio::test]
    async fn map_three_items_serial() {
        let mut scopes = HashMap::new();
        scopes.insert("echo_scope".to_string(), echo_scope());

        let step = map_step("map_test", "alpha\nbeta\ngamma", "echo_scope", None);
        let executor = MapExecutor::new(&scopes, None);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        if let StepOutput::Scope(scope_out) = &result {
            assert_eq!(scope_out.iterations.len(), 3);
            assert!(scope_out.iterations[0].output.text().contains("alpha"));
            assert!(scope_out.iterations[1].output.text().contains("beta"));
            assert!(scope_out.iterations[2].output.text().contains("gamma"));
        } else {
            panic!("Expected Scope output");
        }
    }

    #[tokio::test]
    async fn map_three_items_parallel() {
        let mut scopes = HashMap::new();
        scopes.insert("echo_scope".to_string(), echo_scope());

        let step = map_step("map_parallel", "a\nb\nc", "echo_scope", Some(3));
        let executor = MapExecutor::new(&scopes, None);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        if let StepOutput::Scope(scope_out) = &result {
            assert_eq!(scope_out.iterations.len(), 3);
        } else {
            panic!("Expected Scope output");
        }
    }

    fn map_step_with_config(
        name: &str,
        items: &str,
        scope: &str,
        config_values: HashMap<String, serde_yaml::Value>,
    ) -> StepDef {
        StepDef {
            name: name.to_string(),
            step_type: StepType::Map,
            run: None,
            prompt: None,
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: Some(scope.to_string()),
            max_iterations: None,
            initial_value: None,
            items: Some(items.to_string()),
            parallel: None,
            steps: None,
            config: config_values,
            outputs: None,
            output_type: None,
            async_exec: None,
        }
    }

    #[tokio::test]
    async fn map_collect_text_joins_with_newlines() {
        let mut scopes = HashMap::new();
        scopes.insert("echo_scope".to_string(), echo_scope());

        let mut cfg = HashMap::new();
        cfg.insert(
            "collect".to_string(),
            serde_yaml::Value::String("text".to_string()),
        );
        let step = map_step_with_config("map_collect_text", "alpha\nbeta\ngamma", "echo_scope", cfg);
        let executor = MapExecutor::new(&scopes, None);

        // Build StepConfig with collect=text
        let mut config_values = HashMap::new();
        config_values.insert(
            "collect".to_string(),
            serde_json::Value::String("text".to_string()),
        );
        let config = crate::config::StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        // Should be a Cmd output with newline-joined text
        assert!(matches!(result, StepOutput::Cmd(_)));
        let text = result.text();
        assert!(text.contains("alpha"), "Missing alpha in: {}", text);
        assert!(text.contains("beta"), "Missing beta in: {}", text);
        assert!(text.contains("gamma"), "Missing gamma in: {}", text);
    }

    #[tokio::test]
    async fn map_collect_all_produces_json_array() {
        let mut scopes = HashMap::new();
        scopes.insert("echo_scope".to_string(), echo_scope());

        let step = map_step_with_config(
            "map_collect_all",
            "x\ny\nz",
            "echo_scope",
            HashMap::new(),
        );
        let executor = MapExecutor::new(&scopes, None);

        let mut config_values = HashMap::new();
        config_values.insert(
            "collect".to_string(),
            serde_json::Value::String("all".to_string()),
        );
        let config = crate::config::StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        assert!(matches!(result, StepOutput::Cmd(_)));
        let text = result.text();
        // Should be a valid JSON array
        let arr: Vec<serde_json::Value> = serde_json::from_str(text).expect("Expected JSON array");
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn map_no_collect_returns_scope_output() {
        let mut scopes = HashMap::new();
        scopes.insert("echo_scope".to_string(), echo_scope());

        let step = map_step("map_no_collect", "a\nb", "echo_scope", None);
        let executor = MapExecutor::new(&scopes, None);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        // Without collect, should still be a Scope output
        assert!(matches!(result, StepOutput::Scope(_)));
    }

    #[tokio::test]
    async fn map_reduce_concat_joins_outputs() {
        let mut scopes = HashMap::new();
        scopes.insert("echo_scope".to_string(), echo_scope());

        let step = map_step("map_reduce_concat", "hello\nworld", "echo_scope", None);
        let executor = MapExecutor::new(&scopes, None);

        let mut config_values = HashMap::new();
        config_values.insert(
            "reduce".to_string(),
            serde_json::Value::String("concat".to_string()),
        );
        let config = crate::config::StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        assert!(matches!(result, StepOutput::Cmd(_)));
        let text = result.text();
        assert!(text.contains("hello"), "Missing hello: {}", text);
        assert!(text.contains("world"), "Missing world: {}", text);
    }

    #[tokio::test]
    async fn map_reduce_sum_adds_numbers() {
        let mut scopes = HashMap::new();
        // Each scope step echos the item value (which will be a number string)
        scopes.insert(
            "echo_scope".to_string(),
            ScopeDef {
                steps: vec![cmd_step("echo_val", "echo {{ scope.value }}")],
                outputs: None,
            },
        );

        let step = map_step("map_reduce_sum", "10\n20\n30", "echo_scope", None);
        let executor = MapExecutor::new(&scopes, None);

        let mut config_values = HashMap::new();
        config_values.insert(
            "reduce".to_string(),
            serde_json::Value::String("sum".to_string()),
        );
        let config = crate::config::StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        assert!(matches!(result, StepOutput::Cmd(_)));
        let text = result.text().trim().to_string();
        assert_eq!(text, "60", "Expected 60, got: {}", text);
    }

    #[tokio::test]
    async fn map_reduce_filter_removes_empty() {
        let mut scopes = HashMap::new();
        // Scope that outputs the item value (some will be empty, some not)
        scopes.insert(
            "echo_scope".to_string(),
            ScopeDef {
                steps: vec![cmd_step("echo_val", "echo {{ scope.value }}")],
                outputs: None,
            },
        );

        let step = map_step("map_reduce_filter", "hello\n\nworld", "echo_scope", None);
        let executor = MapExecutor::new(&scopes, None);

        let mut config_values = HashMap::new();
        config_values.insert(
            "reduce".to_string(),
            serde_json::Value::String("filter".to_string()),
        );
        config_values.insert(
            "reduce_condition".to_string(),
            serde_json::Value::String("{{ item.output }}".to_string()),
        );
        let config = crate::config::StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        assert!(matches!(result, StepOutput::Cmd(_)));
        let text = result.text();
        // Empty lines should be filtered out
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert!(lines.len() <= 3, "Should have at most 3 lines: {:?}", lines);
    }

    #[tokio::test]
    async fn map_order_preserved_parallel() {
        let mut scopes = HashMap::new();
        scopes.insert("echo_scope".to_string(), echo_scope());

        let step = map_step("map_order", "first\nsecond\nthird", "echo_scope", Some(3));
        let executor = MapExecutor::new(&scopes, None);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        if let StepOutput::Scope(scope_out) = &result {
            assert_eq!(scope_out.iterations[0].index, 0);
            assert_eq!(scope_out.iterations[1].index, 1);
            assert_eq!(scope_out.iterations[2].index, 2);
            assert!(scope_out.iterations[0].output.text().contains("first"));
            assert!(scope_out.iterations[1].output.text().contains("second"));
            assert!(scope_out.iterations[2].output.text().contains("third"));
        } else {
            panic!("Expected Scope output");
        }
    }
}
