use std::time::Duration;
use tokio::time::sleep;

use crate::error::StepError;

/// Shared retry configuration for rate limit errors
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,  // 1s, 2s, 4s progression
            max_delay_ms: 8000,   // Cap at 8s
        }
    }
}

impl RetryConfig {
    /// Parse retry configuration from StepConfig
    pub fn from_config(config: &crate::config::StepConfig) -> Self {
        Self {
            max_retries: config.get_u64("max_retries").unwrap_or(3) as usize,
            base_delay_ms: config.get_u64("retry_base_delay_ms").unwrap_or(1000),
            max_delay_ms: config.get_u64("retry_max_delay_ms").unwrap_or(8000),
        }
    }
}

/// Check if the error text contains rate limit indicators
pub fn is_rate_limit_error_generic(error_text: &str) -> bool {
    let err_str = error_text.to_lowercase();
    err_str.contains("429") ||
    err_str.contains("rate limit") ||
    err_str.contains("too many requests") ||
    err_str.contains("quota exceeded")
}

/// Extract Retry-After header value from error text if present
pub fn extract_retry_after_generic(error_text: &str) -> Option<Duration> {
    let err_str = error_text.to_lowercase();

    // Look for patterns like "retry after 5 seconds" or "retry-after: 3"
    for pattern in &["retry-after:", "retry after", "wait"] {
        if let Some(start) = err_str.find(pattern) {
            let remainder = &err_str[start + pattern.len()..];
            // Extract first number after the pattern
            let mut num_str = String::new();
            for ch in remainder.chars() {
                if ch.is_ascii_digit() {
                    num_str.push(ch);
                } else if !num_str.is_empty() {
                    break;
                } else if ch.is_ascii_whitespace() || ch == ':' || ch == '=' {
                    continue;
                } else {
                    break;
                }
            }
            if let Ok(seconds) = num_str.parse::<u64>() {
                return Some(Duration::from_secs(seconds));
            }
        }
    }

    None
}

/// Calculate backoff delay for retry attempts
pub fn calculate_backoff_delay(
    attempt: usize,
    config: &RetryConfig,
    retry_after: Option<Duration>,
) -> Duration {
    // Respect Retry-After header if present
    if let Some(retry_after) = retry_after {
        return retry_after.min(Duration::from_millis(config.max_delay_ms));
    }

    // Exponential backoff: base_delay * 2^attempt
    let exponential_delay = config.base_delay_ms * (2_u64.pow(attempt as u32));
    let capped_delay = exponential_delay.min(config.max_delay_ms);
    Duration::from_millis(capped_delay)
}

/// Generic retry loop for operations that might hit rate limits
pub async fn retry_on_rate_limit<F, Fut, T>(
    operation: F,
    config: &RetryConfig,
    provider: &str,
    error_extractor: impl Fn(&StepError) -> Option<String>,
) -> Result<T, StepError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, StepError>>,
{
    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(err) => {
                // Check if this is a rate limit error
                let is_rate_limit = if let Some(error_text) = error_extractor(&err) {
                    is_rate_limit_error_generic(&error_text)
                } else {
                    false
                };

                if is_rate_limit && attempt < config.max_retries {
                    let retry_after = if let Some(error_text) = error_extractor(&err) {
                        extract_retry_after_generic(&error_text)
                    } else {
                        None
                    };

                    let delay = calculate_backoff_delay(attempt, config, retry_after);

                    tracing::warn!(
                        provider = provider,
                        attempt = attempt + 1,
                        delay_ms = delay.as_millis(),
                        "API rate limit hit, retrying after delay"
                    );

                    sleep(delay).await;
                    continue;
                }

                // Either not a rate limit error, or retries exhausted
                if is_rate_limit && attempt >= config.max_retries {
                    return Err(StepError::RateLimitExhausted {
                        provider: provider.to_string(),
                        attempts: config.max_retries + 1
                    });
                }

                return Err(err);
            }
        }
    }

    unreachable!("Loop should always return or continue")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 8000);
    }

    #[test]
    fn test_retry_config_from_config() {
        use std::collections::HashMap;
        use crate::config::StepConfig;

        let mut values = HashMap::new();
        values.insert("max_retries".to_string(), serde_json::Value::Number(5.into()));
        values.insert("retry_base_delay_ms".to_string(), serde_json::Value::Number(2000.into()));
        values.insert("retry_max_delay_ms".to_string(), serde_json::Value::Number(10000.into()));
        let step_config = StepConfig { values };

        let config = RetryConfig::from_config(&step_config);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.base_delay_ms, 2000);
        assert_eq!(config.max_delay_ms, 10000);
    }

    #[test]
    fn test_is_rate_limit_error_generic() {
        assert!(is_rate_limit_error_generic("Error 429: Too Many Requests"));
        assert!(is_rate_limit_error_generic("rate limit exceeded"));
        assert!(is_rate_limit_error_generic("Too many requests"));
        assert!(is_rate_limit_error_generic("Quota exceeded"));
        assert!(!is_rate_limit_error_generic("Internal server error"));
        assert!(!is_rate_limit_error_generic("Connection timeout"));
    }

    #[test]
    fn test_extract_retry_after_generic() {
        assert_eq!(
            extract_retry_after_generic("retry-after: 5"),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            extract_retry_after_generic("Please wait. retry after 10 seconds"),
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            extract_retry_after_generic("Rate limit exceeded. wait 30"),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            extract_retry_after_generic("No retry information"),
            None
        );
        assert_eq!(
            extract_retry_after_generic("retry-after: invalid"),
            None
        );
    }

    #[test]
    fn test_calculate_backoff_delay() {
        let config = RetryConfig::default();

        // Test exponential backoff without retry-after
        assert_eq!(
            calculate_backoff_delay(0, &config, None),
            Duration::from_millis(1000)
        );
        assert_eq!(
            calculate_backoff_delay(1, &config, None),
            Duration::from_millis(2000)
        );
        assert_eq!(
            calculate_backoff_delay(2, &config, None),
            Duration::from_millis(4000)
        );

        // Test retry-after override
        let retry_after = Some(Duration::from_secs(3));
        assert_eq!(
            calculate_backoff_delay(0, &config, retry_after),
            Duration::from_secs(3)
        );

        // Test max delay cap
        assert_eq!(
            calculate_backoff_delay(10, &config, None),
            Duration::from_millis(8000)  // Capped at max_delay_ms
        );
    }
}