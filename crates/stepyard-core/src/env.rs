//! Round 3 Story 3 — env key/value validation at the workflow/defaults boundary.
//!
//! Two grammars, both pinned by `architecture.md` §G env hardening:
//!
//! * **Key:** `^[A-Za-z_][A-Za-z0-9_]*$` (POSIX-shell-portable identifier).
//!   No hyphens, no dots, no leading digits, no empty keys.
//! * **Value:** arbitrary UTF-8 except the NUL byte (`\0`). NUL would
//!   truncate the value mid-stream when handed to a child process via
//!   `execve`, so it is rejected at the same parse boundary as the key.
//!
//! Failures wrap into [`EngineError::InvalidWorkflowField`] with a
//! best-effort serde-style path (e.g. `env.BAD-KEY`,
//! `steps[2].env.FOO`, `scopes.work.steps[1].env.FOO`,
//! `.stepyard/defaults.yaml:env.KEY`). The boundary is the first place
//! the workflow object is materialized — `Workflow::try_from_yaml`,
//! `Workflow::validate` (called from the binary adapter), and the
//! defaults call-site in `commands.rs`.
//!
//! ## `got` redaction for value failures
//!
//! On a key failure we surface the offending key verbatim (it must be
//! valid identifier-shaped, so leakage risk is bounded). On a value
//! failure we replace the value with the fixed string
//! `"<redacted: contains NUL byte>"` — env values can carry secrets
//! (API tokens, signing keys), and even leaking the byte length
//! provides a fingerprint. The operator locates the offender via the
//! key embedded in `path`.

use std::collections::HashMap;

use crate::error::EngineError;

/// Stable `expected` phrase for an env-key failure. Pinned as a
/// `&'static str` because [`EngineError::InvalidWorkflowField::expected`]
/// is `&'static str` (avoids per-call allocation, makes error messages
/// stable for snapshot tests).
pub const EXPECTED_KEY: &str = "env key matching ^[A-Za-z_][A-Za-z0-9_]*$";

/// Stable `expected` phrase for an env-value failure.
pub const EXPECTED_VALUE: &str = "env value (UTF-8 without NUL)";

/// The redacted `got` payload surfaced when a value contains a NUL.
/// Fixed string — does NOT leak length, position, or any partial
/// content, matching the Round 3 env-value confidentiality rule.
pub const REDACTED_NUL_VALUE: &str = "<redacted: contains NUL byte>";

/// True when `key` is a POSIX-shell-portable identifier:
/// first char `[A-Za-z_]`, remaining chars `[A-Za-z0-9_]`.
///
/// Empty string returns `false` — a missing leading char fails the
/// grammar by construction.
pub fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// True when `value` is acceptable env content — UTF-8 (already
/// guaranteed by `&str`) without an interior NUL byte.
pub fn is_valid_env_value(value: &str) -> bool {
    !value.contains('\0')
}

/// Walk an env map and return the first invalid entry as
/// [`EngineError::InvalidWorkflowField`], or `Ok(())` when every entry
/// passes both grammars.
///
/// `path_prefix` is the serde-style prefix appended with `.<KEY>` —
/// e.g. `"env"` for top-level workflow env, `"steps[2].env"` for the
/// third step, `".stepyard/defaults.yaml:env"` for the defaults
/// loader call-site.
///
/// Iteration order is deterministic (keys sorted lexicographically) so
/// tests can pin the path of the first reported offender without
/// depending on `HashMap`'s seed.
pub fn validate_env_map(
    path_prefix: &str,
    env: &HashMap<String, String>,
) -> Result<(), EngineError> {
    let mut keys: Vec<&String> = env.keys().collect();
    keys.sort();
    for key in keys {
        if !is_valid_env_key(key) {
            return Err(EngineError::InvalidWorkflowField {
                path: format!("{path_prefix}.{key}"),
                got: format!("`{key}`"),
                expected: EXPECTED_KEY,
            });
        }
        let value = &env[key];
        if !is_valid_env_value(value) {
            return Err(EngineError::InvalidWorkflowField {
                path: format!("{path_prefix}.{key}"),
                got: REDACTED_NUL_VALUE.to_string(),
                expected: EXPECTED_VALUE,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_keys_pass() {
        assert!(is_valid_env_key("FOO"));
        assert!(is_valid_env_key("foo_bar"));
        assert!(is_valid_env_key("_LEADING_UNDER"));
        assert!(is_valid_env_key("a"));
        assert!(is_valid_env_key("_"));
        assert!(is_valid_env_key("MIXED_123_CASE"));
    }

    #[test]
    fn invalid_keys_fail() {
        assert!(!is_valid_env_key(""), "empty must fail");
        assert!(!is_valid_env_key("1FOO"), "leading digit must fail");
        assert!(!is_valid_env_key("BAD-KEY"), "hyphen must fail");
        assert!(!is_valid_env_key("WITH SPACE"), "space must fail");
        assert!(!is_valid_env_key("DOT.KEY"), "dot must fail");
        assert!(!is_valid_env_key("ÜNICODE"), "non-ASCII must fail");
        assert!(!is_valid_env_key("FOO="), "equals must fail");
    }

    #[test]
    fn valid_values_pass() {
        assert!(is_valid_env_value(""));
        assert!(is_valid_env_value("plain"));
        assert!(is_valid_env_value("with spaces and = signs"));
        assert!(is_valid_env_value("ünicode ✓ utf8 🔑"));
    }

    #[test]
    fn nul_in_value_fails() {
        assert!(!is_valid_env_value("\0"));
        assert!(!is_valid_env_value("prefix\0suffix"));
        assert!(!is_valid_env_value("trailing\0"));
    }

    #[test]
    fn validate_env_map_ok_on_clean_input() {
        let mut env = HashMap::new();
        env.insert("FOO".into(), "bar".into());
        env.insert("BAZ_QUX".into(), "".into());
        env.insert("_LEADING".into(), "value with spaces".into());
        validate_env_map("env", &env).expect("clean input must pass");
    }

    #[test]
    fn validate_env_map_reports_bad_key_with_path_and_constants() {
        let mut env = HashMap::new();
        env.insert("BAD-KEY".into(), "ok".into());

        let err = validate_env_map("env", &env).expect_err("hyphen must fail");
        match err {
            EngineError::InvalidWorkflowField {
                path,
                got,
                expected,
            } => {
                assert_eq!(path, "env.BAD-KEY", "path must embed the offending key");
                assert!(
                    got.contains("BAD-KEY"),
                    "got must surface the bad key, got {got:?}"
                );
                assert_eq!(
                    expected, EXPECTED_KEY,
                    "expected must be the stable EXPECTED_KEY constant"
                );
            }
            other => panic!("expected InvalidWorkflowField, got {other:?}"),
        }
    }

    #[test]
    fn validate_env_map_reports_nul_value_with_redacted_got() {
        let mut env = HashMap::new();
        env.insert("API_TOKEN".into(), "sk_live_x\0LEAK".into());

        let err = validate_env_map("env", &env).expect_err("NUL must fail");
        match err {
            EngineError::InvalidWorkflowField {
                path,
                got,
                expected,
            } => {
                assert_eq!(
                    path, "env.API_TOKEN",
                    "path must embed the (valid) key so operator can locate the offender"
                );
                assert_eq!(
                    got, REDACTED_NUL_VALUE,
                    "got MUST be the fixed redacted string — no length, no prefix, no index"
                );
                // Defense-in-depth: scan the redacted payload itself for any
                // hint of the original. A future refactor that accidentally
                // includes the byte index or a slice of the value would fail
                // these.
                assert!(
                    !got.contains("sk_live"),
                    "redacted got must not leak any prefix of the value, got {got:?}"
                );
                assert!(
                    !got.contains("LEAK"),
                    "redacted got must not leak any suffix of the value, got {got:?}"
                );
                assert!(
                    !got.contains(char::from(0)),
                    "redacted got must not pass through the NUL byte itself"
                );
                assert!(
                    !got.chars().any(|c| c.is_ascii_digit()),
                    "redacted got must not encode the byte length or NUL index, got {got:?}"
                );
                assert_eq!(
                    expected, EXPECTED_VALUE,
                    "expected must be the stable EXPECTED_VALUE constant"
                );
            }
            other => panic!("expected InvalidWorkflowField, got {other:?}"),
        }
    }

    #[test]
    fn validate_env_map_iteration_is_deterministic() {
        // Two bad keys: deterministic sort means the alphabetically
        // first one (`AA-BAD`) is always reported first, regardless of
        // HashMap insertion order or seed.
        let mut env = HashMap::new();
        env.insert("ZZ-BAD".into(), "ok".into());
        env.insert("AA-BAD".into(), "ok".into());

        let err = validate_env_map("env", &env).expect_err("two bad keys");
        if let EngineError::InvalidWorkflowField { path, .. } = err {
            assert_eq!(
                path, "env.AA-BAD",
                "lexicographically first bad key must be reported first"
            );
        } else {
            panic!("expected InvalidWorkflowField");
        }
    }

    #[test]
    fn validate_env_map_carries_caller_supplied_prefix() {
        // The defaults call-site uses a non-serde prefix
        // (`.stepyard/defaults.yaml:env`) so the operator sees where
        // the bad value lives in the filesystem rather than at a YAML
        // serde path.
        let mut env = HashMap::new();
        env.insert("BAD KEY".into(), "x".into());

        let err = validate_env_map(".stepyard/defaults.yaml:env", &env).expect_err("space");
        if let EngineError::InvalidWorkflowField { path, .. } = err {
            assert_eq!(path, ".stepyard/defaults.yaml:env.BAD KEY");
        } else {
            panic!("expected InvalidWorkflowField");
        }
    }
}
