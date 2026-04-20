//! Gate executor — the first non-cmd step kind on the v2 path.
//!
//! PR 2 of Task #31 landed top-level gates with `continue`/`fail`. PR 3
//! widens the action vocabulary to `skip` / `break` for gates that live
//! **inside** a `call` / `repeat` / `map` scope body — those actions do
//! not make sense at the top level (no loop to skip, no scope to break)
//! so they route through a separate [`GateAction::parse_scoped`] entry
//! point and the top-level [`GateAction::parse`] keeps rejecting them.
//! Lives in its own module — not inline in `engine.rs` — to set the
//! pattern for `agent`/`chat`/`script` in later PRs.
//!
//! # PR 3 contract
//!
//! * `condition` is required. A gate without a condition is a validation
//!   error at execution time (the YAML parser itself still admits the
//!   field as optional to keep the data model cmd-compatible).
//! * `on_pass` / `on_fail` default to `"continue"` when absent.
//! * Top-level gates accept `"continue"` and `"fail"` only.
//! * Scoped gates additionally accept `"skip"` (end this iteration) and
//!   `"break"` (end the containing `repeat` / `map`; for `call` a break
//!   ends the scope body like skip — v1 parity). The adapter layer
//!   already rejects `break`/`skip` at the top level with a friendlier
//!   error shape; this module's `parse()` keeps the fallback in place
//!   so a manually-constructed [`Step`](crate::workflow::Step) can't
//!   bypass validation.

/// Action to apply after a gate evaluates.
///
/// String-to-enum conversion splits by gate position: top-level gates go
/// through [`GateAction::parse`] and reject `skip`/`break`; scoped gates
/// go through [`GateAction::parse_scoped`] and accept them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    /// Carry on with the next step (or next scope body step).
    Continue,
    /// Abort the workflow with a failure.
    Fail,
    /// End the current scope iteration early. `repeat`/`map` advance to
    /// the next iteration; `call` treats this as end-of-scope (v1
    /// parity). Only valid for scoped gates.
    Skip,
    /// End the containing scope entirely. `repeat`/`map` exit the loop
    /// and the container completes successfully. Only valid for scoped
    /// gates.
    Break,
}

impl GateAction {
    const ALLOWED_TOP_LEVEL: &'static [&'static str] = &["continue", "fail"];
    const ALLOWED_SCOPED: &'static [&'static str] = &["continue", "fail", "skip", "break"];

    /// Parse a top-level gate's `on_pass` / `on_fail` string, defaulting
    /// to [`GateAction::Continue`] when `None`. Rejects `skip` / `break`
    /// — those are scope-body-only actions (PR 3 of Task #31).
    pub fn parse(raw: Option<&str>) -> Result<Self, GateError> {
        match raw.map(str::trim) {
            None | Some("") | Some("continue") => Ok(Self::Continue),
            Some("fail") => Ok(Self::Fail),
            Some(other) => Err(GateError::UnsupportedAction {
                got: other.to_string(),
                allowed: Self::ALLOWED_TOP_LEVEL,
            }),
        }
    }

    /// Parse a scoped gate's `on_pass` / `on_fail` string. Accepts the
    /// top-level vocabulary plus `skip` and `break`. Scope runners call
    /// this when walking a `call` / `repeat` / `map` body (PR 3 of
    /// Task #31).
    pub fn parse_scoped(raw: Option<&str>) -> Result<Self, GateError> {
        match raw.map(str::trim) {
            None | Some("") | Some("continue") => Ok(Self::Continue),
            Some("fail") => Ok(Self::Fail),
            Some("skip") => Ok(Self::Skip),
            Some("break") => Ok(Self::Break),
            Some(other) => Err(GateError::UnsupportedAction {
                got: other.to_string(),
                allowed: Self::ALLOWED_SCOPED,
            }),
        }
    }
}

/// Structured gate errors. Maps to `StepFailed.error` via `Display`.
#[derive(Debug, thiserror::Error)]
pub enum GateError {
    #[error("gate step `{step}` is missing `condition:`")]
    MissingCondition { step: String },

    #[error(
        "gate action `{got}` not supported (allowed: {})",
        allowed.join(", ")
    )]
    UnsupportedAction {
        got: String,
        allowed: &'static [&'static str],
    },

    #[error(
        "gate step `{step}` condition evaluated to `{value}` — expected \
         a truthy/falsy token (true|false|1|0|yes|no|ok|fail)"
    )]
    NonBooleanCondition { step: String, value: String },

    #[error("gate step `{step}` failed: {message}")]
    Failed { step: String, message: String },
}

/// Resolve a rendered condition string into a boolean.
///
/// Accepts the same vocabulary as the v1 gate (case-insensitive):
/// `true`/`1`/`yes`/`ok` → `true`, `false`/`0`/`no`/`fail` → `false`.
/// Anything else surfaces a [`GateError::NonBooleanCondition`] so a
/// typo or unexpected stdout shape surfaces as a structured failure
/// rather than a silent branch.
pub fn evaluate_bool(step: &str, rendered: &str) -> Result<bool, GateError> {
    match rendered.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "ok" => Ok(true),
        "false" | "0" | "no" | "fail" => Ok(false),
        other => Err(GateError::NonBooleanCondition {
            step: step.to_string(),
            value: other.to_string(),
        }),
    }
}

/// Terminal outcome after applying `on_pass`/`on_fail`.
#[derive(Debug, PartialEq, Eq)]
pub enum GateOutcome {
    /// Continue to the next step (top-level) or next scope body step.
    Continue,
    /// End the current scope iteration early. Scoped gates only — a
    /// top-level gate never produces this outcome because
    /// [`GateAction::parse`] rejects `skip`.
    Skip,
    /// End the containing scope. Scoped gates only — same guardrail as
    /// `Skip`.
    Break,
    /// Abort the workflow. `message` is the operator-visible reason —
    /// either the step's `message:` field (when set) or a generic line
    /// naming the branch that fired.
    Fail { message: String },
}

/// Apply the pass/fail action once the boolean condition is known.
///
/// `message` is the step's `message:` field (optional). Extracted into a
/// pure function so the engine can unit-test branch selection without
/// spinning up a sandbox.
pub fn outcome_for(
    passed: bool,
    on_pass: GateAction,
    on_fail: GateAction,
    message: Option<&str>,
) -> GateOutcome {
    let action = if passed { on_pass } else { on_fail };
    match action {
        GateAction::Continue => GateOutcome::Continue,
        GateAction::Skip => GateOutcome::Skip,
        GateAction::Break => GateOutcome::Break,
        GateAction::Fail => {
            let reason = message
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if passed {
                        "gate passed but on_pass=fail".into()
                    } else {
                        "gate failed".into()
                    }
                });
            GateOutcome::Fail { message: reason }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_parse_defaults_to_continue() {
        assert_eq!(GateAction::parse(None).unwrap(), GateAction::Continue);
        assert_eq!(
            GateAction::parse(Some("")).unwrap(),
            GateAction::Continue
        );
        assert_eq!(
            GateAction::parse(Some("continue")).unwrap(),
            GateAction::Continue
        );
    }

    #[test]
    fn action_parse_accepts_fail() {
        assert_eq!(GateAction::parse(Some("fail")).unwrap(), GateAction::Fail);
    }

    #[test]
    fn action_parse_rejects_break_and_skip_outside_scope() {
        // Top-level gates can't emit skip/break — those actions require a
        // containing scope. Adapter already catches this with a richer
        // error shape; parse() stays the last line of defence so a
        // manually-constructed Step can't slip through.
        for bad in ["break", "skip"] {
            let err = GateAction::parse(Some(bad)).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains(bad) && msg.contains("continue"),
                "expected {bad} rejection to mention allowed set: {msg}"
            );
        }
    }

    #[test]
    fn action_parse_rejects_arbitrary_noise() {
        let err = GateAction::parse(Some("halt")).unwrap_err();
        assert!(err.to_string().contains("halt"));
    }

    #[test]
    fn action_parse_scoped_defaults_to_continue() {
        assert_eq!(
            GateAction::parse_scoped(None).unwrap(),
            GateAction::Continue
        );
        assert_eq!(
            GateAction::parse_scoped(Some("")).unwrap(),
            GateAction::Continue
        );
    }

    #[test]
    fn action_parse_scoped_accepts_skip_and_break() {
        assert_eq!(
            GateAction::parse_scoped(Some("skip")).unwrap(),
            GateAction::Skip
        );
        assert_eq!(
            GateAction::parse_scoped(Some("break")).unwrap(),
            GateAction::Break
        );
        assert_eq!(
            GateAction::parse_scoped(Some("fail")).unwrap(),
            GateAction::Fail
        );
        assert_eq!(
            GateAction::parse_scoped(Some("continue")).unwrap(),
            GateAction::Continue
        );
    }

    #[test]
    fn action_parse_scoped_rejects_arbitrary_noise() {
        let err = GateAction::parse_scoped(Some("explode")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("explode"), "msg={msg}");
        assert!(msg.contains("break"), "scoped msg should list break: {msg}");
    }

    #[test]
    fn evaluate_bool_true_tokens() {
        for tok in ["true", "1", "yes", "ok", "TRUE", "  Yes  "] {
            assert!(evaluate_bool("s", tok).unwrap(), "{tok} should be true");
        }
    }

    #[test]
    fn evaluate_bool_false_tokens() {
        for tok in ["false", "0", "no", "fail", "FALSE"] {
            assert!(!evaluate_bool("s", tok).unwrap(), "{tok} should be false");
        }
    }

    #[test]
    fn evaluate_bool_rejects_non_boolean() {
        let err = evaluate_bool("check", "maybe").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("check"), "msg={msg}");
        assert!(msg.contains("maybe"), "msg={msg}");
    }

    #[test]
    fn outcome_pass_continue() {
        let out = outcome_for(true, GateAction::Continue, GateAction::Fail, None);
        assert_eq!(out, GateOutcome::Continue);
    }

    #[test]
    fn outcome_skip_and_break_route_through() {
        // Scoped gates can configure Skip/Break on either branch — the
        // outcome mirrors the action directly (gates never attach a
        // message to Skip/Break, v1 parity).
        assert_eq!(
            outcome_for(true, GateAction::Skip, GateAction::Continue, None),
            GateOutcome::Skip
        );
        assert_eq!(
            outcome_for(false, GateAction::Continue, GateAction::Break, None),
            GateOutcome::Break
        );
    }

    #[test]
    fn outcome_fail_triggers_failure_with_message() {
        let out = outcome_for(
            false,
            GateAction::Continue,
            GateAction::Fail,
            Some("build must be green"),
        );
        match out {
            GateOutcome::Fail { message } => assert_eq!(message, "build must be green"),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn outcome_fail_without_message_uses_default() {
        let out = outcome_for(false, GateAction::Continue, GateAction::Fail, None);
        match out {
            GateOutcome::Fail { message } => assert_eq!(message, "gate failed"),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn outcome_pass_with_on_pass_fail_uses_distinguishing_default() {
        let out = outcome_for(true, GateAction::Fail, GateAction::Continue, None);
        match out {
            GateOutcome::Fail { message } => {
                assert!(
                    message.contains("on_pass=fail"),
                    "default should name the branch that fired: {message}"
                );
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}
