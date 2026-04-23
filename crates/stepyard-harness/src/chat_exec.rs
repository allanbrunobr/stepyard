//! Chat step executor (PR 5c commit 2 of Task #31) — testable seam for
//! the provider call.
//!
//! The harness talks to chat providers through a narrow trait so the
//! runtime can be exercised without depending on a real rig-core client.
//! PR 5c commit 2 ships the seam + the mock; PR 5c commit 3 wires the
//! production Rig adapter and flips the CLI adapter to accept chat
//! steps.
//!
//! # Cancellation contract
//!
//! Commit 2 is the happy-path contract: user-turn → `complete` →
//! assistant-turn → `StepCompleted`, matching the ordering the
//! [`chat_replay`](../tests/chat_replay.rs) integration test pins and
//! the atomicity `compute_progress` relies on. The cancel / signal /
//! step-timeout / shutdown-broadcast race that the cmd and agent paths
//! run (via `tokio::select!`) is deliberately deferred to commit 4's
//! hardening pass — mixing it into commit 2 would drag the provider
//! seam through the same blast radius and break the user's sequenced
//! commit plan.
//!
//! # What lives here vs. the Engine
//!
//! * [`ChatClient`] / [`ChatCompletionRequest`] / [`ChatCompletionResponse`] /
//!   [`ChatClientError`] — the pure seam. Nothing in this file touches
//!   the session log or the workflow types directly.
//! * [`run_chat_step`] — orchestration: build the request from a
//!   [`crate::workflow::Step`] + history, call the injected client,
//!   return a typed output the Engine lifts onto
//!   [`stepyard_core::Event::StepCompleted`].

use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt::Debug;

use stepyard_core::ChatMessage;

use crate::workflow::{ChatProvider, ChatTruncation, Step};

/// Provider-side call the harness delegates chat completions to.
///
/// Kept as a trait (rather than a concrete Rig client) so tests can
/// inject a canned-response mock without linking rig-core. The
/// production implementation lands in PR 5c commit 3.
///
/// `Debug` is a supertrait so [`crate::engine::HarnessConfig`] (which
/// now carries `Option<Arc<dyn ChatClient>>`) can keep its
/// `#[derive(Debug)]` without a manual impl.
#[async_trait]
pub trait ChatClient: Debug + Send + Sync {
    async fn complete(
        &self,
        req: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ChatClientError>;
}

/// Everything the provider needs to produce one assistant turn.
///
/// `history` is the committed prior turns for this chat-session bucket,
/// rebuilt by `compute_progress` from [`stepyard_core::Event::ChatMessageAppended`]
/// rows — the harness does **not** keep an in-memory transcript
/// between step calls (Invariante 11). The incoming user turn is
/// passed separately via `user_message` so the client doesn't have to
/// guess which tail entry is "the prompt this call is producing a
/// reply for."
#[derive(Debug, Clone)]
pub struct ChatCompletionRequest {
    /// Provider selector (Anthropic, OpenAI, ...). `None` means the
    /// adapter defaulted or the caller is going through the
    /// default-provider path.
    pub provider: Option<ChatProvider>,
    /// Model override, when the workflow pinned one. Providers that
    /// see `None` fall back to their own default.
    pub model: Option<String>,
    /// Response-length cap forwarded as `max_tokens`.
    pub max_tokens: Option<u64>,
    /// Sampling temperature forwarded to the provider.
    pub temperature: Option<f64>,
    /// Provider endpoint override (e.g. OpenAI-compatible gateways,
    /// Ollama host URL).
    pub base_url: Option<String>,
    /// API key value **already resolved** from the step-configured env
    /// var. `None` when the provider doesn't authenticate (Ollama) or
    /// when the client implementation reads the key itself.
    pub api_key: Option<String>,
    /// Reconstructed chat history from the session log, oldest-first.
    /// Empty on the first turn of a session.
    pub history: Vec<ChatMessage>,
    /// The turn that triggered this call — already rendered, ready to
    /// hand to the provider. Not yet persisted; the engine appends a
    /// `ChatMessageAppended { role: user, ... }` **before** invoking
    /// the client so replay sees the prompt even if the call fails.
    pub user_message: String,
    /// Truncation strategy carried through so the client can trim
    /// `history` before dispatch. `None` = send full history.
    pub truncation: Option<ChatTruncation>,
}

/// Structured output from [`ChatClient::complete`].
///
/// Token/cost fields are `Option<_>` because not every provider
/// reports them consistently; the Engine lifts them onto
/// [`stepyard_core::Event::StepCompleted`] as-is.
#[derive(Debug, Clone, Default)]
pub struct ChatCompletionResponse {
    /// Assistant turn to append to the session log and return as the
    /// step's `stdout`.
    pub content: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

/// Failure modes surfaced by [`ChatClient::complete`].
///
/// The variants split by stage so the Engine's terminal `StepFailed`
/// can carry a specific error string without reverse-engineering a
/// message format. `#[non_exhaustive]` so commit 3's production
/// adapter can add provider-specific variants without a major bump.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum ChatClientError {
    /// The provider returned a response the client could not parse or
    /// was structurally empty (no content, no error).
    #[error("provider returned an empty or malformed response: {message}")]
    EmptyResponse { message: String },

    /// Network / HTTP / SDK-level failure. Stringly-typed on purpose —
    /// the production client will translate rig-core's typed errors
    /// into this variant at the seam.
    #[error("chat provider call failed: {message}")]
    ProviderFailure { message: String },
}

/// Failure modes for [`run_chat_step`]. Split from [`ChatClientError`]
/// so orchestration-level problems (missing prompt, missing client)
/// stay distinguishable from provider-call problems in terminal
/// events.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub(crate) enum ChatExecError {
    /// The engine reached dispatch without an injected
    /// [`ChatClient`]. Commit 3 wires a production default; until then
    /// this surfaces as `StepFailed` rather than a panic so operators
    /// get a typed error.
    #[error(
        "chat step `{step}` reached dispatch with no chat client configured; \
         inject `HarnessConfig::chat_client` (commit 3 wires the production \
         default)"
    )]
    NoClientConfigured { step: String },

    /// The chat step has no `prompt:` field. The CLI adapter rejects
    /// this at load time (PR 5b), but an in-process caller that bypasses
    /// the adapter can still construct one — surface it loud.
    #[error(
        "chat step `{step}` has no prompt — the adapter should have rejected \
         this at load time"
    )]
    MissingPrompt { step: String },

    /// The provider call failed.
    #[error(transparent)]
    Client(#[from] ChatClientError),
}

/// Failure modes for [`resolve_api_key`].
///
/// `api_key_env` on a chat [`Step`] is the **name** of a host env var
/// whose value authenticates against the provider
/// (see `crate::workflow::Step::api_key_env`). The resolver consults
/// the workflow/step env map first (explicit override) then falls back
/// to `std::env`. A name that resolves to neither is a user-config
/// problem — surfaced as a typed variant so the Engine can emit
/// `StepFailed` with a specific message instead of a panic.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub(crate) enum ChatApiKeyError {
    /// The `api_key_env` name is absent from both the workflow env
    /// map and the host process env.
    #[error(
        "chat step `{step}` api_key_env `{key}` is not set in workflow env \
         or host env"
    )]
    NotSet { step: String, key: String },

    /// Host env var is set but holds bytes that are not valid UTF-8.
    /// Extremely rare on modern systems; surfaced as a distinct
    /// variant so operators can tell "missing" from "corrupt."
    #[error("chat step `{step}` api_key_env `{key}` holds non-UTF-8 data")]
    NotUnicode { step: String, key: String },
}

/// Resolve `api_key_env` (a variable NAME) to its value.
///
/// Precedence, per the field contract at
/// `crate::workflow::Step::api_key_env`:
/// 1. `resolved_env[key]` — the merged workflow/step env map.
///    Explicit injection wins so tests and ad-hoc workflows can
///    override host env without redundantly mirroring secrets.
/// 2. `std::env::var(key)` — the host process env. This is the normal
///    operator path: the adapter defaults `api_key_env` to names
///    like `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` and operators set
///    those in their shell, not in workflow YAML.
///
/// `None` input (e.g. the Ollama adapter default) short-circuits to
/// `Ok(None)` — local endpoints don't authenticate, and reading
/// host env for a name we never got would be a bug.
pub(crate) fn resolve_api_key(
    step_name: &str,
    api_key_env: Option<&str>,
    resolved_env: &HashMap<String, String>,
) -> Result<Option<String>, ChatApiKeyError> {
    let Some(key) = api_key_env else {
        return Ok(None);
    };
    if let Some(v) = resolved_env.get(key) {
        return Ok(Some(v.clone()));
    }
    match std::env::var(key) {
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Err(ChatApiKeyError::NotSet {
            step: step_name.into(),
            key: key.into(),
        }),
        Err(std::env::VarError::NotUnicode(_)) => Err(ChatApiKeyError::NotUnicode {
            step: step_name.into(),
            key: key.into(),
        }),
    }
}

/// Typed output of [`run_chat_step`]. Mirrors
/// [`crate::agent_exec::AgentExecOutput`] so the Engine can lift the
/// numeric fields onto `StepCompleted` without a second adapter layer.
#[derive(Debug, Default)]
pub(crate) struct ChatExecOutput {
    pub response: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
}

/// Build a [`ChatCompletionRequest`] from a chat [`Step`], the
/// reconstructed history, and the resolved user-facing prompt.
///
/// Kept separate from [`run_chat_step`] so unit tests can pin the
/// request-building contract without touching the trait's async seam.
pub(crate) fn build_chat_request(
    step: &Step,
    rendered_prompt: String,
    history: Vec<ChatMessage>,
    resolved_api_key: Option<String>,
) -> ChatCompletionRequest {
    ChatCompletionRequest {
        provider: step.chat_provider,
        model: step.model.clone(),
        max_tokens: step.max_tokens,
        temperature: step.temperature,
        base_url: step.base_url.clone(),
        api_key: resolved_api_key,
        history,
        user_message: rendered_prompt,
        truncation: step.truncation.clone(),
    }
}

/// Invoke the injected [`ChatClient`] and map its result onto a
/// [`ChatExecOutput`].
///
/// The Engine handles event emission around this call (user turn
/// before, assistant turn + StepCompleted after) so the commit-
/// boundary atomicity PR 5b's `compute_progress` relies on stays
/// enforced by the Engine, not by every chat client implementation.
pub(crate) async fn run_chat_step(
    step: &Step,
    req: ChatCompletionRequest,
    client: &dyn ChatClient,
) -> Result<ChatExecOutput, ChatExecError> {
    if step.prompt.as_deref().is_none_or(str::is_empty) {
        return Err(ChatExecError::MissingPrompt {
            step: step.name.clone(),
        });
    }
    let resp = client.complete(req).await?;
    Ok(ChatExecOutput {
        response: resp.content,
        input_tokens: resp.input_tokens,
        output_tokens: resp.output_tokens,
        cost_usd: resp.cost_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use stepyard_core::ChatRole;

    #[derive(Debug)]
    struct StubClient {
        reply: String,
    }

    #[async_trait]
    impl ChatClient for StubClient {
        async fn complete(
            &self,
            _req: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, ChatClientError> {
            Ok(ChatCompletionResponse {
                content: self.reply.clone(),
                ..Default::default()
            })
        }
    }

    #[derive(Debug)]
    struct FailingClient;

    #[async_trait]
    impl ChatClient for FailingClient {
        async fn complete(
            &self,
            _req: ChatCompletionRequest,
        ) -> Result<ChatCompletionResponse, ChatClientError> {
            Err(ChatClientError::ProviderFailure {
                message: "boom".into(),
            })
        }
    }

    fn chat_step_with_prompt(prompt: &str) -> Step {
        let mut s = Step::chat("ask", prompt);
        s.chat_session = Some("shared".into());
        s
    }

    #[test]
    fn build_chat_request_forwards_step_fields() {
        let mut step = chat_step_with_prompt("Hi");
        step.model = Some("claude-3-opus".into());
        step.max_tokens = Some(512);
        step.temperature = Some(0.2);
        step.base_url = Some("https://example.test".into());
        step.chat_provider = Some(ChatProvider::Anthropic);

        let history = vec![ChatMessage {
            role: ChatRole::User,
            content: "prior".into(),
        }];
        let req = build_chat_request(&step, "Hi".into(), history.clone(), Some("sk-...".into()));

        assert_eq!(req.user_message, "Hi");
        assert_eq!(req.history, history);
        assert_eq!(req.model.as_deref(), Some("claude-3-opus"));
        assert_eq!(req.max_tokens, Some(512));
        assert_eq!(req.temperature, Some(0.2));
        assert_eq!(req.base_url.as_deref(), Some("https://example.test"));
        assert_eq!(req.api_key.as_deref(), Some("sk-..."));
        assert!(matches!(req.provider, Some(ChatProvider::Anthropic)));
    }

    #[tokio::test]
    async fn run_chat_step_returns_client_content() {
        let step = chat_step_with_prompt("Hello");
        let req = build_chat_request(&step, "Hello".into(), vec![], None);
        let client = StubClient {
            reply: "hi back".into(),
        };
        let out = run_chat_step(&step, req, &client).await.unwrap();
        assert_eq!(out.response, "hi back");
    }

    #[tokio::test]
    async fn run_chat_step_surfaces_client_errors_as_client_variant() {
        let step = chat_step_with_prompt("Hello");
        let req = build_chat_request(&step, "Hello".into(), vec![], None);
        let err = run_chat_step(&step, req, &FailingClient).await.unwrap_err();
        assert!(
            matches!(
                err,
                ChatExecError::Client(ChatClientError::ProviderFailure { .. })
            ),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn run_chat_step_rejects_missing_prompt() {
        let mut step = chat_step_with_prompt("placeholder");
        step.prompt = None;
        let req = build_chat_request(&step, String::new(), vec![], None);
        let err = run_chat_step(&step, req, &StubClient { reply: "x".into() })
            .await
            .unwrap_err();
        assert!(
            matches!(err, ChatExecError::MissingPrompt { .. }),
            "got {err:?}"
        );
    }

    // -----------------------------------------------------------------
    // `resolve_api_key` — precedence contract.
    //
    // `api_key_env` on a chat Step is the NAME of a host env var, not
    // a key into the workflow env map. Reviewer-flagged blocker on PR
    // 5c commit 2: the first draft read the value from `resolved_env`
    // directly, which would have broken every adapter-defaulted
    // provider (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, …) the moment
    // commit 3 opened chat through the public CLI. These tests pin
    // the fix: explicit workflow override wins, host env is the
    // fallback, Ollama (None) stays None, missing everywhere surfaces
    // a typed error.
    //
    // Env var names are unique per test so parallel `cargo test` runs
    // don't race on the process-global env — `std::env::set_var` is a
    // shared mutation and the only safe way to keep tests isolated
    // without pulling `serial_test` into this crate's dev-deps.
    // -----------------------------------------------------------------
    #[test]
    fn resolve_api_key_none_short_circuits_to_none() {
        // Ollama path: the adapter defaults `api_key_env` to None for
        // local endpoints. The resolver must not consult host env in
        // that case — reading a key we never got would be a bug.
        let env = HashMap::new();
        let got = resolve_api_key("ask", None, &env).expect("Ollama path");
        assert_eq!(got, None);
    }

    #[test]
    fn resolve_api_key_prefers_workflow_env_over_host_env() {
        let key = "STEPYARD_CHAT_TEST_API_KEY_OVERRIDE_7413";
        std::env::set_var(key, "from-host");
        let mut env = HashMap::new();
        env.insert(key.to_string(), "from-workflow".to_string());

        let got = resolve_api_key("ask", Some(key), &env).expect("override path");
        assert_eq!(
            got.as_deref(),
            Some("from-workflow"),
            "workflow env must win over host env so explicit injection \
             in `env:` still overrides the operator shell"
        );

        std::env::remove_var(key);
    }

    #[test]
    fn resolve_api_key_falls_back_to_host_env_when_workflow_empty() {
        // The commit 2 fix: adapter-defaulted names (e.g.
        // `ANTHROPIC_API_KEY`) resolve from host env without
        // operators redundantly mirroring secrets into workflow
        // `env:`. Before the fix, a normal chat step failed with
        // "not set in the resolved env" even when the key was
        // exported in the shell.
        let key = "STEPYARD_CHAT_TEST_API_KEY_HOST_2956";
        std::env::set_var(key, "from-host-only");
        let env = HashMap::new();

        let got = resolve_api_key("ask", Some(key), &env).expect("host env fallback");
        assert_eq!(got.as_deref(), Some("from-host-only"));

        std::env::remove_var(key);
    }

    #[test]
    fn resolve_api_key_missing_everywhere_surfaces_typed_not_set() {
        // Sad path: name absent from both workflow env and host env
        // yields a typed error. Engine lifts this onto `StepFailed`
        // with the variant's Display — never a panic or silent None.
        let key = "STEPYARD_CHAT_TEST_API_KEY_MISSING_8204";
        std::env::remove_var(key);
        let env = HashMap::new();

        let err = resolve_api_key("ask", Some(key), &env).expect_err("must fail");
        match err {
            ChatApiKeyError::NotSet {
                step: ref s,
                key: ref k,
            } => {
                assert_eq!(s, "ask");
                assert_eq!(k, key);
            }
            other => panic!("expected NotSet, got {other:?}"),
        }
    }
}
