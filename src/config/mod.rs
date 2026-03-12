pub mod manager;
pub mod merge;

pub use manager::ConfigManager;

use std::collections::HashMap;
use std::time::Duration;

/// Resolved configuration for a specific step (after 4-layer merge)
#[derive(Debug, Clone, Default)]
pub struct StepConfig {
    pub values: HashMap<String, serde_json::Value>,
}

#[allow(dead_code)]
impl StepConfig {
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.values.get(key).and_then(|v| v.as_str())
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.values
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    pub fn get_duration(&self, key: &str) -> Option<Duration> {
        let s = self.get_str(key)?;
        parse_duration(s)
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.values.get(key).and_then(|v| v.as_u64())
    }
}

pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    // Check "ms" before "s" to avoid "100ms" matching as seconds
    if let Some(ms) = s.strip_suffix("ms") {
        ms.trim().parse::<u64>().ok().map(Duration::from_millis)
    } else if let Some(secs) = s.strip_suffix('s') {
        secs.trim().parse::<u64>().ok().map(Duration::from_secs)
    } else if let Some(mins) = s.strip_suffix('m') {
        mins.trim()
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60))
    } else {
        s.parse::<u64>().ok().map(Duration::from_secs)
    }
}
