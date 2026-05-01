//! YAML-safe `{{KEY}}` substitution helpers.
//!
//! This module is deliberately pure: no IO, no globals, no async runtime.
//! Values are serialized as YAML scalars before splicing, so placeholder
//! substitution cannot inject sibling YAML fields through raw string content.

use std::collections::HashMap;

use thiserror::Error;

const DEFAULT_FOUND_AT: &str = "<workflow-yaml>";

#[non_exhaustive]
#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("placeholder `{key}` unresolved at {found_at}")]
    Unresolved { key: String, found_at: String },

    #[error("invalid placeholder `{token}` at byte {position}")]
    InvalidPlaceholder { token: String, position: usize },

    #[error("yaml encoding failed: {source}")]
    YamlEncoding { source: serde_yaml::Error },
}

/// Substitute every `{{KEY}}` token in `text` with the YAML-encoded scalar
/// value from `vars`.
///
/// `KEY` must match `[A-Z_][A-Z0-9_]*`. Missing keys return
/// [`TemplateError::Unresolved`]. Invalid or unterminated placeholders return
/// [`TemplateError::InvalidPlaceholder`]. The replacement strips the trailing
/// document newline that `serde_yaml::to_string` appends so the encoded scalar
/// remains inline at the call site.
pub fn substitute(
    text: &str,
    vars: &HashMap<String, String>,
) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;

    while let Some(open_rel) = text[cursor..].find("{{") {
        let open = cursor + open_rel;
        out.push_str(&text[cursor..open]);
        let key_start = open + 2;
        let Some(close_rel) = text[key_start..].find("}}") else {
            return Err(TemplateError::InvalidPlaceholder {
                token: text[open..].to_string(),
                position: open,
            });
        };
        let close = key_start + close_rel;
        let key = &text[key_start..close];
        if !is_valid_key(key) {
            return Err(TemplateError::InvalidPlaceholder {
                token: text[open..close + 2].to_string(),
                position: open,
            });
        }

        let value = vars.get(key).ok_or_else(|| TemplateError::Unresolved {
            key: key.to_string(),
            found_at: DEFAULT_FOUND_AT.to_string(),
        })?;
        out.push_str(&yaml_scalar(value)?);
        cursor = close + 2;
    }

    out.push_str(&text[cursor..]);
    Ok(out)
}

fn yaml_scalar(value: &str) -> Result<String, TemplateError> {
    let encoded =
        serde_yaml::to_string(value).map_err(|source| TemplateError::YamlEncoding { source })?;
    Ok(encoded.trim_end_matches('\n').to_string())
}

fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some('A'..='Z') | Some('_') => {}
        _ => return false,
    }
    chars.all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn substitutes_simple_placeholder() {
        let got = substitute("name: {{PROJECT}}", &vars(&[("PROJECT", "demo")])).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&got).unwrap();
        assert_eq!(value["name"], "demo");
    }

    #[test]
    fn missing_key_returns_unresolved() {
        let err = substitute("name: {{MISSING}}", &HashMap::new()).unwrap_err();
        match err {
            TemplateError::Unresolved { key, found_at } => {
                assert_eq!(key, "MISSING");
                assert_eq!(found_at, DEFAULT_FOUND_AT);
            }
            other => panic!("expected Unresolved, got {other:?}"),
        }
    }

    #[test]
    fn invalid_placeholder_key_is_rejected() {
        let err = substitute("name: {{bad-key}}", &HashMap::new()).unwrap_err();
        match err {
            TemplateError::InvalidPlaceholder { token, position } => {
                assert_eq!(token, "{{bad-key}}");
                assert_eq!(position, 6);
            }
            other => panic!("expected InvalidPlaceholder, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_placeholder_is_rejected() {
        let err = substitute("name: {{PROJECT", &HashMap::new()).unwrap_err();
        match err {
            TemplateError::InvalidPlaceholder { token, position } => {
                assert_eq!(token, "{{PROJECT");
                assert_eq!(position, 6);
            }
            other => panic!("expected InvalidPlaceholder, got {other:?}"),
        }
    }

    #[test]
    fn yaml_escape_prevents_mapping_injection() {
        let got = substitute(
            "name: {{ATTACK}}",
            &vars(&[("ATTACK", "x\nfoo: y")]),
        )
        .unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&got).unwrap();
        assert_eq!(value["name"], "x\nfoo: y");
        assert!(value.get("foo").is_none(), "substitution injected: {got}");
    }

    #[test]
    fn yaml_escape_prevents_alias_injection() {
        let got = substitute("name: {{ATTACK}}", &vars(&[("ATTACK", "*alias_ref")])).unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&got).unwrap();
        assert_eq!(value["name"], "*alias_ref");
    }

    #[test]
    fn strips_serializer_trailing_newline() {
        let got = substitute("value={{VALUE}}.", &vars(&[("VALUE", "abc")])).unwrap();
        assert!(!got.contains('\n'), "unexpected newline in {got:?}");
        assert!(got.ends_with('.'));
    }

    #[test]
    fn multiple_placeholders_are_replaced_left_to_right() {
        let got = substitute(
            "a: {{A}}\nb: {{B}}",
            &vars(&[("A", "one"), ("B", "two")]),
        )
        .unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&got).unwrap();
        assert_eq!(value["a"], "one");
        assert_eq!(value["b"], "two");
    }

    #[test]
    fn key_grammar_allows_uppercase_digits_after_first_and_underscore() {
        let got = substitute(
            "name: {{_A1}}",
            &vars(&[("_A1", "ok")]),
        )
        .unwrap();
        let value: serde_yaml::Value = serde_yaml::from_str(&got).unwrap();
        assert_eq!(value["name"], "ok");
    }
}

#[cfg(test)]
mod proptest_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn substitute_never_panics(text in ".*", value in ".*") {
            let mut vars = HashMap::new();
            vars.insert("KEY".to_string(), value);
            let _ = substitute(&text, &vars);
        }

        #[test]
        fn scalar_values_cannot_break_out_of_yaml_field(value in ".*") {
            let mut vars = HashMap::new();
            vars.insert("KEY".to_string(), value.clone());
            let substituted = substitute("name: {{KEY}}", &vars).unwrap();
            let parsed: serde_yaml::Value = serde_yaml::from_str(&substituted).unwrap();
            prop_assert_eq!(parsed["name"].as_str(), Some(value.as_str()));
        }
    }
}
