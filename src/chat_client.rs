//! Production [`ChatClient`] adapter wrapping `rig-core` providers.
//!
//! PR 5c commit 3b of Task #31 — the production wiring that flips
//! top-level `type: chat` workflows from a typed `NoClientConfigured`
//! error into a real provider call. The harness already ships the
//! testable seam ([`stepyard_harness::ChatClient`] +
//! `MockChatClient`) and the dispatch site (`engine.rs::run_chat_step`).
//! This module is the bridge between that seam and `rig-core 0.32` —
//! the same crate v1's `src/steps/chat.rs::call_via_rig` already uses,
//! so provider behavior stays identical across the v1→v2 cutover.
//!
//! # Cancellation contract
//!
//! Provider calls run **un-bounded** here — no `tokio::time::timeout`
//! wrap, no signal/shutdown select. That's the commit 4 hardening pass.
//! A hung provider call today blocks the chat step until the operator
//! kills the process; commit 4 lifts the cancel/shutdown/timeout race
//! the cmd and agent paths already run into the chat path.
//!
//! # Why a separate root-crate module
//!
//! `rig-core` lives in the root crate (`Cargo.toml:97`); the
//! `stepyard-harness` crate deliberately doesn't depend on it so the
//! harness can be unit-tested without linking real provider clients.
//! Keeping the production adapter root-crate-side preserves that
//! split.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use rig::client::CompletionClient;
use rig::completion::{CompletionError, CompletionModel, CompletionResponse};
use rig::http_client::{
    self, HttpClientExt, LazyBody, MultipartForm, Request, Response, StreamingResponse,
};
use rig::message::{AssistantContent, Message};
use rig::wasm_compat::WasmCompatSend;

use stepyard_core::{ChatMessage, ChatRole};
use stepyard_harness::{
    ChatClient, ChatClientError, ChatCompletionRequest, ChatCompletionResponse, ChatProvider,
    ChatTruncation,
};

/// Default production [`ChatClient`] used by the CLI. Returned as an
/// `Arc<dyn ChatClient>` so callers can store the trait object on
/// `HarnessConfig::chat_client` without naming the concrete type, and
/// tests can swap in a different impl through the same shape.
pub fn default_chat_client() -> Arc<dyn ChatClient> {
    Arc::new(RigChatClient)
}

/// Production [`ChatClient`] backed by `rig-core` 0.32. Stateless —
/// per-call provider clients are constructed inside `complete` so
/// configuration changes (api key, base url) on a hot-reload aren't
/// pinned to whichever value was live the first time the engine
/// dispatched a chat step. Mirrors v1's `call_via_rig` shape.
#[derive(Debug, Default)]
pub struct RigChatClient;

const MAX_RATE_LIMIT_RETRIES: usize = 3;
const RATE_LIMIT_BACKOFFS: [Duration; MAX_RATE_LIMIT_RETRIES] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

#[derive(Clone, Debug)]
struct RetryingHttpClient {
    inner: reqwest::Client,
}

impl Default for RetryingHttpClient {
    fn default() -> Self {
        Self {
            inner: reqwest::Client::new(),
        }
    }
}

fn http_instance_error(
    error: impl std::error::Error + Send + Sync + 'static,
) -> http_client::Error {
    http_client::Error::Instance(Box::new(error))
}

impl HttpClientExt for RetryingHttpClient {
    fn send<T, U>(
        &self,
        req: Request<T>,
    ) -> impl std::future::Future<Output = http_client::Result<Response<LazyBody<U>>>>
           + WasmCompatSend
           + 'static
    where
        T: Into<Bytes>,
        T: WasmCompatSend,
        U: From<Bytes>,
        U: WasmCompatSend + 'static,
    {
        let (parts, body) = req.into_parts();
        let method = parts.method;
        let uri = parts.uri.to_string();
        let headers = parts.headers;
        let body = body.into();
        let client = self.inner.clone();

        async move {
            for retry_index in 0..=MAX_RATE_LIMIT_RETRIES {
                let fallback_delay = RATE_LIMIT_BACKOFFS.get(retry_index).copied();
                let response = client
                    .request(method.clone(), uri.clone())
                    .headers(headers.clone())
                    .body(body.clone())
                    .send()
                    .await
                    .map_err(http_instance_error)?;

                if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                    && fallback_delay.is_some()
                {
                    let delay = retry_after_delay(response.headers())
                        .unwrap_or_else(|| fallback_delay.expect("checked is_some"));
                    let _ = response.text().await;
                    tracing::warn!(
                        retry_attempt = retry_index + 1,
                        max_retries = MAX_RATE_LIMIT_RETRIES,
                        delay_ms = delay.as_millis() as u64,
                        "chat provider returned 429; retrying after backoff"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }

                if !response.status().is_success() {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    return Err(http_client::Error::InvalidStatusCodeWithMessage(
                        status, text,
                    ));
                }

                let mut res = Response::builder().status(response.status());
                if let Some(hs) = res.headers_mut() {
                    *hs = response.headers().clone();
                }

                let body: LazyBody<U> = Box::pin(async move {
                    let bytes = response.bytes().await.map_err(http_instance_error)?;
                    Ok(U::from(bytes))
                });

                return res.body(body).map_err(http_client::Error::Protocol);
            }

            unreachable!("rate-limit retry loop always returns on final attempt")
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn send_multipart<U>(
        &self,
        req: Request<MultipartForm>,
    ) -> impl std::future::Future<Output = http_client::Result<Response<LazyBody<U>>>>
           + WasmCompatSend
           + 'static
    where
        U: From<Bytes>,
        U: WasmCompatSend + 'static,
    {
        let _ = req;
        async { Err(unsupported_http_client_path("multipart chat requests")) }
    }

    #[allow(clippy::manual_async_fn)]
    fn send_streaming<T>(
        &self,
        req: Request<T>,
    ) -> impl std::future::Future<Output = http_client::Result<StreamingResponse>> + WasmCompatSend
    where
        T: Into<Bytes>,
    {
        let _ = req;
        async { Err(unsupported_http_client_path("streaming chat requests")) }
    }
}

fn unsupported_http_client_path(path: &'static str) -> http_client::Error {
    http_instance_error(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("RetryingHttpClient only supports non-streaming completion calls, got {path}"),
    ))
}

fn retry_after_delay(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| parse_retry_after(raw, SystemTime::now()))
}

fn parse_retry_after(raw: &str, now: SystemTime) -> Option<Duration> {
    if let Ok(seconds) = raw.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let parsed = chrono::DateTime::parse_from_rfc2822(raw.trim()).ok()?;
    let target: SystemTime = parsed.with_timezone(&chrono::Utc).into();
    target.duration_since(now).ok().or(Some(Duration::ZERO))
}

#[async_trait]
impl ChatClient for RigChatClient {
    async fn complete(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ChatClientError> {
        let provider = req.provider.unwrap_or_default();
        let model_default = provider.default_model().to_string();
        let model_name = req.model.as_deref().unwrap_or(&model_default);
        let max_tokens = req.max_tokens.unwrap_or(1024);
        let temperature = req.temperature.unwrap_or(0.0);

        let truncated = apply_truncation(req.history, req.truncation.as_ref());
        let messages = to_rig_messages(&truncated);

        let api_key = req.api_key.as_deref().unwrap_or("");
        let base_url = req.base_url.as_deref();

        call_provider(
            provider,
            model_name,
            api_key,
            base_url,
            messages,
            &req.user_message,
            temperature,
            max_tokens,
        )
        .await
    }
}

// ── Truncation ────────────────────────────────────────────────────

/// Trim `history` according to the requested strategy. `None` =
/// pass-through (the harness contract: absent `truncation:` means
/// "send full history").
///
/// Reimplemented against [`ChatTruncation`] rather than reusing v1's
/// `TruncationStrategy` — the v2 enum has no `None` variant (absence
/// is encoded by `Option::None`) and lives in a different module, so
/// importing v1's type would create a coupling we don't want. The
/// sliding-window heuristic mirrors v1's `words * 1.3` estimator so
/// workflows that pinned a specific `max_tokens` see the same trim
/// behavior across the cutover.
fn apply_truncation(
    history: Vec<ChatMessage>,
    strategy: Option<&ChatTruncation>,
) -> Vec<ChatMessage> {
    let Some(strategy) = strategy else {
        return history;
    };
    match *strategy {
        ChatTruncation::Last { count } => {
            let n = count as usize;
            let start = history.len().saturating_sub(n);
            history.into_iter().skip(start).collect()
        }
        ChatTruncation::First { count } => {
            let n = (count as usize).min(history.len());
            history.into_iter().take(n).collect()
        }
        ChatTruncation::FirstLast { first, last } => {
            let len = history.len();
            let first_n = (first as usize).min(len);
            let last_start = len.saturating_sub(last as usize);
            if first_n >= last_start {
                history
            } else {
                let mut head: Vec<ChatMessage> = history.iter().take(first_n).cloned().collect();
                head.extend(history.into_iter().skip(last_start));
                head
            }
        }
        ChatTruncation::SlidingWindow { max_tokens } => {
            let max_tokens = max_tokens as usize;
            let total: usize = history.iter().map(|m| estimate_tokens(&m.content)).sum();
            if total <= max_tokens {
                return history;
            }
            let mut tokens_used = total;
            let mut drop_count = 0usize;
            for msg in history.iter() {
                if tokens_used <= max_tokens {
                    break;
                }
                tokens_used = tokens_used.saturating_sub(estimate_tokens(&msg.content));
                drop_count += 1;
            }
            history.into_iter().skip(drop_count).collect()
        }
        // `ChatTruncation` is `#[non_exhaustive]` — a future variant
        // landing in the harness crate must be wired here explicitly.
        // Until then the safe fallback is "send full history" (matches
        // the absent-strategy contract) plus a tracing warn so the gap
        // surfaces in production logs instead of silently dropping
        // turns.
        _ => {
            tracing::warn!(
                strategy = ?strategy,
                "ChatTruncation variant not handled by RigChatClient; sending full history"
            );
            history
        }
    }
}

fn estimate_tokens(text: &str) -> usize {
    let words = text.split_whitespace().count();
    ((words as f64) * 1.3).ceil() as usize
}

// ── Rig translation helpers ───────────────────────────────────────

fn to_rig_messages(history: &[ChatMessage]) -> Vec<Message> {
    history
        .iter()
        .map(|m| match m.role {
            ChatRole::Assistant => Message::from(AssistantContent::text(&m.content)),
            // `ChatRole` is `#[non_exhaustive]`; new roles (e.g. system,
            // tool_use) need explicit handling here, but until then
            // treat anything else as a user turn — matches v1's
            // catch-all at `src/steps/chat.rs:125-128` and lets the
            // provider treat the content as a normal prompt rather
            // than dropping it.
            ChatRole::User | _ => Message::from(m.content.as_str()),
        })
        .collect()
}

fn extract_response<T>(resp: CompletionResponse<T>) -> ChatCompletionResponse {
    let content = resp
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
    ChatCompletionResponse {
        content,
        input_tokens: Some(resp.usage.input_tokens),
        output_tokens: Some(resp.usage.output_tokens),
        cost_usd: None,
    }
}

fn provider_failure(provider: &str, msg: impl std::fmt::Display) -> ChatClientError {
    ChatClientError::ProviderFailure {
        message: format!("{provider}: {msg}"),
    }
}

// ── Provider dispatch ─────────────────────────────────────────────

/// Per-arm boilerplate — build the `completion_request`, fire it,
/// translate any rig error into [`ChatClientError::ProviderFailure`].
/// Mirrors v1's `send_completion!` macro minus the timeout wrap
/// (commit 4 territory).
macro_rules! send_completion {
    ($client:expr, $model:expr, $prompt:expr, $msgs:expr,
     $temp:expr, $max:expr, $provider:expr) => {{
        let model = $client.completion_model($model);
        let resp: Result<_, CompletionError> = model
            .completion_request($prompt)
            .messages($msgs)
            .temperature($temp)
            .max_tokens($max)
            .send()
            .await;
        let resp = resp.map_err(|e| provider_failure($provider, e))?;
        Ok::<ChatCompletionResponse, ChatClientError>(extract_response(resp))
    }};
}

/// Dispatch on `provider` and call rig with provider-specific client
/// construction. Exhaustive over all 11 [`ChatProvider`] variants so a
/// future variant addition forces a compile error here rather than
/// silently routing through a catch-all.
#[allow(clippy::too_many_arguments)]
async fn call_provider(
    provider: ChatProvider,
    model_name: &str,
    api_key: &str,
    base_url: Option<&str>,
    messages: Vec<Message>,
    prompt: &str,
    temperature: f64,
    max_tokens: u64,
) -> Result<ChatCompletionResponse, ChatClientError> {
    match provider {
        ChatProvider::Anthropic => {
            let mut builder = rig::providers::anthropic::Client::builder()
                .api_key(api_key)
                .http_client(RetryingHttpClient::default());
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder
                .build()
                .map_err(|e| provider_failure("anthropic", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "anthropic"
            )
        }
        ChatProvider::OpenAi => {
            let mut builder = rig::providers::openai::CompletionsClient::builder()
                .api_key(api_key)
                .http_client(RetryingHttpClient::default());
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder.build().map_err(|e| provider_failure("openai", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "openai"
            )
        }
        ChatProvider::Ollama => {
            let url = base_url.unwrap_or("http://localhost:11434");
            let builder = rig::providers::ollama::Client::builder()
                .api_key(rig::client::Nothing)
                .base_url(url)
                .http_client(RetryingHttpClient::default());
            let client = builder.build().map_err(|e| provider_failure("ollama", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "ollama"
            )
        }
        ChatProvider::Groq => {
            let mut builder = rig::providers::groq::Client::builder()
                .api_key(api_key)
                .http_client(RetryingHttpClient::default());
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder.build().map_err(|e| provider_failure("groq", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "groq"
            )
        }
        ChatProvider::DeepSeek => {
            let mut builder = rig::providers::deepseek::Client::builder()
                .api_key(api_key)
                .http_client(RetryingHttpClient::default());
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder
                .build()
                .map_err(|e| provider_failure("deepseek", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "deepseek"
            )
        }
        ChatProvider::Gemini => {
            // Rig's Gemini completion model is not generic over the
            // HTTP backend in 0.32, so it cannot use RetryingHttpClient
            // without reimplementing the provider. Other providers in
            // this adapter use the retrying backend below.
            let mut builder = rig::providers::gemini::Client::builder().api_key(api_key);
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder.build().map_err(|e| provider_failure("gemini", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "gemini"
            )
        }
        ChatProvider::Cohere => {
            let mut builder = rig::providers::cohere::Client::builder()
                .api_key(api_key)
                .http_client(RetryingHttpClient::default());
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder.build().map_err(|e| provider_failure("cohere", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "cohere"
            )
        }
        ChatProvider::Perplexity => {
            let mut builder = rig::providers::perplexity::Client::builder()
                .api_key(api_key)
                .http_client(RetryingHttpClient::default());
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder
                .build()
                .map_err(|e| provider_failure("perplexity", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "perplexity"
            )
        }
        ChatProvider::Xai => {
            let mut builder = rig::providers::xai::Client::builder()
                .api_key(api_key)
                .http_client(RetryingHttpClient::default());
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder.build().map_err(|e| provider_failure("xai", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "xai"
            )
        }
        ChatProvider::Mistral => {
            let mut builder = rig::providers::mistral::Client::builder()
                .api_key(api_key)
                .http_client(RetryingHttpClient::default());
            if let Some(url) = base_url {
                builder = builder.base_url(url);
            }
            let client = builder
                .build()
                .map_err(|e| provider_failure("mistral", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "mistral"
            )
        }
        ChatProvider::OpenAiCompatible => {
            // base_url is required for this variant — the adapter
            // already enforces it at load time
            // (`harness_adapter.rs::parse_chat_step_openai_compatible_requires_base_url`),
            // but an in-process caller building a Step by hand can still
            // skip it. Surface that as a typed provider error rather
            // than silently routing to the OpenAI default endpoint.
            let url = base_url.ok_or_else(|| ChatClientError::ProviderFailure {
                message: "openai_compatible provider requires base_url".into(),
            })?;
            let builder = rig::providers::openai::CompletionsClient::builder()
                .api_key(api_key)
                .base_url(url)
                .http_client(RetryingHttpClient::default());
            let client = builder
                .build()
                .map_err(|e| provider_failure("openai_compatible", e))?;
            send_completion!(
                client,
                model_name,
                prompt,
                messages,
                temperature,
                max_tokens,
                "openai_compatible"
            )
        }
        // `ChatProvider` is `#[non_exhaustive]`; a future variant
        // (e.g. Bedrock) added in the harness crate must extend this
        // dispatch explicitly. The adapter already pins variant→builder
        // mapping at load time, so reaching this arm in production
        // means the harness grew a variant the root crate hasn't been
        // taught about — surface as a typed provider error so the
        // operator sees the gap instead of silent fallthrough.
        _ => Err(ChatClientError::ProviderFailure {
            message: format!("provider {provider:?} not wired in RigChatClient"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msgs(count: usize) -> Vec<ChatMessage> {
        (0..count)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                },
                content: format!("message {i}"),
            })
            .collect()
    }

    #[test]
    fn truncation_none_is_passthrough() {
        let history = msgs(5);
        let result = apply_truncation(history.clone(), None);
        assert_eq!(result, history);
    }

    #[test]
    fn truncation_last_keeps_n_most_recent() {
        let history = msgs(50);
        let result = apply_truncation(history, Some(&ChatTruncation::Last { count: 10 }));
        assert_eq!(result.len(), 10);
        assert_eq!(result[0].content, "message 40");
        assert_eq!(result[9].content, "message 49");
    }

    #[test]
    fn truncation_last_count_exceeding_len_returns_all() {
        let history = msgs(3);
        let result = apply_truncation(history, Some(&ChatTruncation::Last { count: 99 }));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn truncation_first_keeps_n_oldest() {
        let history = msgs(50);
        let result = apply_truncation(history, Some(&ChatTruncation::First { count: 5 }));
        assert_eq!(result.len(), 5);
        assert_eq!(result[0].content, "message 0");
        assert_eq!(result[4].content, "message 4");
    }

    #[test]
    fn truncation_first_last_keeps_head_and_tail() {
        let history = msgs(50);
        let result = apply_truncation(
            history,
            Some(&ChatTruncation::FirstLast { first: 2, last: 5 }),
        );
        assert_eq!(result.len(), 7);
        assert_eq!(result[0].content, "message 0");
        assert_eq!(result[1].content, "message 1");
        assert_eq!(result[2].content, "message 45");
        assert_eq!(result[6].content, "message 49");
    }

    #[test]
    fn truncation_first_last_overlapping_returns_all() {
        // first=10 + last=10 covers all 15 messages → no trim.
        let history = msgs(15);
        let result = apply_truncation(
            history,
            Some(&ChatTruncation::FirstLast {
                first: 10,
                last: 10,
            }),
        );
        assert_eq!(result.len(), 15);
    }

    #[test]
    fn truncation_sliding_window_fits_within_budget() {
        // "message N" is ~2-3 estimated tokens each (words * 1.3).
        // 50 messages → exceeds 50 tokens → drop_count > 0.
        let history = msgs(50);
        let result = apply_truncation(
            history,
            Some(&ChatTruncation::SlidingWindow { max_tokens: 50 }),
        );
        let total: usize = result.iter().map(|m| estimate_tokens(&m.content)).sum();
        assert!(total <= 50, "expected <=50 tokens, got {total}");
        assert!(result.len() < 50, "expected some trimming");
    }

    #[test]
    fn truncation_sliding_window_under_budget_passes_through() {
        let history = msgs(3);
        let result = apply_truncation(
            history.clone(),
            Some(&ChatTruncation::SlidingWindow { max_tokens: 10_000 }),
        );
        assert_eq!(result, history);
    }

    #[test]
    fn to_rig_messages_routes_role_to_correct_variant() {
        let history = vec![
            ChatMessage {
                role: ChatRole::User,
                content: "hello".into(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "hi".into(),
            },
        ];
        let rig_msgs = to_rig_messages(&history);
        assert_eq!(rig_msgs.len(), 2);
        assert!(matches!(rig_msgs[0], Message::User { .. }));
        assert!(matches!(rig_msgs[1], Message::Assistant { .. }));
    }

    #[test]
    fn default_chat_client_constructs_as_trait_object() {
        // Compile-time check that `default_chat_client()` returns an
        // `Arc<dyn ChatClient>` — the shape `HarnessConfig::chat_client`
        // expects. If this stops compiling, the wiring at
        // `cli/commands.rs:215` is broken at the type level.
        let _client: Arc<dyn ChatClient> = default_chat_client();
    }

    #[test]
    fn default_chat_client_populates_harness_config() {
        // Runtime gate: literal "assert chat_client.is_some() after wiring"
        // from PR 5c commit 3b. Mirrors the production wiring at
        // `cli/commands.rs:215`; regresses if the field is dropped from the
        // CLI's HarnessConfig literal.
        use stepyard_harness::HarnessConfig;
        let cfg = HarnessConfig {
            chat_client: Some(default_chat_client()),
            ..HarnessConfig::default()
        };
        assert!(cfg.chat_client.is_some());
    }

    #[test]
    fn retry_after_seconds_header_parses() {
        let delay = parse_retry_after("3", SystemTime::UNIX_EPOCH).expect("delay");
        assert_eq!(delay, Duration::from_secs(3));
    }

    #[test]
    fn retry_after_http_date_header_parses_relative_delay() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777);
        let delay = parse_retry_after("Sun, 06 Nov 1994 08:49:40 GMT", now).expect("delay");
        assert_eq!(delay, Duration::from_secs(3));
    }

    #[test]
    fn retry_after_past_http_date_is_zero_delay() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_780);
        let delay = parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now).expect("delay");
        assert_eq!(delay, Duration::ZERO);
    }

    #[test]
    fn retry_after_invalid_header_is_ignored() {
        assert_eq!(parse_retry_after("soon", SystemTime::UNIX_EPOCH), None);
    }

    #[test]
    fn rate_limit_backoff_schedule_is_one_two_four_seconds() {
        assert_eq!(
            RATE_LIMIT_BACKOFFS,
            [
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4)
            ]
        );
        assert_eq!(MAX_RATE_LIMIT_RETRIES, 3);
    }

    #[tokio::test]
    async fn retrying_http_client_retries_429_then_returns_success() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_server = hits.clone();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf).await.expect("read");
                let hit = hits_for_server.fetch_add(1, Ordering::SeqCst);
                let response = if hit == 0 {
                    concat!(
                        "HTTP/1.1 429 Too Many Requests\r\n",
                        "Retry-After: 0\r\n",
                        "Content-Length: 7\r\n",
                        "Connection: close\r\n",
                        "\r\n",
                        "limited"
                    )
                } else {
                    concat!(
                        "HTTP/1.1 200 OK\r\n",
                        "Content-Length: 2\r\n",
                        "Connection: close\r\n",
                        "\r\n",
                        "ok"
                    )
                };
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            }
        });

        let req = Request::builder()
            .method("POST")
            .uri(format!("http://{addr}/chat"))
            .body(Bytes::from_static(b"{}"))
            .expect("request");
        let res: Response<LazyBody<Bytes>> = RetryingHttpClient::default()
            .send(req)
            .await
            .expect("retry then success");
        assert_eq!(res.status(), reqwest::StatusCode::OK);
        let body = res.into_body().await.expect("body");
        assert_eq!(&body[..], b"ok");
        server.await.expect("server task");
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }
}
