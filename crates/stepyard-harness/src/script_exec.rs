//! Script step executor.
//!
//! PR 4 of Task #31 (commit 2). A `script` step evaluates a Rhai expression
//! against a flat snapshot of the harness render context and emits the
//! return value as the step's `stdout` in a `StepCompleted` event.
//! No sandbox, no filesystem / process APIs exposed — pure in-process
//! evaluation, replay-safe via the log like every other step.
//!
//! # Scope
//!
//! * Operation limit: [`MAX_OPERATIONS`] (`1_000_000`) matches v1
//!   (`src/steps/script.rs:15`). An infinite loop or runaway computation
//!   terminates with a Rhai `TooManyOperations` error, surfaced as a
//!   structured [`ScriptExecError::Eval`].
//! * Output shape is unified with cmd: `{ stdout, stderr: "", exit_code: 0 }`.
//!   Cross-step refs (`{{ steps.sc.stdout }}`) resolve with no new variant.
//! * Context is read-only via `ctx_get("step.field")` and `ctx_get("target")`.
//!   v1 also exposed `ctx_set`; PR 4 deliberately drops it (the scope runner
//!   and engine do not yet consume a per-step key/value side-channel).
//! * Host APIs intentionally NOT registered: filesystem (`open_file`,
//!   `read_file`), process (`system`, `exec`), network. The default Rhai
//!   engine does not expose these; this module does not add them.

use std::collections::HashMap;

use rhai::{Dynamic, Engine as RhaiEngine, EvalAltResult, Scope};
use stepyard_core::StepOutputSnapshot;

/// Hard cap on Rhai operations per script, v1 parity
/// (`src/steps/script.rs:15`). Prevents runaway scripts from blocking
/// the harness loop indefinitely.
pub(crate) const MAX_OPERATIONS: u64 = 1_000_000;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ScriptExecError {
    #[error("script step `{step_name}` is missing its `run:` source")]
    MissingSource { step_name: String },

    #[error("script error: {0}")]
    Eval(String),
}

/// Evaluate a Rhai `source` against a snapshot built from `outputs` and
/// `target`. Returns the script's final value rendered as a string.
pub(crate) fn execute_script(
    step_name: &str,
    source: &str,
    outputs: &HashMap<String, StepOutputSnapshot>,
    target: &str,
) -> Result<String, ScriptExecError> {
    if source.trim().is_empty() {
        return Err(ScriptExecError::MissingSource {
            step_name: step_name.to_string(),
        });
    }

    let snapshot = build_ctx_snapshot(outputs, target);

    let mut engine = RhaiEngine::new();
    engine.set_max_operations(MAX_OPERATIONS);

    // `ctx_get(key)` — read-only context lookup. Returns `Dynamic::UNIT`
    // on miss so scripts can guard with `== ()`.
    engine.register_fn("ctx_get", move |key: &str| -> Dynamic {
        snapshot
            .get(key)
            .map(json_to_dynamic)
            .unwrap_or(Dynamic::UNIT)
    });

    let mut scope = Scope::new();
    let result = engine.eval_with_scope::<Dynamic>(&mut scope, source);

    match result {
        Ok(val) => Ok(dynamic_to_string(&val)),
        Err(e) => Err(ScriptExecError::Eval(format_rhai_error(&e))),
    }
}

/// Flatten the harness render inputs to `ctx_get`-shaped keys. Matches v1
/// (`src/steps/script.rs:83-104`): `steps.X.stdout` → `"X.stdout"`, and
/// `target` stays top-level. Vars are deliberately absent — v1 did not
/// expose them either, and the template step is the entry point for
/// workflow-level variables.
fn build_ctx_snapshot(
    outputs: &HashMap<String, StepOutputSnapshot>,
    target: &str,
) -> HashMap<String, serde_json::Value> {
    let mut flat: HashMap<String, serde_json::Value> = HashMap::new();
    for (name, snap) in outputs {
        flat.insert(format!("{name}.stdout"), serde_json::Value::String(snap.stdout.clone()));
        flat.insert(format!("{name}.stderr"), serde_json::Value::String(snap.stderr.clone()));
        flat.insert(
            format!("{name}.exit_code"),
            serde_json::json!(snap.exit_code),
        );
    }
    flat.insert("target".into(), serde_json::Value::String(target.to_string()));
    flat
}

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

fn dynamic_to_string(val: &Dynamic) -> String {
    if val.is_unit() {
        String::new()
    } else if let Some(s) = val.clone().try_cast::<String>() {
        s
    } else {
        val.to_string()
    }
}

fn format_rhai_error(e: &EvalAltResult) -> String {
    format!("Script error: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(stdout: &str) -> StepOutputSnapshot {
        StepOutputSnapshot {
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: 0,
        }
    }

    #[test]
    fn returns_integer_expression_as_string() {
        let out = execute_script("s", "40 + 2", &HashMap::new(), "").unwrap();
        assert_eq!(out.trim(), "42");
    }

    #[test]
    fn returns_string_value_verbatim() {
        let out = execute_script("s", r#""hello from rhai""#, &HashMap::new(), "").unwrap();
        assert_eq!(out, "hello from rhai");
    }

    #[test]
    fn unit_return_value_is_empty_string() {
        // `let x = 1;` leaves the block with `()` as the final value.
        let out = execute_script("s", "let x = 1;", &HashMap::new(), "").unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn runtime_error_is_structured() {
        let err = execute_script("s", r#"throw "oops""#, &HashMap::new(), "").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Script error") && msg.contains("oops"),
            "error should wrap the Rhai message: {msg}"
        );
    }

    #[test]
    fn missing_source_is_structured() {
        let err = execute_script("sc", "   ", &HashMap::new(), "").unwrap_err();
        assert!(matches!(err, ScriptExecError::MissingSource { step_name } if step_name == "sc"));
    }

    #[test]
    fn ctx_get_reads_prior_step_stdout() {
        let mut outputs = HashMap::new();
        outputs.insert("prev".into(), snap("hello_world"));
        let out = execute_script(
            "s",
            r#"let v = ctx_get("prev.stdout"); v"#,
            &outputs,
            "",
        )
        .unwrap();
        assert_eq!(out, "hello_world");
    }

    #[test]
    fn ctx_get_reads_prior_step_exit_code() {
        let mut outputs = HashMap::new();
        outputs.insert(
            "prev".into(),
            StepOutputSnapshot {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 7,
            },
        );
        let out =
            execute_script("s", r#"ctx_get("prev.exit_code")"#, &outputs, "").unwrap();
        assert_eq!(out, "7");
    }

    #[test]
    fn ctx_get_reads_target() {
        let out = execute_script("s", r#"ctx_get("target")"#, &HashMap::new(), "edenred").unwrap();
        assert_eq!(out, "edenred");
    }

    #[test]
    fn ctx_get_missing_key_returns_unit() {
        // Unit cast to string → "" per `dynamic_to_string`.
        let out =
            execute_script("s", r#"ctx_get("nope.nada")"#, &HashMap::new(), "").unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn max_operations_terminates_infinite_loop() {
        // `while true {}` blows through the 1M cap quickly. Rhai reports
        // `TooManyOperations`; our error wrapper surfaces that text.
        let err = execute_script("s", "loop { }", &HashMap::new(), "").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("operations") || msg.to_lowercase().contains("too many"),
            "expected an operation-limit error, got: {msg}"
        );
    }

    #[test]
    fn ctx_set_function_is_not_registered() {
        // v1 exposed `ctx_set`; PR 4 deliberately drops it. The call must
        // fail at eval time (unknown function), not silently succeed.
        let err =
            execute_script("s", r#"ctx_set("k", 1)"#, &HashMap::new(), "").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("function") || msg.contains("ctx_set"),
            "expected an unknown-function error, got: {msg}"
        );
    }

    #[test]
    fn no_filesystem_api_is_reachable() {
        // The default Rhai engine does not expose file I/O; we register
        // only `ctx_get`. Any filesystem-looking function call must fail
        // at eval time. This guards against future drift where someone
        // adds a convenience helper that opens a path.
        let err = execute_script("s", r#"open_file("/etc/passwd")"#, &HashMap::new(), "")
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("function") || msg.contains("open_file"),
            "filesystem function must not be registered, got: {msg}"
        );
    }
}
