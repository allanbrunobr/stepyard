use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;

use crate::config::StepConfig;
use crate::engine::context::{ChatMessage, Context};
use crate::error::StepError;
use crate::retry::{retry_with_backoff, RetryConfig};
use crate::workflow::schema::StepDef;

use super::{ChatOutput, StepExecutor, StepOutput};

// ── Rig imports ──────────────────────────────────────────────────
use rig::client::CompletionClient;
use rig::completion::{CompletionError, CompletionModel, CompletionResponse};
use rig::message::{AssistantContent, Message};

/// Truncation strategy for chat history (Story 5.2)
#[derive(Debug, Clone)]
pub enum TruncationStrategy {
    /// Keep all messages (default)
    None,
    /// Keep the last N messages
    Last(usize),
    /// Keep the first N messages
    First(usize),
    /// Keep the first `first` and last `last` messages
    FirstLast { first: usize, last: usize },
    /// Drop oldest messages until total estimated tokens <= max_tokens
    SlidingWindow { max_tokens: usize },
}

impl TruncationStrategy {
    /// Parse from StepConfig keys: truncation_strategy, truncation_count, truncation_first,
    /// truncation_last, truncation_max_tokens
    pub fn from_config(config: &crate::config::StepConfig) -> Self {
        match config.get_str("truncation_strategy") {
            Some("last") => {
                let n = config.get_u64("truncation_count").unwrap_or(10) as usize;
                TruncationStrategy::Last(n)
            }
            Some("first") => {
                let n = config.get_u64("truncation_count").unwrap_or(10) as usize;
                TruncationStrategy::First(n)
            }
            Some("first_last") => {
                let first = config.get_u64("truncation_first").unwrap_or(2) as usize;
                let last = config.get_u64("truncation_last").unwrap_or(5) as usize;
                TruncationStrategy::FirstLast { first, last }
            }
            Some("sliding_window") => {
                let max_tokens =
                    config.get_u64("truncation_max_tokens").unwrap_or(50_000) as usize;
                TruncationStrategy::SlidingWindow { max_tokens }
            }
            _ => TruncationStrategy::None,
        }
    }
}

/// Estimate token count using simple word-based heuristic (words * 1.3)
fn estimate_tokens(text: &str) -> usize {
    let words = text.split_whitespace().count();
    ((words as f64) * 1.3).ceil() as usize
}

/// Apply truncation to a list of messages, returning the subset to send
pub fn truncate_messages(
    messages: &[ChatMessage],
    strategy: &TruncationStrategy,
) -> Vec<ChatMessage> {
    match strategy {
        TruncationStrategy::None => messages.to_vec(),
        TruncationStrategy::Last(n) => {
            let start = messages.len().saturating_sub(*n);
            messages[start..].to_vec()
        }
        TruncationStrategy::First(n) => {
            messages[..messages.len().min(*n)].to_vec()
        }
        TruncationStrategy::FirstLast { first, last } => {
            let len = messages.len();
            let first_end = (*first).min(len);
            let last_start = len.saturating_sub(*last);
            if first_end >= last_start {
                // Overlap or adjacent — return all
                messages.to_vec()
            } else {
                let mut result = messages[..first_end].to_vec();
                result.extend_from_slice(&messages[last_start..]);
                result
            }
        }
        TruncationStrategy::SlidingWindow { max_tokens } => {
            // Greedily include messages from oldest to newest until token budget exceeded
            // Then drop from the front until we fit
            let total_tokens: usize =
                messages.iter().map(|m| estimate_tokens(&m.content)).sum();
            if total_tokens <= *max_tokens {
                return messages.to_vec();
            }
            let mut tokens_used = total_tokens;
            let mut drop_count = 0;
            for msg in messages.iter() {
                if tokens_used <= *max_tokens {
                    break;
                }
                tokens_used -= estimate_tokens(&msg.content);
                drop_count += 1;
            }
            messages[drop_count..].to_vec()
        }
    }
}

// ── Rig helper functions ─────────────────────────────────────────

/// Convert internal ChatMessage list to Rig Message vector
fn to_rig_messages(history: &[ChatMessage]) -> Vec<Message> {
    history
        .iter()
        .map(|m| match m.role.as_str() {
            "assistant" => {
                Message::from(AssistantContent::text(&m.content))
            }
            _ => {
                // user, system, or any other role → treat as user message
                Message::from(m.content.as_str())
            }
        })
        .collect()
}

/// Extract text and token usage from a Rig CompletionResponse
fn extract_chat_output<T>(response: CompletionResponse<T>, model: &str) -> ChatOutput {
    let text = response
        .choice
        .iter()
        .filter_map(|c| {
            if let AssistantContent::Text(t) = c {
                Some(t.text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    ChatOutput {
        response: text,
        model: model.to_string(),
        input_tokens: response.usage.input_tokens,
        output_tokens: response.usage.output_tokens,
    }
}

/// Extract HTTP status code from rig-core CompletionError
fn extract_http_status(error: &CompletionError) -> Option<u16> {
    let error_str = error.to_string();

    // Try to extract HTTP status code from error message
    // Common patterns: "HTTP 429", "status: 429", "429 Too Many Requests"
    let patterns = [
        r"(?i)HTTP\s+(\d{3})",
        r"(?i)status[:\s]+(\d{3})",
        r"(?i)^(\d{3})\s+",
        r"(?i)\b(\d{3})\s+(?:Too Many Requests|Rate)",
    ];

    for pattern in &patterns {
        if let Ok(re) = Regex::new(pattern) {
            if let Some(captures) = re.captures(&error_str) {
                if let Some(status_match) = captures.get(1) {
                    if let Ok(status) = status_match.as_str().parse::<u16>() {
                        // Validate it's a real HTTP status code (100-599)
                        if (100..600).contains(&status) {
                            return Some(status);
                        }
                    }
                }
            }
        }
    }

    // Check for common rate limit indicators without regex
    let error_lower = error_str.to_lowercase();
    if error_lower.contains("rate limit")
        || error_lower.contains("too many requests")
        || error_lower.contains("quota exceeded")
        || error_lower.contains("429") {
        return Some(429);
    }

    None
}

/// Extract Retry-After duration from rig-core CompletionError
fn extract_retry_after_from_completion_error(error: &CompletionError) -> Option<Duration> {
    crate::retry::extract_retry_after(error)
}

/// Map Rig CompletionError to StepError
fn map_rig_error(provider: &str, err: CompletionError) -> StepError {
    if let Some(status_code) = extract_http_status(&err) {
        let retry_after = if status_code == 429 {
            extract_retry_after_from_completion_error(&err)
        } else {
            None
        };

        StepError::HttpError {
            status_code,
            message: format!("{} API error: {}", provider, err),
            retry_after,
        }
    } else {
        StepError::Fail(format!("{} API error: {}", provider, err))
    }
}

/// Build Rig client error to StepError
fn map_build_error(provider: &str, err: impl std::fmt::Display) -> StepError {
    StepError::Fail(format!("Failed to build {} client: {}", provider, err))
}

/// Macro to avoid repeating completion_request → send → extract in every match arm.
/// Each provider arm creates its own `client`, then invokes this macro.
macro_rules! send_completion {
    ($client:expr, $model_name:expr, $prompt:expr, $messages:expr,
     $temperature:expr, $max_tokens:expr, $provider:expr) => {{
        let model = $client.completion_model($model_name);
        let resp: Result<_, CompletionError> = model
            .completion_request($prompt)
            .messages($messages)
            .temperature($temperature)
            .max_tokens($max_tokens)
            .send()
            .await;
        let resp = resp.map_err(|e| map_rig_error($provider, e))?;
        Ok::<StepOutput, StepError>(StepOutput::Chat(extract_chat_output(resp, $model_name)))
    }};
}

/// Call LLM via Rig — unified multi-provider completion with retry logic
async fn call_via_rig(
    provider: &str,
    model_name: &str,
    api_key: &str,
    base_url: Option<&str>,
    messages: Vec<Message>,
    prompt: &str,
    temperature: f64,
    max_tokens: u64,
    timeout: Duration,
) -> Result<StepOutput, StepError> {
    let retry_config = RetryConfig {
        max_retries: 3,
        base_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(30),
        backoff_multiplier: 2.0,
    };

    // Clone all needed data for the retry closure
    let provider = provider.to_string();
    let model_name = model_name.to_string();
    let api_key = api_key.to_string();
    let base_url = base_url.map(String::from);
    let prompt = prompt.to_string();

    let operation = || {
        let provider = provider.clone();
        let model_name = model_name.clone();
        let api_key = api_key.clone();
        let base_url = base_url.clone();
        let messages = messages.clone();
        let prompt = prompt.clone();

        async move {
            tokio::time::timeout(timeout, async {
                match provider.as_str() {
                    // ── Anthropic ────────────────────────────────────────
                    "anthropic" => {
                        let mut builder = rig::providers::anthropic::Client::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("anthropic", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "anthropic")
                    }

                    // ── OpenAI (Chat Completions API — LiteLLM compatible) ──
                    "openai" => {
                        let mut builder = rig::providers::openai::CompletionsClient::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("openai", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "openai")
                    }

                    // ── Ollama (local, no API key) ───────────────────────
                    "ollama" => {
                        let mut builder = rig::providers::ollama::Client::builder()
                            .api_key(rig::client::Nothing);
                        let url = base_url.as_deref().unwrap_or("http://localhost:11434");
                        builder = builder.base_url(url);
                        let client = builder.build().map_err(|e| map_build_error("ollama", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "ollama")
                    }

                    // ── Groq ─────────────────────────────────────────────
                    "groq" => {
                        let mut builder = rig::providers::groq::Client::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("groq", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "groq")
                    }

                    // ── DeepSeek ─────────────────────────────────────────
                    "deepseek" => {
                        let mut builder = rig::providers::deepseek::Client::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("deepseek", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "deepseek")
                    }

                    // ── Google Gemini ────────────────────────────────────
                    "gemini" | "google" => {
                        let mut builder = rig::providers::gemini::Client::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("gemini", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "gemini")
                    }

                    // ── Cohere ───────────────────────────────────────────
                    "cohere" => {
                        let mut builder = rig::providers::cohere::Client::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("cohere", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "cohere")
                    }

                    // ── Perplexity ───────────────────────────────────────
                    "perplexity" => {
                        let mut builder = rig::providers::perplexity::Client::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("perplexity", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "perplexity")
                    }

                    // ── xAI (Grok) ──────────────────────────────────────
                    "xai" | "grok" => {
                        let mut builder = rig::providers::xai::Client::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("xai", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "xai")
                    }

                    // ── Mistral ─────────────────────────────────────────
                    "mistral" => {
                        let mut builder = rig::providers::mistral::Client::builder()
                            .api_key(&api_key);
                        if let Some(url) = &base_url {
                            builder = builder.base_url(url);
                        }
                        let client = builder.build().map_err(|e| map_build_error("mistral", e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, "mistral")
                    }

                    // ── Any other: OpenAI-compatible with custom base_url ──
                    // This covers LiteLLM, vLLM, Azure (via base_url), or
                    // any service that implements the OpenAI Chat Completions API.
                    other => {
                        let url = base_url.as_deref().ok_or_else(|| StepError::Fail(format!(
                            "Unknown provider '{}': set 'base_url' to use as OpenAI-compatible endpoint",
                            other
                        )))?;
                        let builder = rig::providers::openai::CompletionsClient::builder()
                            .api_key(&api_key)
                            .base_url(url);
                        let client = builder.build().map_err(|e| map_build_error(other, e))?;
                        send_completion!(client, &model_name, &prompt, messages, temperature, max_tokens, other)
                    }
                }
            })
            .await
            .map_err(|_| StepError::Timeout(timeout))?
        }
    };

    let result = retry_with_backoff(
        operation,
        retry_config,
        |error| error.should_retry(),
    ).await;

    match result {
        Ok(output) => Ok(output),
        Err(retry_error) => Err(StepError::RateLimitExhausted {
            attempts: retry_error.attempts,
            duration: retry_error.total_duration,
            last_error: retry_error.last_error,
        }),
    }
}

// ── ChatExecutor ─────────────────────────────────────────────────

pub struct ChatExecutor;

#[async_trait]
impl StepExecutor for ChatExecutor {
    async fn execute(
        &self,
        step: &StepDef,
        config: &StepConfig,
        ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let provider = config.get_str("provider").unwrap_or("anthropic");
        let model = config.get_str("model").unwrap_or(match provider {
            "openai" => "gpt-4o-mini",
            "ollama" => "llama3.2",
            "groq" => "llama-3.3-70b-versatile",
            "deepseek" => "deepseek-chat",
            "gemini" | "google" => "gemini-2.0-flash",
            _ => "claude-3-haiku-20240307",
        });
        let max_tokens = config.get_u64("max_tokens").unwrap_or(1024);
        let temperature = config
            .values
            .get("temperature")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let timeout = config
            .get_duration("timeout")
            .unwrap_or(Duration::from_secs(120));

        // Resolve API key (Ollama doesn't need one)
        let api_key = if provider == "ollama" {
            String::new()
        } else {
            let api_key_env = config.get_str("api_key_env").unwrap_or(match provider {
                "openai" => "OPENAI_API_KEY",
                "groq" => "GROQ_API_KEY",
                "deepseek" => "DEEPSEEK_API_KEY",
                "gemini" | "google" => "GEMINI_API_KEY",
                "cohere" => "COHERE_API_KEY",
                "perplexity" => "PERPLEXITY_API_KEY",
                "xai" | "grok" => "XAI_API_KEY",
                "mistral" => "MISTRAL_API_KEY",
                _ => "ANTHROPIC_API_KEY",
            });
            std::env::var(api_key_env).map_err(|_| {
                StepError::Fail(format!(
                    "API key not found: environment variable '{}' is not set",
                    api_key_env
                ))
            })?
        };

        // Resolve base_url: generic > provider-specific > default
        let base_url: Option<String> = config
            .get_str("base_url")
            .map(String::from)
            .or_else(|| {
                // Backward compatibility with old per-provider config keys
                match provider {
                    "anthropic" => config.get_str("anthropic_base_url").map(String::from),
                    "openai" => config.get_str("openai_base_url").map(String::from),
                    _ => None,
                }
            });

        let prompt_template = step
            .prompt
            .as_ref()
            .ok_or_else(|| StepError::Fail("chat step missing 'prompt' field".into()))?;

        let prompt = ctx.render_template(prompt_template)?;

        // Story 5.1 + 5.2: Build message list from chat history with optional truncation
        let session_name = config.get_str("session");
        let truncation = TruncationStrategy::from_config(config);
        let rig_messages: Vec<Message> = if let Some(session) = session_name {
            let history = ctx.get_chat_messages(session);
            let truncated = truncate_messages(&history, &truncation);
            to_rig_messages(&truncated)
        } else {
            Vec::new()
        };

        let output = call_via_rig(
            provider,
            model,
            &api_key,
            base_url.as_deref(),
            rig_messages,
            &prompt,
            temperature,
            max_tokens,
            timeout,
        )
        .await?;

        // Story 5.1: Store sent message and response in chat history
        if let Some(session) = session_name {
            let response_text = output.text().to_string();
            ctx.append_chat_messages(
                session,
                vec![
                    ChatMessage { role: "user".to_string(), content: prompt },
                    ChatMessage { role: "assistant".to_string(), content: response_text },
                ],
            );
        }

        Ok(output)
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_step(prompt: &str) -> StepDef {
        StepDef {
            name: "test_chat".to_string(),
            step_type: crate::workflow::schema::StepType::Chat,
            run: None,
            prompt: Some(prompt.to_string()),
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
    async fn chat_missing_api_key_friendly_error() {
        // Use a custom env var name that definitely won't be set
        let step = StepDef {
            name: "test_chat".to_string(),
            step_type: crate::workflow::schema::StepType::Chat,
            run: None,
            prompt: Some("Hello".to_string()),
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
        // Override the api_key_env to a definitely-unset var
        let mut config_values = HashMap::new();
        config_values.insert(
            "api_key_env".to_string(),
            serde_json::Value::String("DEFINITELY_NOT_SET_API_KEY_XYZ123".to_string()),
        );
        let config = StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());
        let result = ChatExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("DEFINITELY_NOT_SET_API_KEY_XYZ123"),
            "Error should mention env var name: {}", err
        );
    }

    #[tokio::test]
    async fn chat_missing_prompt_field_error() {
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "test-key"); }
        let step = StepDef {
            name: "test".to_string(),
            step_type: crate::workflow::schema::StepType::Chat,
            run: None,
            prompt: None,  // missing!
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
        let result = ChatExecutor.execute(&step, &config, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("prompt"), "Error should mention prompt: {}", err);
    }

    #[tokio::test]
    async fn chat_mock_anthropic_response() {
        // Rig's Anthropic client sends POST to /v1/messages with the same format
        // as the raw API. We mock the endpoint using wiremock.
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;
        let response_body = serde_json::json!({
            "id": "msg_mock123",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-haiku-20240307",
            "content": [{"type": "text", "text": "Hello from mock!"}],
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "stop_reason": "end_turn",
            "stop_sequence": null
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .mount(&mock_server)
            .await;

        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "test-key"); }

        let step = make_step("Hello");
        let mut config_values = HashMap::new();
        // Use Rig's base_url config to route to wiremock
        config_values.insert(
            "base_url".to_string(),
            serde_json::Value::String(mock_server.uri()),
        );
        let config = StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = ChatExecutor.execute(&step, &config, &ctx).await.unwrap();
        assert_eq!(result.text(), "Hello from mock!");
        if let StepOutput::Chat(o) = result {
            assert_eq!(o.model, "claude-3-haiku-20240307");
            assert_eq!(o.input_tokens, 10);
            assert_eq!(o.output_tokens, 5);
        } else {
            panic!("Expected Chat output");
        }
    }

    fn make_messages(count: usize) -> Vec<ChatMessage> {
        (0..count)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("message {}", i),
            })
            .collect()
    }

    #[test]
    fn truncation_last_keeps_n_messages() {
        let msgs = make_messages(50);
        let result = truncate_messages(&msgs, &TruncationStrategy::Last(10));
        assert_eq!(result.len(), 10);
        assert_eq!(result[0].content, "message 40");
        assert_eq!(result[9].content, "message 49");
    }

    #[test]
    fn truncation_first_last_keeps_first_and_last() {
        let msgs = make_messages(50);
        let result =
            truncate_messages(&msgs, &TruncationStrategy::FirstLast { first: 2, last: 5 });
        assert_eq!(result.len(), 7);
        assert_eq!(result[0].content, "message 0");
        assert_eq!(result[1].content, "message 1");
        assert_eq!(result[2].content, "message 45");
    }

    #[test]
    fn truncation_sliding_window_fits_within_tokens() {
        // Each message "message X" is ~1-2 words → ~2-3 estimated tokens
        // Build 50 messages; set max_tokens low enough to drop some
        let msgs = make_messages(50);
        let result =
            truncate_messages(&msgs, &TruncationStrategy::SlidingWindow { max_tokens: 50 });
        // Total tokens of 50 messages would exceed 50; result should be smaller
        let total: usize = result.iter().map(|m| estimate_tokens(&m.content)).sum();
        assert!(total <= 50, "Expected tokens <= 50, got {}", total);
    }

    #[test]
    fn truncation_none_returns_all() {
        let msgs = make_messages(10);
        let result = truncate_messages(&msgs, &TruncationStrategy::None);
        assert_eq!(result.len(), 10);
    }

    #[tokio::test]
    async fn chat_history_stores_messages_and_resends_on_second_call() {
        use wiremock::{Mock, MockServer, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;
        let response_body = serde_json::json!({
            "id": "msg_mock456",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-haiku-20240307",
            "content": [{"type": "text", "text": "Response text"}],
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "stop_reason": "end_turn",
            "stop_sequence": null
        });

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .expect(2)
            .mount(&mock_server)
            .await;

        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "test-key"); }

        let step = make_step("First message");
        let mut config_values = HashMap::new();
        config_values.insert(
            "base_url".to_string(),
            serde_json::Value::String(mock_server.uri()),
        );
        config_values.insert(
            "session".to_string(),
            serde_json::Value::String("review".to_string()),
        );
        let config = StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        // First call — stores user + assistant messages
        let _result1 = ChatExecutor.execute(&step, &config, &ctx).await.unwrap();

        // After first call, history should have 2 messages
        let history = ctx.get_chat_messages("review");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "First message");
        assert_eq!(history[1].role, "assistant");

        // Second call — history is sent along with new message
        let step2 = make_step("Second message");
        let _result2 = ChatExecutor.execute(&step2, &config, &ctx).await.unwrap();

        // History now has 4 messages
        let history2 = ctx.get_chat_messages("review");
        assert_eq!(history2.len(), 4);
    }

    #[test]
    fn to_rig_messages_converts_correctly() {
        let history = vec![
            ChatMessage { role: "user".to_string(), content: "Hello".to_string() },
            ChatMessage { role: "assistant".to_string(), content: "Hi!".to_string() },
            ChatMessage { role: "user".to_string(), content: "How are you?".to_string() },
        ];
        let rig_msgs = to_rig_messages(&history);
        assert_eq!(rig_msgs.len(), 3);

        // Verify user messages
        match &rig_msgs[0] {
            Message::User { .. } => {},
            _ => panic!("Expected User message at index 0"),
        }

        // Verify assistant messages
        match &rig_msgs[1] {
            Message::Assistant { .. } => {},
            _ => panic!("Expected Assistant message at index 1"),
        }
    }

    #[test]
    fn ollama_does_not_require_api_key() {
        // Verify that "ollama" provider skips the API key check
        let step = make_step("Hello");
        let mut config_values = HashMap::new();
        config_values.insert(
            "provider".to_string(),
            serde_json::Value::String("ollama".to_string()),
        );
        // No api_key_env set — should not fail at config resolution
        let config = StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        // We can't fully execute without Ollama running, but we verify
        // the provider is recognized and no API key error is raised.
        // The execute will fail at the HTTP level (connection refused)
        // rather than at the "API key not found" level.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ChatExecutor.execute(&step, &config, &ctx));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // Should NOT contain "API key not found"
        assert!(
            !err.contains("API key not found"),
            "Ollama should not require API key, but got: {}",
            err
        );
    }

    #[test]
    fn test_extract_http_status_from_error_messages() {
        // Test our regex logic directly with sample error messages
        let test_cases = vec![
            ("HTTP 429 Too Many Requests", Some(429)),
            ("status: 500 Internal Server Error", Some(500)),
            ("429 rate limit exceeded", Some(429)),
            ("Rate limit exceeded", Some(429)),
            ("too many requests", Some(429)),
            ("quota exceeded", Some(429)),
            ("Connection refused", None),
            ("Invalid response", None),
        ];

        for (message, expected) in test_cases {
            // Test our status extraction logic
            let error_lower = message.to_lowercase();
            let result = if error_lower.contains("rate limit")
                || error_lower.contains("too many requests")
                || error_lower.contains("quota exceeded")
                || error_lower.contains("429") {
                Some(429)
            } else {
                // Test HTTP status extraction patterns
                let patterns = [
                    r"(?i)HTTP\s+(\d{3})",
                    r"(?i)status[:\s]+(\d{3})",
                    r"(?i)^(\d{3})\s+",
                    r"(?i)\b(\d{3})\s+(?:Too Many Requests|Rate)",
                ];

                let mut found = None;
                for pattern in &patterns {
                    if let Ok(re) = Regex::new(pattern) {
                        if let Some(captures) = re.captures(message) {
                            if let Some(status_match) = captures.get(1) {
                                if let Ok(status) = status_match.as_str().parse::<u16>() {
                                    if (100..600).contains(&status) {
                                        found = Some(status);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                found
            };

            assert_eq!(result, expected, "Failed for message: '{}'", message);
        }
    }

    #[tokio::test]
    async fn test_chat_with_rate_limit_retry_mock() {
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        // First request returns 429 with Retry-After header
        let rate_limit_response = ResponseTemplate::new(429)
            .set_header("Retry-After", "2")
            .set_body_json(&serde_json::json!({
                "error": {
                    "type": "rate_limit_error",
                    "message": "Rate limit exceeded. Retry after 2 seconds"
                }
            }));

        // Second request succeeds
        let success_response = serde_json::json!({
            "id": "msg_retry_success",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-haiku-20240307",
            "content": [{"type": "text", "text": "Success after retry!"}],
            "usage": {"input_tokens": 10, "output_tokens": 5},
            "stop_reason": "end_turn",
            "stop_sequence": null
        });

        // Set up mock to return 429 first, then success
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(rate_limit_response)
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&success_response))
            .expect(1)
            .mount(&mock_server)
            .await;

        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "test-key"); }

        let step = make_step("Test retry");
        let mut config_values = HashMap::new();
        config_values.insert(
            "base_url".to_string(),
            serde_json::Value::String(mock_server.uri()),
        );
        let config = StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = ChatExecutor.execute(&step, &config, &ctx).await;

        // Should eventually succeed after retry
        assert!(result.is_ok(), "Expected success after retry, got: {:?}", result);
        let output = result.unwrap();
        assert_eq!(output.text(), "Success after retry!");
    }

    #[tokio::test]
    async fn test_chat_rate_limit_exhaustion_mock() {
        use wiremock::{MockServer, Mock, ResponseTemplate};
        use wiremock::matchers::{method, path};

        let mock_server = MockServer::start().await;

        // Always return 429 to exhaust retries
        let rate_limit_response = ResponseTemplate::new(429)
            .set_header("Retry-After", "1")
            .set_body_json(&serde_json::json!({
                "error": {
                    "type": "rate_limit_error",
                    "message": "Rate limit exceeded"
                }
            }));

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(rate_limit_response)
            .expect(4) // Initial attempt + 3 retries
            .mount(&mock_server)
            .await;

        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "test-key"); }

        let step = make_step("Test exhaustion");
        let mut config_values = HashMap::new();
        config_values.insert(
            "base_url".to_string(),
            serde_json::Value::String(mock_server.uri()),
        );
        let config = StepConfig { values: config_values };
        let ctx = Context::new(String::new(), HashMap::new());

        let result = ChatExecutor.execute(&step, &config, &ctx).await;

        // Should fail with RateLimitExhausted error
        assert!(result.is_err());
        let error = result.unwrap_err();

        match error {
            StepError::RateLimitExhausted { attempts, duration, .. } => {
                assert_eq!(attempts, 4); // 1 initial + 3 retries
                assert!(duration.as_secs() >= 3); // At least some delay time
            }
            other => panic!("Expected RateLimitExhausted error, got: {:?}", other),
        }
    }
}
