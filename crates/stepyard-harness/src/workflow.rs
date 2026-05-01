//! Workflow representation for the harness.
//!
//! Story 2.3 landed a cmd-only shape. PR 1 of Task #31 (v2 migration) widened
//! the data model to represent every step kind plus named scopes so the
//! harness can parse and round-trip the full legacy shape. PR 2 added
//! gate-specific fields (`condition` / `on_pass` / `on_fail` / `message`);
//! PR 3 adds the container fields (`scope` / `max_iterations` / `items` /
//! `parallel` / `initial_value` / `outputs`) that `call` / `repeat` / `map`
//! need, plus the scope runner that executes them. PR 5a wires the
//! Claude-CLI agent executor; PR 5b adds the chat-step data model
//! (`chat_provider` / `max_tokens` / `temperature` / `api_key_env` /
//! `base_url` / `chat_session` / `truncation`), with the rig-core
//! runtime landing in a follow-up PR. **Executable kinds today:**
//! `cmd`, `gate`, `call`, `repeat`, `map`, `parallel`, `template`,
//! `script`, `agent`, and `chat`.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use stepyard_core::duration as core_duration;
use stepyard_core::env as core_env;
use stepyard_core::EngineError;
use stepyard_sandbox_orchestrator::BranchStrategy;

/// Every step kind the harness can represent.
///
/// Executable today: [`StepKind::Cmd`], [`StepKind::Gate`], [`StepKind::Call`],
/// [`StepKind::Repeat`], [`StepKind::Map`], [`StepKind::Template`],
/// [`StepKind::Script`], [`StepKind::Agent`], [`StepKind::Chat`], and
/// [`StepKind::Parallel`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    #[default]
    Cmd,
    Agent,
    Chat,
    Gate,
    Repeat,
    Map,
    Parallel,
    Call,
    Template,
    Script,
}

/// YAML-facing branch strategy. YAML deliberately uses snake_case while the
/// CLI accepts kebab-case flags and maps them into [`BranchStrategy`] at the
/// command boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BranchStrategyYaml {
    #[default]
    Head,
    MergeToHead,
    NamedBranch,
}

impl BranchStrategyYaml {
    fn is_head(&self) -> bool {
        matches!(self, Self::Head)
    }
}

impl StepKind {
    fn is_cmd(&self) -> bool {
        matches!(self, StepKind::Cmd)
    }
}

impl std::fmt::Display for StepKind {
    /// Renders the same snake_case label the engine writes to
    /// `step_type` on emitted events, so display/log/CLI can share one
    /// source of truth.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StepKind::Cmd => "cmd",
            StepKind::Agent => "agent",
            StepKind::Chat => "chat",
            StepKind::Gate => "gate",
            StepKind::Repeat => "repeat",
            StepKind::Map => "map",
            StepKind::Parallel => "parallel",
            StepKind::Call => "call",
            StepKind::Template => "template",
            StepKind::Script => "script",
        };
        f.write_str(s)
    }
}

/// Claude CLI permission posture for [`StepKind::Agent`] steps. Maps to
/// the absence/presence of `--dangerously-skip-permissions` on the child
/// process argv (PR 5a of Task #31). Stored as a typed enum on [`Step`]
/// — v1 carried this as a free-form string in its [`StepConfig`] bag
/// (`"default"` / `"skip"`), which v2 tightens so unknown values fail at
/// the adapter boundary rather than silently becoming the default.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentPermissions {
    /// Run the CLI with its standard permission prompts. No extra flags.
    #[default]
    Default,
    /// Append `--dangerously-skip-permissions` to the CLI argv. Opt-in
    /// only; the adapter never infers this.
    Skip,
}

/// Workflow-level session mode for [`StepKind::Agent`] steps that don't
/// declare explicit `resume:` / `fork_session:` (PR 5a of Task #31).
/// Mirrors v1's `session:` YAML key (`"shared"` / `"isolated"`). When
/// [`Self::Shared`], the executor feeds the first captured
/// `agent_session_id` from the session log via `--resume`; when
/// [`Self::Isolated`], each agent step starts fresh.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionMode {
    /// Continue the workflow's shared session. Default when the field is
    /// absent — matches v1's `SessionManager` first-wins semantics.
    #[default]
    Shared,
    /// Start a brand new session for this step. No `--resume` args are
    /// derived from the session log.
    Isolated,
}

/// Chat provider for [`StepKind::Chat`] steps (PR 5b of Task #31). Maps to
/// the rig-core client the runtime instantiates. v1 carried this as a
/// free-form `provider:` string in its `StepConfig` bag, routed through
/// a match arm with silent fallback to `anthropic` on unknown values.
/// v2 tightens the contract so unknown providers fail at the adapter
/// boundary rather than silently becoming the default.
///
/// v1 aliases (`"google"` → [`Self::Gemini`], `"grok"` → [`Self::Xai`])
/// are translated at the adapter boundary, not on this enum.
/// [`Self::OpenAiCompatible`] is a unit variant because [`Step::base_url`]
/// is already the single source of truth for the endpoint override —
/// duplicating it as a payload would create two ways to spell the same
/// fact and force the adapter to reconcile them.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatProvider {
    /// Anthropic (Claude). v1 default — preserved so workflows that
    /// omit `provider:` keep working through the adapter.
    #[default]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    Ollama,
    Groq,
    #[serde(rename = "deepseek")]
    DeepSeek,
    Gemini,
    Cohere,
    Perplexity,
    Xai,
    Mistral,
    /// OpenAI-compatible endpoint (self-hosted gateway, vLLM, LM Studio,
    /// Together AI, Azure OpenAI, etc.). The adapter requires
    /// [`Step::base_url`] to be set when this variant is selected.
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
}

impl ChatProvider {
    /// Per-provider default model name applied by the adapter when the
    /// workflow omits `config.model`. Mirrors the v1 fallback match at
    /// `src/steps/chat.rs:341-348` (PR 5b of Task #31). Variants v1 didn't
    /// enumerate (Anthropic, Cohere, Perplexity, Xai, Mistral,
    /// OpenAiCompatible) inherit v1's catch-all
    /// `"claude-3-haiku-20240307"` — a known v1 quirk that the adapter
    /// preserves so workflows that relied on the fallback keep running.
    pub fn default_model(&self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-4o-mini",
            Self::Ollama => "llama3.2",
            Self::Groq => "llama-3.3-70b-versatile",
            Self::DeepSeek => "deepseek-chat",
            Self::Gemini => "gemini-2.0-flash",
            Self::Anthropic
            | Self::Cohere
            | Self::Perplexity
            | Self::Xai
            | Self::Mistral
            | Self::OpenAiCompatible => "claude-3-haiku-20240307",
        }
    }

    /// Per-provider default environment variable name the runtime reads
    /// for the provider's API key when the workflow omits
    /// `config.api_key_env`. Mirrors the v1 match at
    /// `src/steps/chat.rs:363-373`. `None` for [`Self::Ollama`] — v1
    /// skipped the key lookup entirely for Ollama
    /// (`src/steps/chat.rs:359-362`) because local Ollama endpoints don't
    /// authenticate. [`Self::OpenAiCompatible`] returns
    /// `Some("ANTHROPIC_API_KEY")` to preserve v1 parity (v1's catch-all
    /// `_ =>`); operators targeting a non-Anthropic gateway are expected
    /// to set `config.api_key_env` explicitly.
    pub fn default_api_key_env(&self) -> Option<&'static str> {
        match self {
            Self::Ollama => None,
            Self::OpenAi => Some("OPENAI_API_KEY"),
            Self::Groq => Some("GROQ_API_KEY"),
            Self::DeepSeek => Some("DEEPSEEK_API_KEY"),
            Self::Gemini => Some("GEMINI_API_KEY"),
            Self::Cohere => Some("COHERE_API_KEY"),
            Self::Perplexity => Some("PERPLEXITY_API_KEY"),
            Self::Xai => Some("XAI_API_KEY"),
            Self::Mistral => Some("MISTRAL_API_KEY"),
            Self::Anthropic | Self::OpenAiCompatible => Some("ANTHROPIC_API_KEY"),
        }
    }
}

/// History truncation strategy for [`StepKind::Chat`] steps (PR 5b of
/// Task #31). Mirrors v1's `truncation:` config shape. Absent at the
/// [`Step`] level (field is `None`) means no truncation — the runtime
/// sends the full conversation. A `"none"` strategy on the wire is
/// normalized to `None` at the adapter boundary.
///
/// Serde tag is `strategy` so the YAML reads naturally:
///
/// ```yaml
/// truncation:
///   strategy: sliding_window
///   max_tokens: 8000
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum ChatTruncation {
    /// Keep the `count` most recent messages; drop everything older.
    Last { count: u64 },
    /// Keep the `count` oldest messages; drop everything newer. Rare
    /// in production — carried for v1 parity.
    First { count: u64 },
    /// Keep `first` oldest plus `last` newest messages; drop the middle.
    FirstLast { first: u64, last: u64 },
    /// Drop oldest messages until the conversation fits within
    /// `max_tokens` of input budget (heuristic, not a hard cap —
    /// exact accounting happens provider-side).
    SlidingWindow { max_tokens: u64 },
}

/// A complete workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub steps: Vec<Step>,
    /// Workflow-level env vars. Merged below step-level env and above
    /// `.stepyard/defaults.yaml` in the cascade resolver (Story 3.4).
    /// `#[serde(default)]` preserves backward compat for YAML without an
    /// `env:` field (NFR18, NFR22).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Named step groups referenced by repeat/map/call/parallel kinds.
    /// `#[serde(default)]` keeps existing cmd-only YAML without a
    /// `scopes:` block parseable.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scopes: HashMap<String, Scope>,
    /// Directory where `template` steps look up `<name>.md.tera` files.
    /// Interpreted relative to the harness process's working directory
    /// when it's not absolute. Absent = fall back to `"prompts"` (v1
    /// parity with `src/engine/context.rs:58`). PR 4 of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts_dir: Option<String>,
    /// Workflow-wide git branch strategy for workspace preparation.
    #[serde(default, skip_serializing_if = "BranchStrategyYaml::is_head")]
    pub branch_strategy: BranchStrategyYaml,
    /// Required when `branch_strategy: named_branch`. A literal
    /// `{{BRANCH_NAME}}` placeholder is intentionally preserved for Epic 5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    /// Base branch for `branch_strategy: merge_to_head`; defaults to `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
}

impl Workflow {
    pub fn new(name: impl Into<String>, steps: Vec<Step>) -> Self {
        Self {
            name: name.into(),
            steps,
            env: HashMap::new(),
            scopes: HashMap::new(),
            prompts_dir: None,
            branch_strategy: BranchStrategyYaml::Head,
            branch_name: None,
            base_branch: None,
        }
    }

    /// Parse workflow YAML into a [`Workflow`], turning per-field
    /// deserialization failures into [`EngineError::InvalidWorkflowField`]
    /// with the serde path (`steps[0].timeout`, `scopes.work.steps[2].timeout`)
    /// pinned via [`serde_path_to_error`]. Document-level failures
    /// (malformed YAML, missing required keys) collapse to
    /// [`EngineError::InvalidWorkflow`], matching Round 3 architecture.md
    /// §D9 rule text.
    ///
    /// The adapter (`src/cli/harness_adapter.rs`) still owns v1 YAML
    /// shape translation; this entry point is for harness callers that
    /// already have a v2-shaped workflow document and want the strict,
    /// field-path-aware error surface.
    pub fn try_from_yaml(yaml: &str) -> Result<Self, EngineError> {
        let de = serde_yaml::Deserializer::from_str(yaml);
        match serde_path_to_error::deserialize::<_, Workflow>(de) {
            Ok(wf) => {
                wf.validate()?;
                Ok(wf)
            }
            Err(err) => Err(map_path_error(err)),
        }
    }

    /// Boundary check that every env entry — workflow-level, per-step,
    /// and per-scope-step — satisfies the Round 3 Story 3 grammars
    /// pinned in [`stepyard_core::env`]. Returns the first offender as
    /// [`EngineError::InvalidWorkflowField`] with a serde-style path
    /// pointing at the bad entry, or `Ok(())` when every entry passes.
    ///
    /// Called automatically by [`Workflow::try_from_yaml`]. Programmatic
    /// callers (e.g. the CLI's v1→v2 adapter) MUST invoke this before
    /// handing the workflow to [`crate::Engine`] — `Workflow::new` and
    /// the `pub` field setters do NOT auto-validate so tests can build
    /// invalid workflows for negative-path coverage.
    pub fn validate(&self) -> Result<(), EngineError> {
        self.resolve_branch_strategy()?;
        core_env::validate_env_map("env", &self.env)?;
        for (idx, step) in self.steps.iter().enumerate() {
            core_env::validate_env_map(&format!("steps[{idx}].env"), &step.env)?;
        }
        let mut scope_names: Vec<&String> = self.scopes.keys().collect();
        scope_names.sort();
        for name in scope_names {
            let scope = &self.scopes[name];
            for (idx, step) in scope.steps.iter().enumerate() {
                core_env::validate_env_map(&format!("scopes.{name}.steps[{idx}].env"), &step.env)?;
            }
        }
        Ok(())
    }

    pub fn resolve_branch_strategy(&self) -> Result<BranchStrategy, EngineError> {
        match self.branch_strategy {
            BranchStrategyYaml::Head => Ok(BranchStrategy::Head),
            BranchStrategyYaml::MergeToHead => Ok(BranchStrategy::MergeToHead {
                target: self
                    .base_branch
                    .clone()
                    .unwrap_or_else(|| "main".to_string()),
            }),
            BranchStrategyYaml::NamedBranch => {
                let Some(name) = self.branch_name.as_ref().filter(|s| !s.is_empty()) else {
                    return Err(EngineError::PlaceholderUnresolved {
                        key: "branch_name".to_string(),
                        found_at: "<workflow-file>".to_string(),
                    });
                };
                Ok(BranchStrategy::NamedBranch { name: name.clone() })
            }
        }
    }

    pub fn set_branch_strategy(&mut self, strategy: BranchStrategy) {
        match strategy {
            BranchStrategy::Head => {
                self.branch_strategy = BranchStrategyYaml::Head;
                self.branch_name = None;
                self.base_branch = None;
            }
            BranchStrategy::MergeToHead { target } => {
                self.branch_strategy = BranchStrategyYaml::MergeToHead;
                self.branch_name = None;
                self.base_branch = Some(target);
            }
            BranchStrategy::NamedBranch { name } => {
                self.branch_strategy = BranchStrategyYaml::NamedBranch;
                self.branch_name = Some(name);
                self.base_branch = None;
            }
            _ => {}
        }
    }
}

/// Classify a `serde_path_to_error` failure into the right
/// [`EngineError`] variant. When the offending path points at a known
/// `Duration`-shaped field (`timeout`) we synthesise
/// [`EngineError::InvalidWorkflowField`] with the canonical grammar
/// hint from [`core_duration::EXPECTED`]; everything else stays on
/// [`EngineError::InvalidWorkflow`] since we can't reliably extract
/// a stable per-field `expected` phrase for arbitrary types.
fn map_path_error(err: serde_path_to_error::Error<serde_yaml::Error>) -> EngineError {
    let path = err.path().to_string();
    let inner = err.into_inner();
    if path_points_at_duration_field(&path) {
        EngineError::InvalidWorkflowField {
            path,
            got: extract_got(&inner),
            expected: core_duration::EXPECTED,
        }
    } else {
        EngineError::InvalidWorkflow(format!("{path}: {inner}"))
    }
}

/// True when the serde path ends in a segment we know is a
/// `Option<Duration>` field on [`Step`]. Kept as a substring match
/// (not an exact tail check) so nested shapes like
/// `scopes.work.steps[0].timeout` still route through
/// [`EngineError::InvalidWorkflowField`] without listing every
/// possible parent prefix.
fn path_points_at_duration_field(path: &str) -> bool {
    path.ends_with(".timeout") || path == "timeout"
}

/// Pull the offending raw value out of a serde_yaml error message.
/// serde_yaml renders "invalid type: integer `30`, expected …" (or
/// `string \"zzz\"`, `floating point \`1.5\``). We peel off the quoted
/// token so [`EngineError::InvalidWorkflowField`]'s `got` field
/// matches the operator's YAML without the surrounding noise. If
/// parsing fails we fall back to the full message — the operator
/// still sees the error, just with a less precise `got` slot.
fn extract_got(err: &serde_yaml::Error) -> String {
    let msg = err.to_string();
    if let Some(start) = msg.find('`') {
        if let Some(end) = msg[start + 1..].find('`') {
            return msg[start + 1..start + 1 + end].to_string();
        }
    }
    msg
}

/// A named sub-sequence of steps, referenced by repeat/map/call kinds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    pub steps: Vec<Step>,
    /// Tera expression evaluated at scope completion to produce the
    /// container step's output snapshot (PR 3 of Task #31). Absent = the
    /// container falls back to the last step's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<String>,
}

/// One step in a workflow. Executable kinds are dispatched by
/// `crates/stepyard-harness/src/engine.rs`; placement rules such as
/// "containers cannot nest inside scope bodies" are enforced by the CLI
/// adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    /// Step kind. Controls which executor the engine dispatches to. YAML
    /// field name is `type` to match the legacy workflow schema. Absent
    /// `type:` defaults to [`StepKind::Cmd`] so existing cmd-only YAML
    /// keeps deserializing unchanged.
    #[serde(default, rename = "type", skip_serializing_if = "StepKind::is_cmd")]
    pub kind: StepKind,
    /// Shell command for [`StepKind::Cmd`] steps and Rhai source for
    /// [`StepKind::Script`] steps — both map to the same legacy YAML field
    /// (`run:`). Empty for other kinds.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Wall-clock step timeout. Absent = no timeout. YAML field name is
    /// `timeout` to match the Story 1.4 workflow schema. Round 3 Story 1
    /// tightened the wire type: only duration strings (`30s`, `500ms`,
    /// `1h30m`) are accepted — bare integers are rejected before
    /// execution so `timeout: 30` can no longer silently mean "30 ms"
    /// in one code path and "30 s" in another.
    #[serde(
        rename = "timeout",
        default,
        serialize_with = "core_duration::serialize_optional",
        deserialize_with = "core_duration::deserialize_optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub timeout: Option<Duration>,
    /// Step-level env vars. Highest precedence in the cascade resolver
    /// (Story 3.4) — overrides workflow, defaults, and host `${VAR}`.
    /// `#[serde(default)]` keeps existing YAML without `env:` parseable
    /// (NFR18, NFR22).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Tera template evaluated by [`StepKind::Gate`] steps to decide the
    /// pass/fail branch. PR 2 of Task #31. Other kinds leave this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// Gate action when `condition` evaluates truthy. PR 2 only accepts
    /// `"continue"` and `"fail"`; `"break"` and `"skip"` land with
    /// repeat/map/call semantics in PR 3 and are rejected by the adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_pass: Option<String>,
    /// Gate action when `condition` evaluates falsy. Same value space as
    /// `on_pass`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_fail: Option<String>,
    /// Operator-visible message attached to the gate's terminal outcome
    /// (displayed in Dashboard and CLI). Plain string, no templating in
    /// PR 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Named scope referenced by `call` / `repeat` / `map` (PR 3 of Task
    /// #31). Required at the adapter boundary for those kinds; `None` for
    /// every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Safety cap for `repeat`: once the counter reaches this value the
    /// container completes cleanly with a warning (v1 parity, see
    /// v1 `repeat.rs:143`). `None` = no explicit cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
    /// Seed value exposed as `scope.value` for the first iteration/pass
    /// of `call` / `repeat` / `map` (v1 parity). Persisted as JSON so
    /// the harness stays YAML-agnostic — the adapter converts the YAML
    /// value once at parse time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_value: Option<serde_json::Value>,
    /// Tera expression rendered by `map` to produce the iteration
    /// collection. Required at the adapter boundary for `map`; `None`
    /// for every other kind. Shape of the rendered result is *not*
    /// enforced at adapt time — the scope runner preserves v1's
    /// "JSON array or line-split" heuristic (v1 `map.rs:237`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    /// Parallelism budget for `map`. Carried through for legacy-YAML
    /// round-trip fidelity; the scope runner in PR 3 executes
    /// sequentially even when this is `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<usize>,
    /// Override for the containing [`Scope::outputs`] template. `None`
    /// falls back to the scope-level value (if any) or to the last
    /// scope-body step's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<String>,
    /// Tri-use prompt field. [`StepKind::Template`]: Tera expression
    /// rendered once to produce the prompt-file basename (absent → fall
    /// back to `step.name`, two-pass render matches v1
    /// `src/steps/template_step.rs:32-36`, PR 4 of Task #31).
    /// [`StepKind::Agent`]: inline prompt piped to the Claude CLI's
    /// stdin (v1 parity with `StepDef.prompt` at
    /// `src/workflow/schema.rs:106`, required by the adapter for agent
    /// steps, PR 5a of Task #31).
    /// [`StepKind::Chat`]: user message appended to the chat session's
    /// history and sent to the provider (v1 parity with `config.prompt`
    /// in the chat bag, required by the adapter for chat steps,
    /// PR 5b of Task #31).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Model override shared by [`StepKind::Agent`] and [`StepKind::Chat`].
    /// For agent steps, threaded to `--model <value>` on the Claude CLI
    /// argv (v1 parity with `config.get_str("model")` at
    /// `src/steps/agent.rs:28-30`, PR 5a of Task #31). For chat steps,
    /// forwarded to the selected rig-core provider's model parameter —
    /// absent = per-provider default resolved at the adapter boundary
    /// (PR 5b of Task #31). Left `None` for every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// [`StepKind::Agent`] only: appended to the system prompt via
    /// `--append-system-prompt <value>`. v1 parity with
    /// `src/steps/agent.rs:31-33`. Absent = no append. PR 5a of #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_append: Option<String>,
    /// [`StepKind::Agent`] only: permission posture for the child CLI.
    /// `None` or `Some(Default)` = no extra flag; `Some(Skip)` appends
    /// `--dangerously-skip-permissions`. v1 parity with
    /// `config.get_str("permissions") == Some("skip")` at
    /// `src/steps/agent.rs:34-36`. PR 5a of #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<AgentPermissions>,
    /// [`StepKind::Agent`] only: name of a prior agent step whose
    /// captured `agent_session_id` the executor resolves (from the
    /// session log) and passes via `--resume <id>`. v1 parity with
    /// `config.get_str("resume")` + `lookup_session_id` at
    /// `src/steps/agent.rs:39-43`. Mutually exclusive with
    /// `fork_session` at the adapter boundary. PR 5a of #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// [`StepKind::Agent`] only: name of a prior agent step whose
    /// `agent_session_id` the executor uses to fork a new session
    /// (`--fork-session --resume <id>`). v1 parity with
    /// `config.get_str("fork_session")` at `src/steps/agent.rs:46-50`
    /// (v1 used the bare `--resume`; v2 emits `--fork-session` too so
    /// the argv reflects the semantic intent). Mutually exclusive with
    /// `resume` at the adapter boundary. PR 5a of #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_session: Option<String>,
    /// [`StepKind::Agent`] only: workflow-level session mode applied
    /// when no explicit `resume:` / `fork_session:` is set. `None` =
    /// [`AgentSessionMode::Shared`] (v1 default). v1 parity with
    /// `config.get_str("session") == Some("isolated")` at
    /// `src/steps/agent.rs:55`. PR 5a of #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<AgentSessionMode>,
    /// [`StepKind::Agent`] only: path (or PATH-resolvable name) of the
    /// Claude CLI binary the executor spawns. `None` = fall back to
    /// `"claude"`. v1 parity with `config.get_str("command")` at
    /// `src/steps/agent.rs:24`; this keeps workflows that point at a
    /// wrapper, path-pinned binary, corporate script, or mock CLI
    /// (integration tests) working unchanged on v2.
    ///
    /// Deliberately named `agent_command` rather than reusing
    /// [`Self::command`] — the latter is the shell source for
    /// [`StepKind::Cmd`] and the Rhai source for [`StepKind::Script`];
    /// reusing it for the agent binary path would conflate two
    /// unrelated semantic axes and make YAML serialization ambiguous.
    /// PR 5a of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_command: Option<String>,

    /// [`StepKind::Chat`] only: provider selector routed to rig-core.
    /// `None` = fall back to [`ChatProvider::Anthropic`] at the adapter
    /// boundary (v1 parity — omitting `provider:` defaulted to
    /// Anthropic). Unknown string values fail adapt-time rather than
    /// silently defaulting. PR 5b of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_provider: Option<ChatProvider>,
    /// [`StepKind::Chat`] only: response-length cap threaded to the
    /// provider as its `max_tokens` (OpenAI/Anthropic semantics). v1
    /// parity with `config.get_int("max_tokens")`. Absent = provider
    /// default. PR 5b of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// [`StepKind::Chat`] only: sampling temperature forwarded to the
    /// provider. v1 parity with `config.get_float("temperature")`.
    /// Absent = provider default. PR 5b of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// [`StepKind::Chat`] only: environment variable name whose value is
    /// read at spawn time to authenticate against the provider (e.g.
    /// `"OPENAI_API_KEY"`). v1 parity with `config.get_str("api_key_env")`.
    /// Absent = fall back to the per-provider default env var at the
    /// adapter boundary. Never persisted to the session log. PR 5b of
    /// Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// [`StepKind::Chat`] only: provider endpoint override. Required
    /// when [`Self::chat_provider`] is [`ChatProvider::OpenAiCompatible`]
    /// (adapter boundary); optional for other providers that support
    /// endpoint overrides (e.g. Ollama's host URL). v1 parity with
    /// `config.get_str("base_url")`. PR 5b of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// [`StepKind::Chat`] only: logical chat session name that scopes
    /// the conversation history. Two chat steps with the same
    /// `chat_session` share history; distinct names isolate. Absent =
    /// stateless single-turn (v1 parity with `config.get_str("session")`
    /// absent). The runtime replays
    /// [`stepyard_core::Event::ChatMessageAppended`] entries from the
    /// session log rather than keeping per-session state in memory, so
    /// history survives a process crash. PR 5b of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_session: Option<String>,
    /// [`StepKind::Chat`] only: history truncation strategy applied
    /// before dispatching to the provider. `None` = send full history
    /// (v1 parity with `config.get("truncation")` absent). A v1
    /// `strategy: none` value is normalized to `None` at the adapter
    /// boundary so this enum never needs a placeholder variant.
    /// PR 5b of Task #31.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<ChatTruncation>,
}

impl Step {
    pub fn cmd(name: impl Into<String>, command: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Cmd);
        step.command = command.into();
        step
    }

    /// Constructor for a [`StepKind::Gate`] step. PR 2 of Task #31.
    pub fn gate(name: impl Into<String>, condition: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Gate);
        step.condition = Some(condition.into());
        step
    }

    /// Constructor for a [`StepKind::Call`] step — runs `scope` once.
    /// PR 3 of Task #31.
    pub fn call(name: impl Into<String>, scope: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Call);
        step.scope = Some(scope.into());
        step
    }

    /// Constructor for a [`StepKind::Repeat`] step — runs `scope` until a
    /// scope-body gate emits `break`, or `max_iterations` fires (v1
    /// parity with a completion warning).
    pub fn repeat(name: impl Into<String>, scope: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Repeat);
        step.scope = Some(scope.into());
        step
    }

    /// Constructor for a [`StepKind::Map`] step — runs `scope` once per
    /// item rendered from the `items` Tera expression.
    pub fn map(
        name: impl Into<String>,
        scope: impl Into<String>,
        items: impl Into<String>,
    ) -> Self {
        let mut step = Step::empty(name, StepKind::Map);
        step.scope = Some(scope.into());
        step.items = Some(items.into());
        step
    }

    /// Constructor for a [`StepKind::Parallel`] step — runs every step in
    /// `scope` concurrently and returns when all sub-steps finish (or the
    /// first sub-step error aborts the rest, v1 parity). The adapter
    /// synthesises a hidden scope from v1's inline `steps:` list (PR 6 of
    /// Task #31), so this constructor takes the same `scope:` reference
    /// shape as [`Self::call`] / [`Self::repeat`] / [`Self::map`] and
    /// keeps the v2 data model uniform across container kinds.
    pub fn parallel(name: impl Into<String>, scope: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Parallel);
        step.scope = Some(scope.into());
        step
    }

    /// Constructor for a [`StepKind::Template`] step — renders the
    /// `{prompts_dir}/{prompt or name}.md.tera` file against the current
    /// render context. PR 4 of Task #31.
    pub fn template(name: impl Into<String>, prompt: Option<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Template);
        step.prompt = prompt;
        step
    }

    /// Constructor for a [`StepKind::Script`] step — evaluates the Rhai
    /// `source` against a flat snapshot of the harness render context and
    /// emits the return value as `stdout`. PR 4 of Task #31.
    pub fn script(name: impl Into<String>, source: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Script);
        step.command = source.into();
        step
    }

    /// Constructor for a [`StepKind::Agent`] step — invokes the Claude CLI
    /// with `prompt` piped over stdin. Additional knobs (`model`,
    /// `system_prompt_append`, `permissions`, `resume`, `fork_session`,
    /// `agent_session`) land via direct field assignment on the returned
    /// `Step`. PR 5a of Task #31.
    pub fn agent(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Agent);
        step.prompt = Some(prompt.into());
        step
    }

    /// Constructor for a [`StepKind::Chat`] step — sends `prompt` to the
    /// configured rig-core provider. Additional knobs (`chat_provider`,
    /// `max_tokens`, `temperature`, `api_key_env`, `base_url`,
    /// `chat_session`, `truncation`) land via direct field assignment on
    /// the returned `Step`. PR 5b of Task #31.
    pub fn chat(name: impl Into<String>, prompt: impl Into<String>) -> Self {
        let mut step = Step::empty(name, StepKind::Chat);
        step.prompt = Some(prompt.into());
        step
    }

    fn empty(name: impl Into<String>, kind: StepKind) -> Self {
        Self {
            name: name.into(),
            kind,
            command: String::new(),
            timeout: None,
            env: HashMap::new(),
            condition: None,
            on_pass: None,
            on_fail: None,
            message: None,
            scope: None,
            max_iterations: None,
            initial_value: None,
            items: None,
            parallel: None,
            outputs: None,
            prompt: None,
            model: None,
            system_prompt_append: None,
            permissions: None,
            resume: None,
            fork_session: None,
            agent_session: None,
            agent_command: None,
            chat_provider: None,
            max_tokens: None,
            temperature: None,
            api_key_env: None,
            base_url: None,
            chat_session: None,
            truncation: None,
        }
    }

    /// Builder variant that attaches a wall-clock timeout. Accepts a
    /// [`Duration`] directly so callers don't thread a millisecond
    /// integer through the constructor — the wire format accepts only
    /// duration strings after Round 3 Story 1.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Builder variant that sets step-level env vars (used by Story 3.4's
    /// cascade resolver).
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backward_compat_cmd_only_yaml_still_parses() {
        let yaml = r#"
name: legacy
steps:
  - name: one
    command: "echo 1"
  - name: two
    command: "echo 2"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.name, "legacy");
        assert_eq!(wf.steps.len(), 2);
        assert_eq!(wf.steps[0].kind, StepKind::Cmd);
        assert!(wf.scopes.is_empty());
    }

    #[test]
    fn non_cmd_step_kind_deserializes() {
        let yaml = r#"
name: scaffold
steps:
  - name: check
    type: gate
  - name: loop
    type: repeat
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.steps[0].kind, StepKind::Gate);
        assert_eq!(wf.steps[1].kind, StepKind::Repeat);
    }

    #[test]
    fn scopes_block_deserializes() {
        let yaml = r#"
name: scaffold-scopes
steps:
  - name: enter
    command: "echo enter"
scopes:
  work:
    steps:
      - name: inner
        command: "echo inner"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        let scope = wf.scopes.get("work").expect("scope `work` missing");
        assert_eq!(scope.steps.len(), 1);
        assert_eq!(scope.steps[0].name, "inner");
        assert_eq!(scope.steps[0].kind, StepKind::Cmd);
    }

    #[test]
    fn branch_strategy_defaults_to_head() {
        let yaml = r#"
name: default-branch
steps:
  - name: one
    command: "echo one"
"#;
        let wf = Workflow::try_from_yaml(yaml).unwrap();
        assert_eq!(wf.branch_strategy, BranchStrategyYaml::Head);
        assert!(matches!(
            wf.resolve_branch_strategy().unwrap(),
            BranchStrategy::Head
        ));
    }

    #[test]
    fn branch_strategy_yaml_fixtures_parse() {
        let head = Workflow::try_from_yaml(
            r#"
name: head
branch_strategy: head
steps:
  - name: one
    command: "echo one"
"#,
        )
        .unwrap();
        assert_eq!(head.branch_strategy, BranchStrategyYaml::Head);

        let merge = Workflow::try_from_yaml(
            r#"
name: merge
branch_strategy: merge_to_head
base_branch: release
steps:
  - name: one
    command: "echo one"
"#,
        )
        .unwrap();
        assert!(matches!(
            merge.resolve_branch_strategy().unwrap(),
            BranchStrategy::MergeToHead { ref target } if target == "release"
        ));

        let named = Workflow::try_from_yaml(
            r#"
name: named
branch_strategy: named_branch
branch_name: feat/test
steps:
  - name: one
    command: "echo one"
"#,
        )
        .unwrap();
        assert!(matches!(
            named.resolve_branch_strategy().unwrap(),
            BranchStrategy::NamedBranch { ref name } if name == "feat/test"
        ));
    }

    #[test]
    fn merge_to_head_defaults_base_branch_to_main() {
        let yaml = r#"
name: merge-default
branch_strategy: merge_to_head
steps:
  - name: one
    command: "echo one"
"#;
        let wf = Workflow::try_from_yaml(yaml).unwrap();
        assert!(matches!(
            wf.resolve_branch_strategy().unwrap(),
            BranchStrategy::MergeToHead { ref target } if target == "main"
        ));
    }

    #[test]
    fn named_branch_without_branch_name_fails_fast() {
        let yaml = r#"
name: missing-branch-name
branch_strategy: named_branch
steps:
  - name: one
    command: "echo one"
"#;
        let err = Workflow::try_from_yaml(yaml).unwrap_err();
        assert!(matches!(
            err,
            EngineError::PlaceholderUnresolved {
                ref key,
                ref found_at
            } if key == "branch_name" && found_at == "<workflow-file>"
        ));
    }

    #[test]
    fn named_branch_placeholder_is_preserved_literal() {
        let yaml = r#"
name: placeholder-branch
branch_strategy: named_branch
branch_name: "{{BRANCH_NAME}}"
steps:
  - name: one
    command: "echo one"
"#;
        let wf = Workflow::try_from_yaml(yaml).unwrap();
        assert!(matches!(
            wf.resolve_branch_strategy().unwrap(),
            BranchStrategy::NamedBranch { ref name } if name == "{{BRANCH_NAME}}"
        ));
    }

    #[test]
    fn cmd_kind_omitted_from_serialized_output() {
        let step = Step::cmd("s", "true");
        let yaml = serde_yaml::to_string(&step).unwrap();
        assert!(
            !yaml.contains("type:"),
            "cmd is the default; `type:` should be skipped: {yaml}"
        );
    }

    #[test]
    fn cmd_step_omits_gate_fields_from_serialized_output() {
        // Backward-compat: the four gate fields added in PR 2 of Task #31
        // must stay absent on the wire for cmd steps, otherwise existing
        // workflow round-trips would gain noise.
        let step = Step::cmd("s", "true");
        let yaml = serde_yaml::to_string(&step).unwrap();
        for field in ["condition:", "on_pass:", "on_fail:", "message:"] {
            assert!(
                !yaml.contains(field),
                "cmd step leaked `{field}` onto the wire: {yaml}"
            );
        }
    }

    #[test]
    fn gate_fields_roundtrip_through_yaml() {
        let yaml = r#"
name: with-gate
steps:
  - name: check
    type: gate
    condition: "{{ steps.one.exit_code }} == 0"
    on_pass: continue
    on_fail: fail
    message: "build must pass"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.kind, StepKind::Gate);
        assert_eq!(
            step.condition.as_deref(),
            Some("{{ steps.one.exit_code }} == 0")
        );
        assert_eq!(step.on_pass.as_deref(), Some("continue"));
        assert_eq!(step.on_fail.as_deref(), Some("fail"));
        assert_eq!(step.message.as_deref(), Some("build must pass"));
    }

    #[test]
    fn container_fields_roundtrip_through_yaml() {
        let yaml = r#"
name: containers
steps:
  - name: once
    type: call
    scope: setup
  - name: loop
    type: repeat
    scope: work
    max_iterations: 5
    initial_value: 0
  - name: fan
    type: map
    scope: per_file
    items: "{{ steps.list.stdout }}"
    parallel: 4
    initial_value:
      seed: ok
      tries: 3
scopes:
  setup:
    steps:
      - name: seed
        command: "echo seed"
    outputs: "{{ steps.seed.stdout }}"
  work:
    steps:
      - name: tick
        command: "echo tick"
  per_file:
    steps:
      - name: touch
        command: "echo {{ item }}"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();

        let call = &wf.steps[0];
        assert_eq!(call.kind, StepKind::Call);
        assert_eq!(call.scope.as_deref(), Some("setup"));

        let rep = &wf.steps[1];
        assert_eq!(rep.kind, StepKind::Repeat);
        assert_eq!(rep.max_iterations, Some(5));
        assert_eq!(rep.initial_value, Some(serde_json::json!(0)));

        let map = &wf.steps[2];
        assert_eq!(map.kind, StepKind::Map);
        assert_eq!(map.items.as_deref(), Some("{{ steps.list.stdout }}"));
        assert_eq!(map.parallel, Some(4));
        assert_eq!(
            map.initial_value,
            Some(serde_json::json!({ "seed": "ok", "tries": 3 }))
        );

        let setup = wf.scopes.get("setup").unwrap();
        assert_eq!(setup.outputs.as_deref(), Some("{{ steps.seed.stdout }}"));
        assert!(wf.scopes.get("work").unwrap().outputs.is_none());
    }

    #[test]
    fn cmd_step_omits_container_fields_from_serialized_output() {
        // Backward-compat: the six container fields added in PR 3 of #31
        // must stay absent on the wire for cmd and gate steps, otherwise
        // existing workflow round-trips would gain noise.
        let step = Step::cmd("s", "true");
        let yaml = serde_yaml::to_string(&step).unwrap();
        for field in [
            "scope:",
            "max_iterations:",
            "initial_value:",
            "items:",
            "parallel:",
            "outputs:",
        ] {
            assert!(
                !yaml.contains(field),
                "cmd step leaked `{field}` onto the wire: {yaml}"
            );
        }
    }

    #[test]
    fn container_constructors_roundtrip_through_serde() {
        let call = Step::call("once", "setup");
        let rep = Step::repeat("loop", "work");
        let map = Step::map("fan", "per_file", "{{ steps.list.stdout }}");

        let wf = Workflow::new("ctor", vec![call, rep, map]);
        let yaml = serde_yaml::to_string(&wf).unwrap();
        let back: Workflow = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(back.steps[0].kind, StepKind::Call);
        assert_eq!(back.steps[0].scope.as_deref(), Some("setup"));
        assert_eq!(back.steps[1].kind, StepKind::Repeat);
        assert_eq!(back.steps[1].scope.as_deref(), Some("work"));
        assert_eq!(back.steps[2].kind, StepKind::Map);
        assert_eq!(
            back.steps[2].items.as_deref(),
            Some("{{ steps.list.stdout }}")
        );
    }

    #[test]
    fn agent_fields_roundtrip_through_yaml() {
        let yaml = r#"
name: with-agent
steps:
  - name: plan
    type: agent
    prompt: "Summarize {{ target }}"
    model: claude-sonnet-4-6
    system_prompt_append: "Be concise."
    permissions: skip
    agent_session: isolated
  - name: refine
    type: agent
    prompt: "Continue from plan"
    resume: plan
  - name: branch
    type: agent
    prompt: "Try alternative"
    fork_session: plan
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.steps.len(), 3);

        let plan = &wf.steps[0];
        assert_eq!(plan.kind, StepKind::Agent);
        assert_eq!(plan.prompt.as_deref(), Some("Summarize {{ target }}"));
        assert_eq!(plan.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(plan.system_prompt_append.as_deref(), Some("Be concise."));
        assert_eq!(plan.permissions, Some(AgentPermissions::Skip));
        assert_eq!(plan.agent_session, Some(AgentSessionMode::Isolated));
        assert!(plan.resume.is_none());
        assert!(plan.fork_session.is_none());

        let refine = &wf.steps[1];
        assert_eq!(refine.resume.as_deref(), Some("plan"));
        assert!(refine.fork_session.is_none());

        let branch = &wf.steps[2];
        assert!(branch.resume.is_none());
        assert_eq!(branch.fork_session.as_deref(), Some("plan"));
    }

    #[test]
    fn agent_permissions_default_variant_roundtrips() {
        let yaml = r#"
name: agent-default-perms
steps:
  - name: ask
    type: agent
    prompt: "Hi"
    permissions: default
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.steps[0].permissions, Some(AgentPermissions::Default));
    }

    #[test]
    fn cmd_step_omits_agent_fields_from_serialized_output() {
        // Backward-compat mirror of the earlier gate/container guards:
        // cmd steps must not leak any of the six agent fields added in
        // PR 5a of #31 onto the wire, otherwise existing workflow
        // round-trips would gain noise.
        let step = Step::cmd("s", "true");
        let yaml = serde_yaml::to_string(&step).unwrap();
        for field in [
            "model:",
            "system_prompt_append:",
            "permissions:",
            "resume:",
            "fork_session:",
            "agent_session:",
            "agent_command:",
        ] {
            assert!(
                !yaml.contains(field),
                "cmd step leaked `{field}` onto the wire: {yaml}"
            );
        }
    }

    #[test]
    fn agent_command_roundtrips_through_yaml() {
        // v1 parity: `config.command` in the legacy agent bag carries the
        // CLI binary path (wrapper, mock, path-pinned claude). v2 promotes
        // it to a typed top-level field on agent steps — this guards the
        // serde roundtrip in both directions.
        let yaml = r#"
name: with-agent-command
steps:
  - name: ask
    type: agent
    prompt: "Hi"
    agent_command: "/usr/local/bin/claude"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            wf.steps[0].agent_command.as_deref(),
            Some("/usr/local/bin/claude")
        );

        let back = serde_yaml::to_string(&wf).unwrap();
        assert!(
            back.contains("agent_command: /usr/local/bin/claude"),
            "agent_command must be serialized back: {back}"
        );
    }

    #[test]
    fn agent_command_absent_by_default_on_agent_constructor() {
        // Defensive: the `Step::agent` constructor must not coerce a
        // default binary string — absent-equals-fall-back is what the
        // executor enforces at spawn time.
        let step = Step::agent("ask", "Hi");
        assert!(step.agent_command.is_none());
    }

    #[test]
    fn agent_constructor_roundtrips_through_serde() {
        let mut step = Step::agent("plan", "Summarize {{ target }}");
        step.model = Some("claude-sonnet-4-6".into());
        step.resume = Some("prior".into());
        step.agent_session = Some(AgentSessionMode::Isolated);

        let wf = Workflow::new("ctor", vec![step]);
        let yaml = serde_yaml::to_string(&wf).unwrap();
        let back: Workflow = serde_yaml::from_str(&yaml).unwrap();

        let s = &back.steps[0];
        assert_eq!(s.kind, StepKind::Agent);
        assert_eq!(s.prompt.as_deref(), Some("Summarize {{ target }}"));
        assert_eq!(s.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(s.resume.as_deref(), Some("prior"));
        assert_eq!(s.agent_session, Some(AgentSessionMode::Isolated));
    }

    #[test]
    fn agent_session_mode_default_is_shared() {
        assert_eq!(AgentSessionMode::default(), AgentSessionMode::Shared);
    }

    #[test]
    fn agent_permissions_default_is_default_variant() {
        assert_eq!(AgentPermissions::default(), AgentPermissions::Default);
    }

    #[test]
    fn chat_fields_roundtrip_through_yaml() {
        let yaml = r#"
name: with-chat
steps:
  - name: ask
    type: chat
    prompt: "Summarize {{ target }}"
    chat_provider: openai
    model: gpt-4o-mini
    max_tokens: 1024
    temperature: 0.5
    api_key_env: MY_OPENAI_KEY
    base_url: "https://api.openai.com/v1"
    chat_session: assistant
    truncation:
      strategy: sliding_window
      max_tokens: 8000
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.steps.len(), 1);

        let ask = &wf.steps[0];
        assert_eq!(ask.kind, StepKind::Chat);
        assert_eq!(ask.prompt.as_deref(), Some("Summarize {{ target }}"));
        assert_eq!(ask.chat_provider, Some(ChatProvider::OpenAi));
        assert_eq!(ask.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(ask.max_tokens, Some(1024));
        assert_eq!(ask.temperature, Some(0.5));
        assert_eq!(ask.api_key_env.as_deref(), Some("MY_OPENAI_KEY"));
        assert_eq!(ask.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(ask.chat_session.as_deref(), Some("assistant"));
        assert_eq!(
            ask.truncation,
            Some(ChatTruncation::SlidingWindow { max_tokens: 8000 })
        );
    }

    #[test]
    fn chat_provider_default_variant_roundtrips() {
        // v1 parity: omitting `provider:` defaulted to anthropic. The
        // enum default matches, and the tagged form serializes back as
        // its snake_case label without a rename override.
        assert_eq!(ChatProvider::default(), ChatProvider::Anthropic);

        let yaml = r#"
name: default-provider
steps:
  - name: ask
    type: chat
    prompt: "Hi"
    chat_provider: anthropic
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.steps[0].chat_provider, Some(ChatProvider::Anthropic));
    }

    #[test]
    fn chat_provider_renamed_variants_deserialize() {
        // Guards that the three rename overrides (OpenAi → "openai",
        // DeepSeek → "deepseek", OpenAiCompatible → "openai_compatible")
        // match the v1 provider strings users already have in YAML. The
        // rest of the variants fall out of snake_case automatically.
        let yaml = r#"
name: renamed
steps:
  - name: a
    type: chat
    prompt: "x"
    chat_provider: openai
  - name: b
    type: chat
    prompt: "x"
    chat_provider: deepseek
  - name: c
    type: chat
    prompt: "x"
    chat_provider: openai_compatible
    base_url: "http://localhost:8080/v1"
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(wf.steps[0].chat_provider, Some(ChatProvider::OpenAi));
        assert_eq!(wf.steps[1].chat_provider, Some(ChatProvider::DeepSeek));
        assert_eq!(
            wf.steps[2].chat_provider,
            Some(ChatProvider::OpenAiCompatible)
        );

        // Round-trip: serialized form uses the overridden labels.
        let back = serde_yaml::to_string(&wf).unwrap();
        assert!(
            back.contains("chat_provider: openai\n"),
            "OpenAi must serialize as `openai`: {back}"
        );
        assert!(
            back.contains("chat_provider: deepseek\n"),
            "DeepSeek must serialize as `deepseek`: {back}"
        );
        assert!(
            back.contains("chat_provider: openai_compatible\n"),
            "OpenAiCompatible must serialize as `openai_compatible`: {back}"
        );
    }

    #[test]
    fn chat_truncation_variants_roundtrip_through_yaml() {
        // Each ChatTruncation variant round-trips with the `strategy:`
        // tag + snake_case name. No "none" variant — absent truncation
        // is expressed by omitting the field (normalized by the adapter
        // from a v1 `strategy: none`).
        let yaml = r#"
name: truncs
steps:
  - name: keep_tail
    type: chat
    prompt: "x"
    truncation:
      strategy: last
      count: 10
  - name: keep_head
    type: chat
    prompt: "x"
    truncation:
      strategy: first
      count: 4
  - name: bookend
    type: chat
    prompt: "x"
    truncation:
      strategy: first_last
      first: 2
      last: 8
  - name: window
    type: chat
    prompt: "x"
    truncation:
      strategy: sliding_window
      max_tokens: 4096
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            wf.steps[0].truncation,
            Some(ChatTruncation::Last { count: 10 })
        );
        assert_eq!(
            wf.steps[1].truncation,
            Some(ChatTruncation::First { count: 4 })
        );
        assert_eq!(
            wf.steps[2].truncation,
            Some(ChatTruncation::FirstLast { first: 2, last: 8 })
        );
        assert_eq!(
            wf.steps[3].truncation,
            Some(ChatTruncation::SlidingWindow { max_tokens: 4096 })
        );
    }

    #[test]
    fn cmd_step_omits_chat_fields_from_serialized_output() {
        // Backward-compat mirror of the earlier gate/container/agent
        // guards: cmd steps must not leak any of the seven chat fields
        // added in PR 5b of #31 onto the wire, otherwise existing
        // workflow round-trips would gain noise.
        let step = Step::cmd("s", "true");
        let yaml = serde_yaml::to_string(&step).unwrap();
        for field in [
            "chat_provider:",
            "max_tokens:",
            "temperature:",
            "api_key_env:",
            "base_url:",
            "chat_session:",
            "truncation:",
        ] {
            assert!(
                !yaml.contains(field),
                "cmd step leaked `{field}` onto the wire: {yaml}"
            );
        }
    }

    #[test]
    fn chat_constructor_roundtrips_through_serde() {
        let mut step = Step::chat("ask", "Summarize {{ target }}");
        step.chat_provider = Some(ChatProvider::OpenAi);
        step.model = Some("gpt-4o-mini".into());
        step.max_tokens = Some(256);
        step.chat_session = Some("assistant".into());
        step.truncation = Some(ChatTruncation::Last { count: 20 });

        let wf = Workflow::new("chat-ctor", vec![step]);
        let yaml = serde_yaml::to_string(&wf).unwrap();
        let back: Workflow = serde_yaml::from_str(&yaml).unwrap();

        let s = &back.steps[0];
        assert_eq!(s.kind, StepKind::Chat);
        assert_eq!(s.prompt.as_deref(), Some("Summarize {{ target }}"));
        assert_eq!(s.chat_provider, Some(ChatProvider::OpenAi));
        assert_eq!(s.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(s.max_tokens, Some(256));
        assert_eq!(s.chat_session.as_deref(), Some("assistant"));
        assert_eq!(s.truncation, Some(ChatTruncation::Last { count: 20 }));
    }
}
