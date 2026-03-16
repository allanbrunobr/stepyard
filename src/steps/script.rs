use std::cell::RefCell;
use std::collections::HashMap;

use async_trait::async_trait;
use rhai::{Dynamic, Engine as RhaiEngine, EvalAltResult, Scope};

use crate::config::StepConfig;
use crate::engine::context::Context;
use crate::error::StepError;
use crate::workflow::schema::StepDef;

use super::{CmdOutput, StepExecutor, StepOutput};

/// Maximum number of Rhai operations before timeout (prevents infinite loops)
const MAX_OPERATIONS: u64 = 1_000_000;

pub struct ScriptExecutor;

#[async_trait]
impl StepExecutor for ScriptExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        _config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let script = step
            .run
            .as_ref()
            .ok_or_else(|| StepError::Fail("script step missing 'run' field".into()))?
            .clone();

        // Build a flat snapshot of context values for ctx_get access
        let ctx_snapshot = build_ctx_snapshot(ctx);

        // Build Rhai engine with operation limit
        let mut engine = RhaiEngine::new();
        engine.set_max_operations(MAX_OPERATIONS);

        // Register ctx_get(key) — reads from the context snapshot
        let snapshot = ctx_snapshot.clone();
        engine.register_fn("ctx_get", move |key: &str| -> Dynamic {
            match snapshot.get(key) {
                Some(v) => json_to_dynamic(v),
                None => Dynamic::UNIT,
            }
        });

        // Register ctx_set(key, value) — writes to thread_local storage
        thread_local! {
            static CTX_WRITES: RefCell<HashMap<String, serde_json::Value>> =
                RefCell::new(HashMap::new());
        }
        CTX_WRITES.with(|w| w.borrow_mut().clear());

        engine.register_fn("ctx_set", |key: &str, value: Dynamic| {
            let json_val = dynamic_to_json(&value);
            CTX_WRITES.with(|w| w.borrow_mut().insert(key.to_string(), json_val));
        });

        // Evaluate the script synchronously
        let mut scope = Scope::new();
        let result = engine.eval_with_scope::<Dynamic>(&mut scope, &script);

        let output_text = match result {
            Ok(val) => dynamic_to_string(&val),
            Err(e) => {
                return Err(StepError::Fail(format_rhai_error(&e)));
            }
        };

        Ok(StepOutput::Cmd(CmdOutput {
            stdout: output_text,
            stderr: String::new(),
            exit_code: 0,
            duration: std::time::Duration::ZERO,
        }))
    }
}

/// Build a flat key-value snapshot from the context for ctx_get access.
/// Uses the tera context's `get()` API to extract step outputs.
fn build_ctx_snapshot(ctx: &Context) -> HashMap<String, serde_json::Value> {
    let tera_ctx = ctx.to_tera_context();
    let mut flat: HashMap<String, serde_json::Value> = HashMap::new();

    // Extract steps map: "step_name.field" => value
    if let Some(serde_json::Value::Object(steps_map)) = tera_ctx.get("steps") {
        for (step_name, step_val) in steps_map {
            if let serde_json::Value::Object(fields) = step_val {
                for (field, field_val) in fields {
                    flat.insert(format!("{}.{}", step_name, field), field_val.clone());
                }
            }
        }
    }

    // Extract top-level variables
    if let Some(target) = tera_ctx.get("target") {
        flat.insert("target".to_string(), target.clone());
    }

    flat
}

/// Convert a serde_json::Value to a Rhai Dynamic
fn json_to_dynamic(val: &serde_json::Value) -> Dynamic {
    match val {
        serde_json::Value::Null => Dynamic::UNIT,
        serde_json::Value::Bool(b) => Dynamic::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                Dynamic::from(f)
            } else {
                Dynamic::UNIT
            }
        }
        serde_json::Value::String(s) => Dynamic::from(s.clone()),
        serde_json::Value::Array(arr) => {
            let v: rhai::Array = arr.iter().map(json_to_dynamic).collect();
            Dynamic::from(v)
        }
        serde_json::Value::Object(obj) => {
            let mut map = rhai::Map::new();
            for (k, v) in obj {
                map.insert(k.clone().into(), json_to_dynamic(v));
            }
            Dynamic::from(map)
        }
    }
}

/// Convert a Rhai Dynamic to a serde_json::Value
fn dynamic_to_json(val: &Dynamic) -> serde_json::Value {
    if val.is_unit() {
        serde_json::Value::Null
    } else if let Some(b) = val.clone().try_cast::<bool>() {
        serde_json::Value::Bool(b)
    } else if let Some(i) = val.clone().try_cast::<i64>() {
        serde_json::json!(i)
    } else if let Some(f) = val.clone().try_cast::<f64>() {
        serde_json::json!(f)
    } else if let Some(s) = val.clone().try_cast::<String>() {
        serde_json::Value::String(s)
    } else {
        serde_json::Value::String(val.to_string())
    }
}

/// Convert a Rhai Dynamic to a display string (script return value → step output text)
fn dynamic_to_string(val: &Dynamic) -> String {
    if val.is_unit() {
        String::new()
    } else if let Some(s) = val.clone().try_cast::<String>() {
        s
    } else {
        val.to_string()
    }
}

/// Format a Rhai evaluation error with line number info when available
fn format_rhai_error(e: &EvalAltResult) -> String {
    format!("Script error: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StepConfig;
    use crate::workflow::schema::StepType;

    fn script_step(name: &str, run: &str) -> StepDef {
        StepDef {
            name: name.to_string(),
            step_type: StepType::Script,
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

    #[tokio::test]
    async fn script_returns_integer_expression() {
        let step = script_step("s", "40 + 2");
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = ScriptExecutor.execute(&step, &config, &ctx).await.unwrap();
        assert_eq!(result.text().trim(), "42");
    }

    #[tokio::test]
    async fn script_returns_string_value() {
        let step = script_step("s", r#""hello from rhai""#);
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = ScriptExecutor.execute(&step, &config, &ctx).await.unwrap();
        assert_eq!(result.text(), "hello from rhai");
    }

    #[tokio::test]
    async fn script_runtime_error_returns_step_error() {
        let step = script_step("s", "throw \"oops\";");
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());

        let result = ScriptExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Script error") || err.contains("oops"),
            "Got: {err}"
        );
    }

    #[tokio::test]
    async fn script_ctx_get_reads_step_output() {
        use crate::steps::{CmdOutput, StepOutput};
        use std::time::Duration;

        let mut ctx = Context::new(String::new(), HashMap::new());
        ctx.store(
            "prev",
            StepOutput::Cmd(CmdOutput {
                stdout: "hello_world".to_string(),
                stderr: String::new(),
                exit_code: 0,
                duration: Duration::ZERO,
            }),
        );

        let step = script_step("s", r#"let v = ctx_get("prev.stdout"); v"#);
        let config = StepConfig::default();

        let result = ScriptExecutor.execute(&step, &config, &ctx).await.unwrap();
        assert_eq!(result.text(), "hello_world");
    }

    #[tokio::test]
    async fn script_missing_run_field_returns_error() {
        let step = StepDef {
            name: "s".to_string(),
            step_type: StepType::Script,
            run: None,
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
        };
        let config = StepConfig::default();
        let ctx = Context::new(String::new(), HashMap::new());
        let result = ScriptExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing 'run'"));
    }
}
