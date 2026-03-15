//! Retry logic for API rate limit errors (429)
//! 
//! This module provides exponential backoff retry for transient API failures.

use std::time::Duration;

/// Configuration for retry behavior
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Base delay between retries (doubles each attempt)
    pub base_delay: Duration,
    /// Maximum delay cap
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

/// Calculate delay for a given retry attempt using exponential backoff
pub fn backoff_delay(attempt: u32, config: &RetryConfig) -> Duration {
    let delay = config.base_delay * 2u32.pow(attempt);
    std::cmp::min(delay, config.max_delay)
}

/// Check if an error is retryable (e.g., 429 rate limit)
pub fn is_retryable_error(error_msg: &str) -> bool {
    error_msg.contains("429")
        || error_msg.contains("rate_limit")
        || error_msg.contains("Too Many Requests")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_delay() {
        let config = RetryConfig::default();
        assert_eq!(backoff_delay(0, &config), Duration::from_secs(1));
        assert_eq!(backoff_delay(1, &config), Duration::from_secs(2));
        assert_eq!(backoff_delay(2, &config), Duration::from_secs(4));
    }

    #[test]
    fn test_backoff_max_cap() {
        let config = RetryConfig {
            max_retries: 10,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
        };
        assert_eq!(backoff_delay(5, &config), Duration::from_secs(10));
    }

    #[test]
    fn test_is_retryable() {
        assert!(is_retryable_error("HTTP 429 Too Many Requests"));
        assert!(is_retryable_error("rate_limit_error"));
        assert!(!is_retryable_error("HTTP 500 Internal Server Error"));
    }
}
