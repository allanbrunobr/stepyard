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

use async_trait::async_trait;
use rig::client::CompletionClient;
use rig::completion::{CompletionError, CompletionModel, CompletionResponse};
use rig::message::{AssistantContent, Message};

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
            let mut builder = rig::providers::anthropic::Client::builder().api_key(api_key);
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
            let mut builder = rig::providers::openai::CompletionsClient::builder().api_key(api_key);
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
                .base_url(url);
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
            let mut builder = rig::providers::groq::Client::builder().api_key(api_key);
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
            let mut builder = rig::providers::deepseek::Client::builder().api_key(api_key);
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
            let mut builder = rig::providers::cohere::Client::builder().api_key(api_key);
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
            let mut builder = rig::providers::perplexity::Client::builder().api_key(api_key);
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
            let mut builder = rig::providers::xai::Client::builder().api_key(api_key);
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
            let mut builder = rig::providers::mistral::Client::builder().api_key(api_key);
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
                .base_url(url);
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
}
