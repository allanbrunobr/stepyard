use serde_json::Value as JsonValue;

/// Convert serde_yaml::Value → serde_json::Value
pub fn yaml_to_json(v: &serde_yaml::Value) -> JsonValue {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}
