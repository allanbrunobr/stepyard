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
    call::dispatch_scope_step, CmdOutput, IterationOutput, ScopeOutput, StepExecutor, StepOutput,
};

pub struct MapExecutor {
    scopes: HashMap<String, ScopeDef>,
}

impl MapExecutor {
    pub fn new(scopes: &HashMap<String, ScopeDef>) -> Self {
        Self {
            scopes: scopes.clone(),
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

        if parallel_count == 0 {
            // Serial execution
            serial_execute(items, &scope, ctx, &self.scopes).await
        } else {
            // Parallel execution with semaphore
            parallel_execute(items, &scope, ctx, &self.scopes, parallel_count).await
        }
    }
}

async fn serial_execute(
    items: Vec<String>,
    scope: &ScopeDef,
    ctx: &Context,
    scopes: &HashMap<String, ScopeDef>,
) -> Result<StepOutput, StepError> {
    let mut iterations = Vec::new();

    for (i, item) in items.iter().enumerate() {
        let mut child_ctx = make_child_ctx(ctx, Some(serde_json::Value::String(item.clone())), i);

        let iter_output = execute_scope_steps(scope, &mut child_ctx, scopes).await?;

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
) -> Result<StepOutput, StepError> {
    let sem = Arc::new(Semaphore::new(parallel_count));
    let mut set: JoinSet<(usize, Result<StepOutput, StepError>)> = JoinSet::new();

    for (i, item) in items.iter().enumerate() {
        let sem = Arc::clone(&sem);
        let item_val = serde_json::Value::String(item.clone());
        let child_ctx = make_child_ctx(ctx, Some(item_val), i);
        let scope_clone = scope.clone();
        let scopes_clone = scopes.clone();

        set.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let result = execute_scope_steps_owned(scope_clone, child_ctx, scopes_clone).await;
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
    let mut ctx = Context::new(target, HashMap::new());
    ctx.scope_value = scope_value;
    ctx.scope_index = index;
    ctx
}

async fn execute_scope_steps(
    scope: &ScopeDef,
    child_ctx: &mut Context,
    scopes: &HashMap<String, ScopeDef>,
) -> Result<StepOutput, StepError> {
    let mut last_output = StepOutput::Empty;

    for scope_step in &scope.steps {
        let config = StepConfig::default();
        let result = dispatch_scope_step(scope_step, &config, child_ctx, scopes).await;

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
        match child_ctx.render_template(outputs_template) {
            Ok(rendered) => {
                return Ok(StepOutput::Cmd(CmdOutput {
                    stdout: rendered,
                    stderr: String::new(),
                    exit_code: 0,
                    duration: std::time::Duration::ZERO,
                }));
            }
            Err(_) => {}
        }
    }

    Ok(last_output)
}

async fn execute_scope_steps_owned(
    scope: ScopeDef,
    mut child_ctx: Context,
    scopes: HashMap<String, ScopeDef>,
) -> Result<StepOutput, StepError> {
    execute_scope_steps(&scope, &mut child_ctx, &scopes).await
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
        let executor = MapExecutor::new(&scopes);
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
        let executor = MapExecutor::new(&scopes);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = executor.execute(&step, &config, &ctx).await.unwrap();
        if let StepOutput::Scope(scope_out) = &result {
            assert_eq!(scope_out.iterations.len(), 3);
        } else {
            panic!("Expected Scope output");
        }
    }

    #[tokio::test]
    async fn map_order_preserved_parallel() {
        let mut scopes = HashMap::new();
        scopes.insert("echo_scope".to_string(), echo_scope());

        let step = map_step("map_order", "first\nsecond\nthird", "echo_scope", Some(3));
        let executor = MapExecutor::new(&scopes);
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
