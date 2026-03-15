use std::fmt;
use std::time::{Duration, Instant};

use tracing::{error, info, warn};

/// Configuration for retry operations with exponential backoff
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (default: 3)
    pub max_retries: u32,
    /// Base delay for first retry (default: 1s)
    pub base_delay: Duration,
    /// Maximum delay between retries (default: 30s)
    pub max_delay: Duration,
    /// Multiplier for exponential backoff (default: 2.0)
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
        }
    }
}

/// Error returned when all retry attempts have been exhausted
#[derive(Debug)]
pub struct RetryError {
    /// Number of retry attempts made
    pub attempts: u32,
    /// The last error encountered
    pub last_error: Box<dyn std::error::Error + Send + Sync>,
    /// Total time spent retrying
    pub total_duration: Duration,
}

impl fmt::Display for RetryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Operation failed after {} attempts (duration: {:?}): {}",
            self.attempts, self.total_duration, self.last_error
        )
    }
}

impl std::error::Error for RetryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.last_error.as_ref())
    }
}

/// Extract Retry-After header value from an error if available
pub fn extract_retry_after<E: std::error::Error>(error: &E) -> Option<Duration> {
    // Try to extract from error message - this is a best-effort approach
    // since we don't have direct access to HTTP response headers from rig-core errors
    let error_str = error.to_string();

    // Look for common patterns like "retry after 30 seconds" or "Retry-After: 60"
    if let Some(captures) = regex::Regex::new(r"(?i)retry[- _]?after[:\s]+(\d+)")
        .ok()
        .and_then(|re| re.captures(&error_str))
    {
        if let Ok(seconds) = captures.get(1).unwrap().as_str().parse::<u64>() {
            return Some(Duration::from_secs(seconds));
        }
    }

    None
}

/// Calculate the next delay using exponential backoff
fn calculate_delay(attempt: u32, config: &RetryConfig, retry_after: Option<Duration>) -> Duration {
    // If Retry-After header is present, respect it
    if let Some(retry_after_duration) = retry_after {
        return retry_after_duration.min(config.max_delay);
    }

    // Calculate exponential backoff delay
    let exponential_delay = Duration::from_millis(
        (config.base_delay.as_millis() as f64
            * config.backoff_multiplier.powi(attempt as i32)) as u64
    );

    // Apply maximum delay cap
    exponential_delay.min(config.max_delay)
}

/// Retry an async operation with exponential backoff
///
/// # Arguments
/// * `operation` - A function that returns a Future representing the operation to retry
/// * `config` - Configuration for retry behavior
/// * `should_retry` - A function that determines if an error should trigger a retry
///
/// # Returns
/// * `Ok(T)` - The successful result of the operation
/// * `Err(RetryError)` - Aggregated error information after all retries are exhausted
pub async fn retry_with_backoff<T, E, Fut, F>(
    operation: F,
    config: RetryConfig,
    should_retry: impl Fn(&E) -> bool,
) -> Result<T, RetryError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let start_time = Instant::now();
    let mut last_error: Option<E> = None;

    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => {
                if attempt > 0 {
                    info!(
                        attempt = attempt,
                        duration = ?start_time.elapsed(),
                        "Operation succeeded after retry"
                    );
                }
                return Ok(result);
            }
            Err(error) => {
                let is_retryable = should_retry(&error);
                let is_last_attempt = attempt == config.max_retries;

                if !is_retryable || is_last_attempt {
                    error!(
                        attempt = attempt + 1,
                        retryable = is_retryable,
                        duration = ?start_time.elapsed(),
                        error = %error,
                        "Operation failed and will not be retried"
                    );

                    return Err(RetryError {
                        attempts: attempt + 1,
                        last_error: Box::new(error),
                        total_duration: start_time.elapsed(),
                    });
                }

                // Extract retry-after information
                let retry_after = extract_retry_after(&error);
                let delay = calculate_delay(attempt, &config, retry_after);

                warn!(
                    attempt = attempt + 1,
                    max_attempts = config.max_retries + 1,
                    delay_ms = delay.as_millis(),
                    retry_after_ms = retry_after.map(|d| d.as_millis()),
                    error = %error,
                    "Operation failed, retrying after delay"
                );

                last_error = Some(error);

                // Wait before retrying
                tokio::time::sleep(delay).await;
            }
        }
    }

    // This should never be reached due to the loop logic above,
    // but we include it for completeness
    let final_error = last_error.unwrap_or_else(|| {
        // Create a generic error if we somehow get here without one
        E::try_from("Unexpected retry loop completion").unwrap_or_else(|_| {
            panic!("Failed to create error type - this should never happen")
        })
    });

    Err(RetryError {
        attempts: config.max_retries + 1,
        last_error: Box::new(final_error),
        total_duration: start_time.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[derive(Debug)]
    struct TestError {
        message: String,
        retryable: bool,
    }

    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl std::error::Error for TestError {}

    #[tokio::test]
    async fn test_successful_operation_first_try() {
        let config = RetryConfig::default();
        let result = retry_with_backoff(
            || async { Ok::<i32, TestError>(42) },
            config,
            |_| true,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_successful_operation_after_retry() {
        let config = RetryConfig::default();
        let attempt_count = Arc::new(AtomicU32::new(0));
        let attempt_count_clone = attempt_count.clone();

        let result = retry_with_backoff(
            move || {
                let count = attempt_count_clone.fetch_add(1, Ordering::SeqCst);
                async move {
                    if count < 2 {
                        Err(TestError {
                            message: "Temporary failure".to_string(),
                            retryable: true,
                        })
                    } else {
                        Ok(42)
                    }
                }
            },
            config,
            |e| e.retryable,
        ).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhaustion() {
        let config = RetryConfig {
            max_retries: 2,
            base_delay: Duration::from_millis(10),
            ..Default::default()
        };

        let result = retry_with_backoff(
            || async {
                Err::<i32, TestError>(TestError {
                    message: "Always fails".to_string(),
                    retryable: true,
                })
            },
            config,
            |e| e.retryable,
        ).await;

        assert!(result.is_err());
        let retry_error = result.unwrap_err();
        assert_eq!(retry_error.attempts, 3); // max_retries + 1
        assert!(retry_error.last_error.to_string().contains("Always fails"));
    }

    #[tokio::test]
    async fn test_non_retryable_error() {
        let config = RetryConfig::default();

        let result = retry_with_backoff(
            || async {
                Err::<i32, TestError>(TestError {
                    message: "Non-retryable error".to_string(),
                    retryable: false,
                })
            },
            config,
            |e| e.retryable,
        ).await;

        assert!(result.is_err());
        let retry_error = result.unwrap_err();
        assert_eq!(retry_error.attempts, 1); // No retries for non-retryable error
    }

    #[tokio::test]
    async fn test_exponential_backoff_timing() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            backoff_multiplier: 2.0,
            max_delay: Duration::from_secs(1),
        };

        let start = Instant::now();
        let result = retry_with_backoff(
            || async {
                Err::<i32, TestError>(TestError {
                    message: "Always fails".to_string(),
                    retryable: true,
                })
            },
            config,
            |e| e.retryable,
        ).await;

        let elapsed = start.elapsed();
        assert!(result.is_err());

        // Should have delays of approximately: 100ms, 200ms, 400ms
        // Total should be at least 700ms but we allow some variance
        assert!(elapsed >= Duration::from_millis(650));
        assert!(elapsed <= Duration::from_millis(1000));
    }

    #[tokio::test]
    async fn test_extract_retry_after_from_error_message() {
        let error = TestError {
            message: "Rate limited. Retry after 30 seconds".to_string(),
            retryable: true,
        };

        let retry_after = extract_retry_after(&error);
        assert_eq!(retry_after, Some(Duration::from_secs(30)));
    }

    #[tokio::test]
    async fn test_max_delay_cap() {
        let config = RetryConfig {
            max_retries: 1,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_millis(500), // Cap at 500ms
            backoff_multiplier: 10.0, // Large multiplier
        };

        let delay = calculate_delay(1, &config, None);
        assert_eq!(delay, Duration::from_millis(500)); // Should be capped
    }

    #[tokio::test]
    async fn test_retry_after_header_priority() {
        let config = RetryConfig {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            ..Default::default()
        };

        let retry_after = Some(Duration::from_secs(5));
        let delay = calculate_delay(0, &config, retry_after);

        // Should use Retry-After value instead of exponential backoff
        assert_eq!(delay, Duration::from_secs(5));
    }
}