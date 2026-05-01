//! Bridge between the legacy [`WorkflowDef`] (with all 10 step types) and the
//! narrower [`stepyard_harness::Workflow`] exposed to the v2 engine.
//!
//! PR 2 of Task #31 accepted `cmd` + `gate` only. PR 3 widens the accept list
//! to the three container kinds (`call` / `repeat` / `map`) and threads their
//! named scope bodies through the harness. Shape validation stays in the CLI
//! adapter so `stepyard-harness` never learns about legacy YAML:
//!
//! * `call` / `repeat` / `map` must reference a named scope that exists in
//!   `scopes:`; missing field → [`AdapterError::ContainerMissingScope`],
//!   unknown ref → [`AdapterError::UnknownScope`].
//! * `map` must declare an `items:` expression, but its rendered shape
//!   stays unenforced here — the scope runner preserves v1's
//!   "JSON array or line-split fallback" heuristic (v1 `map.rs:237`).
//! * Nested containers (a `call` / `repeat` / `map` inside another
//!   container's scope body) are rejected outright
//!   ([`AdapterError::NestedScopesNotSupported`]). A later PR that lifts
//!   this restriction can add a `scope_path` frame without reshaping the
//!   event log; see `stepyard_core::event::ScopeContext`.
//! * Scope bodies currently execute only `cmd` and `gate` steps. Other
//!   non-container kinds are rejected at the adapter boundary until their
//!   scoped replay/render semantics are implemented.
//! * Gate actions split by position: `break` and `skip` only make sense
//!   inside a scope body, so a top-level gate that declares them fails
//!   with [`AdapterError::TopLevelGateUnsupportedAction`]; a scoped
//!   gate that declares an unknown action fails with
//!   [`AdapterError::ScopedGateUnsupportedAction`].
//!
//! PR 4 of Task #31 adds `template` — a pure Tera-render step that reads
//! `<prompts_dir>/<rendered_name>.md.tera`, with `prompts_dir` threaded
//! from the legacy [`WorkflowDef`] (default `"prompts"`) — and `script`,
//! an in-process Rhai evaluation whose return value lands on the unified
//! cmd output shape (`stdout`) so cross-step refs work without a new
//! event variant. The script step's source lives in the YAML `run:` field,
//! reusing the same slot as cmd (v1 parity, `src/steps/script.rs`).
//!
//! PR 5a of Task #31 adds `agent` — translates the v1 `config` bag
//! (`model` / `system_prompt_append` / `permissions` / `resume` /
//! `fork_session` / `session`) into the typed fields the harness now
//! exposes on [`stepyard_harness::Step`]. Top-level only; scoped agent
//! bodies stay rejected by [`AdapterError::ScopedStepUnsupported`] until
//! a later PR widens scope-body dispatch.
//!
//! PR 5b of Task #31 added the chat-step translation helper
//! ([`parse_chat_step`]) — walks the v1 `config` bag (`provider` /
//! `model` / `max_tokens` / `temperature` / `api_key_env` / `base_url` /
//! `session` / `truncation_strategy` family) and produces a typed
//! [`stepyard_harness::Step`]. PR 5c commit 3 wires the helper into
//! [`adapt_step`] so top-level `chat` steps reach the v2 dispatcher;
//! scope-body chat stays rejected via [`AdapterError::ScopedStepUnsupported`]
//! until the engine's chat-session map gains scope-body semantics.
//!
//! Executors for `call` / `repeat` / `map` landed in the scope-runner commit.
//! `parallel` is the only v1 kind still rejected outright with
//! [`AdapterError::UnsupportedStepType`].

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use stepyard_harness::{
    AgentPermissions, AgentSessionMode, ChatProvider, ChatTruncation, Scope, Step, Workflow,
};

use crate::workflow::schema::{ScopeDef, StepDef, StepType, WorkflowDef};

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error(
        "step type `{step_type}` not yet supported by v2 engine — use --engine v1 \
         or migrate the workflow to a supported kind (cmd, gate, call, repeat, map, template, script, agent, chat)"
    )]
    UnsupportedStepType { step_type: StepType },

    #[error("step `{name}` has type cmd but no `run:` field")]
    CmdMissingRun { name: String },

    #[error("script step `{name}` has no `run:` source")]
    ScriptMissingRun { name: String },

    #[error("gate step `{name}` is missing `condition:`")]
    GateMissingCondition { name: String },

    #[error(
        "top-level gate step `{name}` action `{action}=\"{value}\"` is not \
         supported — `break` / `skip` only apply inside a call/repeat/map \
         scope body (allowed at top level: continue, fail)"
    )]
    TopLevelGateUnsupportedAction {
        name: String,
        action: &'static str,
        value: String,
    },

    #[error(
        "scoped gate step `{name}` action `{action}=\"{value}\"` is not a \
         recognized scope outcome (allowed inside a scope: continue, skip, \
         break, fail)"
    )]
    ScopedGateUnsupportedAction {
        name: String,
        action: &'static str,
        value: String,
    },

    #[error("{kind} step `{name}` is missing `scope:`")]
    ContainerMissingScope { name: String, kind: &'static str },

    #[error("map step `{name}` is missing `items:`")]
    MapMissingItems { name: String },

    #[error(
        "{kind} step `{name}` references scope `{scope}` which is not \
         declared in the workflow's `scopes:` block"
    )]
    UnknownScope {
        name: String,
        kind: &'static str,
        scope: String,
    },

    #[error(
        "nested containers are not supported in PR 3 of #31 — {kind} step \
         `{inner_name}` inside scope `{scope}` must be a non-container kind \
         (cmd, gate)"
    )]
    NestedScopesNotSupported {
        scope: String,
        inner_name: String,
        kind: &'static str,
    },

    #[error(
        "scope `{scope}` contains step `{inner_name}` of type `{step_type}` \
         which is not supported inside v2 scope bodies in PR 4 of #31 \
         (allowed inside scopes: cmd, gate)"
    )]
    ScopedStepUnsupported {
        scope: String,
        inner_name: String,
        step_type: StepType,
    },

    #[error(
        "step `{name}` field `{field}:` value is not representable as JSON \
         (harness stores container seeds as JSON): {error}"
    )]
    InitialValueNotJson {
        name: String,
        field: &'static str,
        error: String,
    },

    #[error("agent step `{name}` is missing `prompt:`")]
    AgentMissingPrompt { name: String },

    #[error(
        "agent step `{name}` has non-string `config.{key}` — expected a \
         quoted string (v1 parity: agent fields are always strings in the \
         legacy config bag)"
    )]
    AgentConfigNotString { name: String, key: &'static str },

    #[error(
        "agent step `{name}` declares both `resume:` and `fork_session:` — \
         pick one (v2 adapter rejects the combination explicitly; v1 \
         silently appended `--resume` twice)"
    )]
    AgentResumeAndForkConflict { name: String },

    #[error(
        "agent step `{name}` has `config.permissions=\"{value}\"` — \
         expected `default` or `skip`"
    )]
    AgentInvalidPermissions { name: String, value: String },

    #[error(
        "agent step `{name}` has `config.session=\"{value}\"` — \
         expected `shared` or `isolated`"
    )]
    AgentInvalidSession { name: String, value: String },

    #[error("chat step `{name}` is missing `prompt:`")]
    ChatMissingPrompt { name: String },

    #[error(
        "chat step `{name}` has non-string `config.{key}` — expected a \
         quoted string (v1 parity: chat provider/model/api_key_env/base_url/\
         session/truncation_strategy are always strings in the legacy config bag)"
    )]
    ChatConfigNotString { name: String, key: &'static str },

    #[error(
        "chat step `{name}` has non-integer `config.{key}` — expected a \
         non-negative integer (v1 parity: numeric chat knobs parse as u64)"
    )]
    ChatConfigNotU64 { name: String, key: &'static str },

    #[error(
        "chat step `{name}` has non-numeric `config.{key}` — expected a \
         floating-point number (v1 parity: temperature parses as f64)"
    )]
    ChatConfigNotF64 { name: String, key: &'static str },

    #[error(
        "chat step `{name}` has unknown `config.provider=\"{value}\"` — \
         set `config.base_url:` to treat it as an OpenAI-compatible endpoint, \
         or use a known provider (anthropic, openai, ollama, groq, deepseek, \
         gemini/google, cohere, perplexity, xai/grok, mistral)"
    )]
    ChatUnknownProvider { name: String, value: String },

    #[error(
        "chat step `{name}` uses `config.provider=\"openai_compatible\"` but \
         no `config.base_url:` — set `base_url:` to the endpoint URL so the \
         runtime knows where to POST"
    )]
    ChatOpenAiCompatibleMissingBaseUrl { name: String },

    #[error(
        "chat step `{name}` has `config.truncation_strategy=\"{value}\"` — \
         expected one of: none, last, first, first_last, sliding_window"
    )]
    ChatInvalidTruncationStrategy { name: String, value: String },

    #[error(
        "chat step `{name}` has malformed `config.timeout=\"{value}\"` — \
         expected a non-negative integer optionally suffixed with `ms`, `s`, \
         or `m` (v1 parity: bare numbers are seconds)"
    )]
    ChatInvalidTimeout { name: String, value: String },

    #[error(
        "parallel step `{name}` has no `steps:` block — parallel must list at \
         least one sub-step (v1 parity)"
    )]
    ParallelMissingSteps { name: String },

    #[error(
        "parallel step `{parent}` sub-step `{inner_name}` has type `{step_type}` \
         which is not supported in PR 6 of #31 (allowed inside parallel: cmd)"
    )]
    ParallelSubStepUnsupported {
        parent: String,
        inner_name: String,
        step_type: StepType,
    },

    #[error(
        "parallel step `{name}` synthesises scope `{scope}` but the workflow \
         already declares a scope with that name — `__parallel_*` is reserved \
         for adapter-synthesised parallel scopes; rename the user-declared scope"
    )]
    ParallelSynthScopeCollision { name: String, scope: String },
}

#[derive(Clone, Copy)]
enum StepPosition {
    TopLevel,
    Scoped,
}

/// Convert a parsed [`WorkflowDef`] into the harness-facing [`Workflow`].
///
/// Walks the top-level step list and every declared scope body. Executable
/// kinds (`cmd` / `gate` / `call` / `repeat` / `map` / `template` / `script`)
/// are adapted; scoped bodies additionally reject nested containers and
/// unsupported non-container kinds. Env maps (workflow-level and step-level)
/// are threaded through so the cascade
/// resolver (Story 3.4) has the values the v2 engine expects.
pub fn adapt(def: &WorkflowDef) -> Result<Workflow, AdapterError> {
    let scope_names: HashSet<&str> = def.scopes.keys().map(String::as_str).collect();

    // PR 6 of #31: each top-level `parallel` step's YAML `steps:` body is
    // synthesised into a hidden scope named `__parallel_<top_level_index>`.
    // The harness sees parallel as just another scope-bodied container.
    // Synth scopes accumulate here and merge into the final scopes map
    // after the regular scope pass; collisions with user-declared scopes
    // (a workflow that names a scope `__parallel_0` itself) are rejected.
    let mut steps = Vec::with_capacity(def.steps.len());
    let mut synth_scopes: HashMap<String, Scope> = HashMap::new();
    for (idx, s) in def.steps.iter().enumerate() {
        if matches!(s.step_type, StepType::Parallel) {
            let (step, synth_name, synth_scope) = adapt_parallel(s, idx)?;
            if scope_names.contains(synth_name.as_str()) {
                return Err(AdapterError::ParallelSynthScopeCollision {
                    name: s.name.clone(),
                    scope: synth_name,
                });
            }
            steps.push(step);
            synth_scopes.insert(synth_name, synth_scope);
        } else {
            steps.push(adapt_step(s, &scope_names, StepPosition::TopLevel)?);
        }
    }

    let mut scopes: HashMap<String, Scope> =
        HashMap::with_capacity(def.scopes.len() + synth_scopes.len());
    for (scope_name, scope_def) in &def.scopes {
        scopes.insert(
            scope_name.clone(),
            adapt_scope(scope_name, scope_def, &scope_names)?,
        );
    }
    scopes.extend(synth_scopes);

    let mut wf = Workflow::new(def.name.clone(), steps);
    wf.env = def.env.clone();
    wf.scopes = scopes;
    // PR 4 of #31: thread `prompts_dir:` from legacy YAML so template
    // steps resolve the same files v1 did. Absent → harness default
    // (`"prompts"`, per `stepyard_harness::template_exec::DEFAULT_PROMPTS_DIR`).
    wf.prompts_dir = def.prompts_dir.clone();
    Ok(wf)
}

fn adapt_scope(
    scope_name: &str,
    scope_def: &ScopeDef,
    scope_names: &HashSet<&str>,
) -> Result<Scope, AdapterError> {
    let mut body = Vec::with_capacity(scope_def.steps.len());
    for inner in &scope_def.steps {
        // Guardrail 1: nested containers — caught here before adapt_step so
        // the inner kind can't be silently adapted as if top-level.
        if let Some(kind) = container_kind(&inner.step_type) {
            return Err(AdapterError::NestedScopesNotSupported {
                scope: scope_name.to_string(),
                inner_name: inner.name.clone(),
                kind,
            });
        }
        if !scope_body_kind_supported(&inner.step_type) {
            return Err(AdapterError::ScopedStepUnsupported {
                scope: scope_name.to_string(),
                inner_name: inner.name.clone(),
                step_type: inner.step_type.clone(),
            });
        }
        body.push(adapt_step(inner, scope_names, StepPosition::Scoped)?);
    }
    Ok(Scope {
        steps: body,
        outputs: scope_def.outputs.clone(),
    })
}

fn adapt_step(
    s: &StepDef,
    scope_names: &HashSet<&str>,
    position: StepPosition,
) -> Result<Step, AdapterError> {
    match &s.step_type {
        StepType::Cmd => adapt_cmd(s),
        StepType::Gate => adapt_gate(s, position),
        StepType::Call => adapt_call(s, scope_names),
        StepType::Repeat => adapt_repeat(s, scope_names),
        StepType::Map => adapt_map(s, scope_names),
        StepType::Template => adapt_template(s),
        StepType::Script => adapt_script(s),
        StepType::Agent => adapt_agent(s),
        StepType::Chat => parse_chat_step(s),
        other => Err(AdapterError::UnsupportedStepType {
            step_type: other.clone(),
        }),
    }
}

fn adapt_cmd(s: &StepDef) -> Result<Step, AdapterError> {
    let cmd = s.run.clone().ok_or_else(|| AdapterError::CmdMissingRun {
        name: s.name.clone(),
    })?;
    Ok(Step::cmd(s.name.clone(), cmd).with_env(s.env.clone()))
}

fn adapt_gate(s: &StepDef, position: StepPosition) -> Result<Step, AdapterError> {
    let condition = s
        .condition
        .clone()
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| AdapterError::GateMissingCondition {
            name: s.name.clone(),
        })?;

    // Guardrail 2: top-level gates reject break/skip; scoped gates accept
    // the full {continue, fail, skip, break} set.
    validate_gate_action(&s.name, "on_pass", s.on_pass.as_deref(), position)?;
    validate_gate_action(&s.name, "on_fail", s.on_fail.as_deref(), position)?;

    let mut step = Step::gate(s.name.clone(), condition);
    step.on_pass = s.on_pass.clone();
    step.on_fail = s.on_fail.clone();
    step.message = s.message.clone();
    step.env = s.env.clone();
    Ok(step)
}

fn adapt_call(s: &StepDef, scope_names: &HashSet<&str>) -> Result<Step, AdapterError> {
    let scope = require_scope_ref(s, "call", scope_names)?;
    let mut step = Step::call(s.name.clone(), scope);
    apply_common_container_fields(&mut step, s)?;
    Ok(step)
}

fn adapt_repeat(s: &StepDef, scope_names: &HashSet<&str>) -> Result<Step, AdapterError> {
    let scope = require_scope_ref(s, "repeat", scope_names)?;
    let mut step = Step::repeat(s.name.clone(), scope);
    step.max_iterations = s.max_iterations;
    apply_common_container_fields(&mut step, s)?;
    Ok(step)
}

fn adapt_template(s: &StepDef) -> Result<Step, AdapterError> {
    // Template has no required fields: both `prompt` (resolves the file
    // basename) and `name` (fallback) exist. v1 parity — template_step.rs
    // accepts missing prompt and falls back to step name.
    let mut step = Step::template(s.name.clone(), s.prompt.clone());
    step.env = s.env.clone();
    Ok(step)
}

fn adapt_agent(s: &StepDef) -> Result<Step, AdapterError> {
    // Agent steps own the legacy `prompt:` typed field on `StepDef`; the
    // harness's executor (PR 5a commit 3b) pipes it into the CLI's stdin.
    let prompt = s
        .prompt
        .clone()
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| AdapterError::AgentMissingPrompt {
            name: s.name.clone(),
        })?;

    let mut step = Step::agent(s.name.clone(), prompt).with_env(s.env.clone());

    // v1 surfaced agent knobs through the untyped `config:` bag
    // (`src/workflow/schema.rs:128`). Translate the seven string keys the
    // executor cares about (model, system_prompt_append, permissions,
    // resume, fork_session, session, command) into typed fields so the
    // v2 harness can stay YAML-unaware.
    step.model = agent_config_str(s, "model")?.map(String::from);
    step.system_prompt_append = agent_config_str(s, "system_prompt_append")?.map(String::from);

    if let Some(value) = agent_config_str(s, "permissions")? {
        step.permissions = Some(parse_agent_permissions(&s.name, value)?);
    }

    let resume = agent_config_str(s, "resume")?.map(String::from);
    let fork = agent_config_str(s, "fork_session")?.map(String::from);
    if resume.is_some() && fork.is_some() {
        return Err(AdapterError::AgentResumeAndForkConflict {
            name: s.name.clone(),
        });
    }
    step.resume = resume;
    step.fork_session = fork;

    if let Some(value) = agent_config_str(s, "session")? {
        step.agent_session = Some(parse_agent_session(&s.name, value)?);
    }

    // v1 parity for the CLI binary path — `config.command` (default
    // `"claude"`) at `src/steps/agent.rs:24`. Preserves workflows that
    // point at a wrapper, mock, path-pinned claude, or corporate script.
    // Absent = executor falls back to `"claude"` at spawn time.
    step.agent_command = agent_config_str(s, "command")?.map(String::from);

    Ok(step)
}

/// Looks up a string key in the v1 `config:` bag, erroring if the value
/// exists but is not a string scalar. Matches v1's
/// `StepConfig::get_str` shape — v1 silently returned `None` on
/// non-string payloads; v2 surfaces the mismatch so operators don't ship
/// a misspelled YAML shape that silently ignores a flag.
fn agent_config_str<'a>(
    s: &'a StepDef,
    key: &'static str,
) -> Result<Option<&'a str>, AdapterError> {
    match s.config.get(key) {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(serde_yaml::Value::String(v)) => Ok(Some(v.as_str())),
        Some(_) => Err(AdapterError::AgentConfigNotString {
            name: s.name.clone(),
            key,
        }),
    }
}

fn parse_agent_permissions(step_name: &str, value: &str) -> Result<AgentPermissions, AdapterError> {
    match value {
        "default" => Ok(AgentPermissions::Default),
        "skip" => Ok(AgentPermissions::Skip),
        other => Err(AdapterError::AgentInvalidPermissions {
            name: step_name.to_string(),
            value: other.to_string(),
        }),
    }
}

fn parse_agent_session(step_name: &str, value: &str) -> Result<AgentSessionMode, AdapterError> {
    match value {
        "shared" => Ok(AgentSessionMode::Shared),
        "isolated" => Ok(AgentSessionMode::Isolated),
        other => Err(AdapterError::AgentInvalidSession {
            name: step_name.to_string(),
            value: other.to_string(),
        }),
    }
}

/// Translates a v1 `type: chat` step into a typed [`Step`] by walking
/// the `config:` bag (`provider` / `model` / `max_tokens` / `temperature`
/// / `api_key_env` / `base_url` / `session` / `truncation_strategy` and
/// its `truncation_count` / `truncation_first` / `truncation_last` /
/// `truncation_max_tokens` siblings).
///
/// Wired into [`adapt_step`] in PR 5c commit 3 — top-level `chat`
/// steps now flow through this helper into the v2 dispatcher. Scope-body
/// chat is still rejected by [`AdapterError::ScopedStepUnsupported`]
/// (see [`scope_body_kind_supported`]) because the engine's chat-session
/// map commits only on a non-scoped chat completion (`engine.rs:1837`).
fn parse_chat_step(s: &StepDef) -> Result<Step, AdapterError> {
    let prompt = s
        .prompt
        .clone()
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| AdapterError::ChatMissingPrompt {
            name: s.name.clone(),
        })?;

    let mut step = Step::chat(s.name.clone(), prompt).with_env(s.env.clone());

    // The adapter cristallizes every v1 "missing = default" fallback into
    // an explicit typed value here so the runtime never has to guess. v1
    // spread these defaults across `src/steps/chat.rs:340-357`; keeping
    // them in the adapter matches D6 (resolve env/defaults at the
    // adapter boundary) and leaves the runtime a single clean code path.
    let provider_raw = chat_config_str(s, "provider")?.unwrap_or("anthropic");
    let base_url = chat_config_str(s, "base_url")?.map(String::from);
    let provider = parse_chat_provider(&s.name, provider_raw, base_url.as_deref())?;

    step.model = Some(
        chat_config_str(s, "model")?
            .map(String::from)
            .unwrap_or_else(|| provider.default_model().to_string()),
    );
    step.api_key_env = match chat_config_str(s, "api_key_env")? {
        Some(v) => Some(v.to_string()),
        None => provider.default_api_key_env().map(String::from),
    };
    step.max_tokens = Some(chat_config_u64(s, "max_tokens")?.unwrap_or(1024));
    step.temperature = Some(chat_config_f64(s, "temperature")?.unwrap_or(0.0));
    step.timeout = Some(
        parse_chat_timeout_duration(s)?.unwrap_or_else(|| Duration::from_secs(120)),
    );
    step.chat_provider = Some(provider);
    step.base_url = base_url;
    step.chat_session = chat_config_str(s, "session")?.map(String::from);
    step.truncation = parse_chat_truncation(s)?;

    Ok(step)
}

/// Parses the v1 duration format used by `config.timeout:` into a
/// [`Duration`] (v1 source: `src/config/mod.rs:42-57`). Accepts string
/// values with `ms` / `s` / `m` suffixes — bare numeric strings are
/// seconds per v1 parity. Integer/float YAML values error (v1's
/// `get_duration` silently fell back to the default in that case — a
/// quirk the adapter tightens so operators learn about malformed
/// timeouts at load time instead of at the 120-second default boundary).
///
/// **Not a strict-grammar parser.** This is the v1 chat-config
/// compatibility boundary: it intentionally does NOT delegate to
/// [`stepyard_core::duration::parse_duration`], because that parser
/// rejects bare numerics and the `h` suffix is not part of the v1
/// chat-config surface. Round 3 Story 1 tightened the workflow-level
/// `timeout:` field (see `stepyard-harness::Workflow::try_from_yaml`);
/// migrating this adapter-internal helper to the strict grammar is a
/// separate decision that would break documented v1 chat-config YAML.
fn parse_chat_timeout_duration(s: &StepDef) -> Result<Option<Duration>, AdapterError> {
    let raw = match chat_config_str(s, "timeout")? {
        Some(v) => v,
        None => return Ok(None),
    };
    let trimmed = raw.trim();
    let (digits, multiplier_ms): (&str, u64) = if let Some(rest) = trimmed.strip_suffix("ms") {
        (rest.trim_end(), 1)
    } else if let Some(rest) = trimmed.strip_suffix('s') {
        (rest.trim_end(), 1_000)
    } else if let Some(rest) = trimmed.strip_suffix('m') {
        (rest.trim_end(), 60_000)
    } else {
        (trimmed, 1_000)
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| AdapterError::ChatInvalidTimeout {
            name: s.name.clone(),
            value: raw.to_string(),
        })?;
    n.checked_mul(multiplier_ms)
        .map(|ms| Some(Duration::from_millis(ms)))
        .ok_or_else(|| AdapterError::ChatInvalidTimeout {
            name: s.name.clone(),
            value: raw.to_string(),
        })
}

/// Looks up a string-valued chat `config:` key. Mirrors
/// [`agent_config_str`] for the same reason — v1 silently returned
/// `None` on non-string payloads; v2 surfaces the mismatch so a
/// misspelled YAML shape doesn't drop a flag on the floor.
#[allow(dead_code)] // wired into adapt_step in PR 5c
fn chat_config_str<'a>(s: &'a StepDef, key: &'static str) -> Result<Option<&'a str>, AdapterError> {
    match s.config.get(key) {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(serde_yaml::Value::String(v)) => Ok(Some(v.as_str())),
        Some(_) => Err(AdapterError::ChatConfigNotString {
            name: s.name.clone(),
            key,
        }),
    }
}

/// Looks up a `u64`-valued chat `config:` key. v1's `get_u64` silently
/// swallowed non-integer payloads (a stray string on `max_tokens` would
/// ship as "no override"); the adapter errors instead so a typo in the
/// YAML surfaces at load time.
#[allow(dead_code)] // wired into adapt_step in PR 5c
fn chat_config_u64(s: &StepDef, key: &'static str) -> Result<Option<u64>, AdapterError> {
    match s.config.get(key) {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(serde_yaml::Value::Number(n)) => {
            n.as_u64()
                .map(Some)
                .ok_or_else(|| AdapterError::ChatConfigNotU64 {
                    name: s.name.clone(),
                    key,
                })
        }
        Some(_) => Err(AdapterError::ChatConfigNotU64 {
            name: s.name.clone(),
            key,
        }),
    }
}

/// Looks up an `f64`-valued chat `config:` key (only `temperature`
/// today). Accepts both integer and float YAML numbers so
/// `temperature: 0` and `temperature: 0.7` both work — v1 parity at
/// `src/steps/chat.rs:350-354`.
#[allow(dead_code)] // wired into adapt_step in PR 5c
fn chat_config_f64(s: &StepDef, key: &'static str) -> Result<Option<f64>, AdapterError> {
    match s.config.get(key) {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(serde_yaml::Value::Number(n)) => {
            n.as_f64()
                .map(Some)
                .ok_or_else(|| AdapterError::ChatConfigNotF64 {
                    name: s.name.clone(),
                    key,
                })
        }
        Some(_) => Err(AdapterError::ChatConfigNotF64 {
            name: s.name.clone(),
            key,
        }),
    }
}

/// Resolves a v1 provider string to a typed [`ChatProvider`]. Mirrors
/// the v1 match arm at `src/steps/chat.rs:220-322`, including the
/// `google → Gemini` and `grok → Xai` aliases. Unknown providers fall
/// through to [`ChatProvider::OpenAiCompatible`] only when `base_url:`
/// is set — v1's escape hatch for self-hosted gateways. Without it, an
/// unknown name is treated as a typo.
#[allow(dead_code)] // wired into adapt_step in PR 5c
fn parse_chat_provider(
    step_name: &str,
    value: &str,
    base_url: Option<&str>,
) -> Result<ChatProvider, AdapterError> {
    match value {
        "anthropic" => Ok(ChatProvider::Anthropic),
        "openai" => Ok(ChatProvider::OpenAi),
        "ollama" => Ok(ChatProvider::Ollama),
        "groq" => Ok(ChatProvider::Groq),
        "deepseek" => Ok(ChatProvider::DeepSeek),
        "gemini" | "google" => Ok(ChatProvider::Gemini),
        "cohere" => Ok(ChatProvider::Cohere),
        "perplexity" => Ok(ChatProvider::Perplexity),
        "xai" | "grok" => Ok(ChatProvider::Xai),
        "mistral" => Ok(ChatProvider::Mistral),
        "openai_compatible" => {
            if base_url.is_some() {
                Ok(ChatProvider::OpenAiCompatible)
            } else {
                Err(AdapterError::ChatOpenAiCompatibleMissingBaseUrl {
                    name: step_name.to_string(),
                })
            }
        }
        other => {
            if base_url.is_some() {
                Ok(ChatProvider::OpenAiCompatible)
            } else {
                Err(AdapterError::ChatUnknownProvider {
                    name: step_name.to_string(),
                    value: other.to_string(),
                })
            }
        }
    }
}

/// Resolves the v1 flat truncation knobs (`truncation_strategy` plus
/// `truncation_count` / `truncation_first` / `truncation_last` /
/// `truncation_max_tokens`) into a typed [`ChatTruncation`]. Absent or
/// `"none"` normalize to `None` at the adapter boundary — the runtime
/// only sees a strategy when one was actually requested. Default counts
/// match `src/steps/chat.rs:34-57` (10, 10, 2+5, 50_000).
#[allow(dead_code)] // wired into adapt_step in PR 5c
fn parse_chat_truncation(s: &StepDef) -> Result<Option<ChatTruncation>, AdapterError> {
    let Some(strategy) = chat_config_str(s, "truncation_strategy")? else {
        return Ok(None);
    };
    match strategy {
        "none" => Ok(None),
        "last" => {
            let count = chat_config_u64(s, "truncation_count")?.unwrap_or(10);
            Ok(Some(ChatTruncation::Last { count }))
        }
        "first" => {
            let count = chat_config_u64(s, "truncation_count")?.unwrap_or(10);
            Ok(Some(ChatTruncation::First { count }))
        }
        "first_last" => {
            let first = chat_config_u64(s, "truncation_first")?.unwrap_or(2);
            let last = chat_config_u64(s, "truncation_last")?.unwrap_or(5);
            Ok(Some(ChatTruncation::FirstLast { first, last }))
        }
        "sliding_window" => {
            let max_tokens = chat_config_u64(s, "truncation_max_tokens")?.unwrap_or(50_000);
            Ok(Some(ChatTruncation::SlidingWindow { max_tokens }))
        }
        other => Err(AdapterError::ChatInvalidTruncationStrategy {
            name: s.name.clone(),
            value: other.to_string(),
        }),
    }
}

fn adapt_script(s: &StepDef) -> Result<Step, AdapterError> {
    // `run:` is the Rhai source (v1 parity with `src/steps/script.rs`).
    // Reject empty sources at the adapter so the harness never dispatches
    // a script step with nothing to evaluate.
    let source = s
        .run
        .clone()
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| AdapterError::ScriptMissingRun {
            name: s.name.clone(),
        })?;
    Ok(Step::script(s.name.clone(), source).with_env(s.env.clone()))
}

fn adapt_map(s: &StepDef, scope_names: &HashSet<&str>) -> Result<Step, AdapterError> {
    let scope = require_scope_ref(s, "map", scope_names)?;
    // Guardrail 4: presence only. The render-time shape check stays with the
    // scope runner (v1 heuristic, map.rs:237).
    let items = s
        .items
        .clone()
        .filter(|i| !i.trim().is_empty())
        .ok_or_else(|| AdapterError::MapMissingItems {
            name: s.name.clone(),
        })?;
    let mut step = Step::map(s.name.clone(), scope, items);
    step.parallel = s.parallel;
    apply_common_container_fields(&mut step, s)?;
    Ok(step)
}

fn apply_common_container_fields(step: &mut Step, s: &StepDef) -> Result<(), AdapterError> {
    step.env = s.env.clone();
    step.outputs = s.outputs.clone();
    if let Some(v) = &s.initial_value {
        step.initial_value = Some(yaml_to_json(v, &s.name, "initial_value")?);
    }
    Ok(())
}

/// Adapt a top-level `parallel` step (PR 6 of #31).
///
/// v1 stored sub-steps inline on `step.steps:`. v2's harness model is
/// scope-based, so the adapter synthesises a hidden scope named
/// `__parallel_<top_level_index>` carrying the v1 sub-step list, and
/// the [`Step::parallel`] points at that scope. The harness's
/// `run_parallel` then runs every sub-step in `JoinSet` and synthesises
/// the container's terminal output from the LAST sub-step by definition
/// order (v1 parity).
///
/// PR 6 narrows v1's "any nested step type" to `cmd`-only — agent /
/// chat dispatch inside scope bodies is task #80 and lands separately.
fn adapt_parallel(
    s: &StepDef,
    top_level_index: usize,
) -> Result<(Step, String, Scope), AdapterError> {
    let synth_name = format!("__parallel_{top_level_index}");

    let raw_steps = s
        .steps
        .as_ref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| AdapterError::ParallelMissingSteps {
            name: s.name.clone(),
        })?;

    let mut body: Vec<Step> = Vec::with_capacity(raw_steps.len());
    for inner in raw_steps {
        // Defence-in-depth: nested containers in parallel.steps. A
        // `parallel` whose sub-step is a `call`/`repeat`/`map`/`parallel`
        // would need a multi-level scope_path (see `ScopeContext` doc on
        // future `scope_path: Vec<ScopeFrame>`); reject for PR 6.
        if let Some(kind) = container_kind(&inner.step_type) {
            return Err(AdapterError::NestedScopesNotSupported {
                scope: synth_name.clone(),
                inner_name: inner.name.clone(),
                kind,
            });
        }
        // PR 6 of #31 / #80 deferral: parallel sub-steps are restricted
        // to `cmd` until agent/chat dispatch is wired into scope bodies.
        // Mirrors the harness's defensive `matches!(sub.kind, StepKind::Cmd)`
        // check so the YAML rejection is symmetric with the runtime one.
        if !matches!(inner.step_type, StepType::Cmd) {
            return Err(AdapterError::ParallelSubStepUnsupported {
                parent: s.name.clone(),
                inner_name: inner.name.clone(),
                step_type: inner.step_type.clone(),
            });
        }
        // Pass an empty scope_names: cmd doesn't reference scopes, and
        // any non-cmd sub-step has already been rejected above. The
        // `Scoped` position keeps gate-action validation conservative
        // for any future widening.
        body.push(adapt_step(inner, &HashSet::new(), StepPosition::Scoped)?);
    }

    let mut step = Step::parallel(s.name.clone(), &synth_name);
    apply_common_container_fields(&mut step, s)?;

    let scope = Scope {
        steps: body,
        outputs: None,
    };

    Ok((step, synth_name, scope))
}

fn container_kind(step_type: &StepType) -> Option<&'static str> {
    match step_type {
        StepType::Call => Some("call"),
        StepType::Repeat => Some("repeat"),
        StepType::Map => Some("map"),
        // PR 6 of #31: parallel is a scope-bodied container too. Listing
        // it here makes `adapt_scope` reject `parallel` inside another
        // scope body (nested-container guardrail) and matches the runner
        // dispatch in `stepyard_harness::engine::run_container_step`.
        StepType::Parallel => Some("parallel"),
        _ => None,
    }
}

fn scope_body_kind_supported(step_type: &StepType) -> bool {
    matches!(step_type, StepType::Cmd | StepType::Gate)
}

fn require_scope_ref(
    s: &StepDef,
    kind: &'static str,
    scope_names: &HashSet<&str>,
) -> Result<String, AdapterError> {
    // Guardrail 3: call/repeat/map need both a non-empty `scope:` value AND
    // a matching entry in `scopes:`. Two distinct errors so the message
    // points at the actual mistake.
    let scope = s
        .scope
        .clone()
        .filter(|r| !r.trim().is_empty())
        .ok_or_else(|| AdapterError::ContainerMissingScope {
            name: s.name.clone(),
            kind,
        })?;
    if !scope_names.contains(scope.as_str()) {
        return Err(AdapterError::UnknownScope {
            name: s.name.clone(),
            kind,
            scope,
        });
    }
    Ok(scope)
}

fn validate_gate_action(
    step_name: &str,
    field: &'static str,
    raw: Option<&str>,
    position: StepPosition,
) -> Result<(), AdapterError> {
    let trimmed = raw.map(str::trim).unwrap_or("");
    if trimmed.is_empty() {
        return Ok(());
    }
    let allowed: &[&str] = match position {
        StepPosition::TopLevel => &["continue", "fail"],
        StepPosition::Scoped => &["continue", "fail", "skip", "break"],
    };
    if allowed.contains(&trimmed) {
        return Ok(());
    }
    Err(match position {
        StepPosition::TopLevel => AdapterError::TopLevelGateUnsupportedAction {
            name: step_name.to_string(),
            action: field,
            value: trimmed.to_string(),
        },
        StepPosition::Scoped => AdapterError::ScopedGateUnsupportedAction {
            name: step_name.to_string(),
            action: field,
            value: trimmed.to_string(),
        },
    })
}

fn yaml_to_json(
    v: &serde_yaml::Value,
    step_name: &str,
    field: &'static str,
) -> Result<serde_json::Value, AdapterError> {
    serde_json::to_value(v).map_err(|e| AdapterError::InitialValueNotJson {
        name: step_name.to_string(),
        field,
        error: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::parser;
    use std::io::Write;

    fn write_tmp(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(".yaml").tempfile().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn adapts_cmd_only_workflow() {
        let yaml = r#"
name: adapter-smoke
steps:
  - name: one
    type: cmd
    run: "echo 1"
  - name: two
    type: cmd
    run: "echo 2"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        assert_eq!(wf.name, "adapter-smoke");
        assert_eq!(wf.steps.len(), 2);
        assert_eq!(wf.steps[0].name, "one");
        assert_eq!(wf.steps[1].command, "echo 2");
    }

    #[test]
    fn carries_env_at_workflow_and_step_scopes() {
        let yaml = r#"
name: adapter-env
env:
  WF_VAR: workflow_value
  SHARED: from_workflow
steps:
  - name: one
    type: cmd
    run: "echo 1"
    env:
      STEP_VAR: step_value
      SHARED: from_step
  - name: two
    type: cmd
    run: "echo 2"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();

        assert_eq!(
            wf.env.get("WF_VAR").map(String::as_str),
            Some("workflow_value")
        );
        assert_eq!(
            wf.env.get("SHARED").map(String::as_str),
            Some("from_workflow")
        );

        let step_one_env = &wf.steps[0].env;
        assert_eq!(
            step_one_env.get("STEP_VAR").map(String::as_str),
            Some("step_value")
        );
        assert_eq!(
            step_one_env.get("SHARED").map(String::as_str),
            Some("from_step")
        );

        assert!(
            wf.steps[1].env.is_empty(),
            "step without env: should have empty map, got {:?}",
            wf.steps[1].env
        );
    }

    #[test]
    fn accepts_gate_step() {
        let yaml = r#"
name: adapter-gate
steps:
  - name: check
    type: gate
    condition: "{{ steps.build.exit_code }} == 0"
    on_pass: continue
    on_fail: fail
    message: "build must be green"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        assert_eq!(wf.steps.len(), 1);
        let step = &wf.steps[0];
        assert_eq!(step.name, "check");
        assert_eq!(
            step.condition.as_deref(),
            Some("{{ steps.build.exit_code }} == 0")
        );
        assert_eq!(step.on_pass.as_deref(), Some("continue"));
        assert_eq!(step.on_fail.as_deref(), Some("fail"));
        assert_eq!(step.message.as_deref(), Some("build must be green"));
    }

    #[test]
    fn rejects_gate_without_condition() {
        let yaml = r#"
name: adapter-gate-no-cond
steps:
  - name: naked
    type: gate
    on_pass: continue
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("missing `condition:`"), "msg={msg}");
        assert!(msg.contains("naked"), "msg={msg}");
    }

    #[test]
    fn top_level_gate_rejects_break_and_skip() {
        for (field, value) in [
            ("on_pass", "break"),
            ("on_pass", "skip"),
            ("on_fail", "break"),
            ("on_fail", "skip"),
        ] {
            let yaml = format!(
                r#"
name: adapter-top-gate-bad-action
steps:
  - name: g
    type: gate
    condition: "true"
    {field}: {value}
"#
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let err = adapt(&def).unwrap_err();
            assert!(
                matches!(err, AdapterError::TopLevelGateUnsupportedAction { .. }),
                "expected TopLevelGateUnsupportedAction, got {err:?}"
            );
            let msg = err.to_string();
            assert!(msg.contains(value), "msg={msg}");
            assert!(msg.contains(field), "msg={msg}");
            assert!(msg.contains("top-level gate"), "msg={msg}");
        }
    }

    #[test]
    fn accepts_parallel_with_cmd_sub_steps_synthesizing_scope() {
        // PR 6 of #31: a top-level `parallel` step's inline `steps:` body
        // is lifted into a hidden scope named `__parallel_<top_level_index>`.
        // The harness sees parallel as just another scope-bodied container.
        let yaml = r#"
name: adapter-parallel-cmd-only
steps:
  - name: fan
    type: parallel
    steps:
      - name: a
        type: cmd
        run: "echo aaa"
      - name: b
        type: cmd
        run: "echo bbb"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).expect("adapt should succeed");

        // The top-level step itself should still be named `fan` and
        // point at the synthesised scope `__parallel_0`.
        assert_eq!(wf.steps.len(), 1);
        let fan = &wf.steps[0];
        assert_eq!(fan.name, "fan");
        assert_eq!(fan.scope.as_deref(), Some("__parallel_0"));

        // Synth scope holds both sub-steps in declaration order.
        let synth = wf
            .scopes
            .get("__parallel_0")
            .expect("__parallel_0 scope should be synthesised");
        assert_eq!(synth.steps.len(), 2);
        assert_eq!(synth.steps[0].name, "a");
        assert_eq!(synth.steps[1].name, "b");
        assert!(synth.outputs.is_none(), "synth scopes do not carry outputs templates");
    }

    #[test]
    fn rejects_parallel_with_missing_steps() {
        let yaml = r#"
name: adapter-parallel-missing-steps
steps:
  - name: fan
    type: parallel
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no `steps:` block"), "msg={msg}");
        assert!(msg.contains("fan"), "msg={msg}");
    }

    #[test]
    fn rejects_parallel_with_empty_steps_list() {
        let yaml = r#"
name: adapter-parallel-empty-steps
steps:
  - name: fan
    type: parallel
    steps: []
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no `steps:` block"), "msg={msg}");
    }

    #[test]
    fn rejects_parallel_with_unsupported_sub_step_kind() {
        // PR 6 narrows parallel sub-steps to `cmd` only. agent/chat
        // would land via #80 once scope bodies dispatch them.
        let yaml = r#"
name: adapter-parallel-rejects-agent
steps:
  - name: fan
    type: parallel
    steps:
      - name: ask
        type: agent
        prompt: "hi"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("allowed inside parallel: cmd"), "msg={msg}");
        assert!(msg.contains("ask"), "msg={msg}");
        assert!(msg.contains("fan"), "msg={msg}");
    }

    #[test]
    fn rejects_parallel_with_nested_container_sub_step() {
        // Nested containers inside parallel are rejected at the adapter
        // boundary; the synth-scope path uses the same NestedScopesNotSupported
        // error as `adapt_scope` for consistency.
        let yaml = r#"
name: adapter-parallel-rejects-nested-call
steps:
  - name: fan
    type: parallel
    steps:
      - name: inner
        type: call
        scope: setup
scopes:
  setup:
    steps:
      - name: seed
        type: cmd
        run: "echo seed"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nested containers are not supported"), "msg={msg}");
        assert!(msg.contains("inner"), "msg={msg}");
    }

    #[test]
    fn rejects_parallel_synth_scope_collision_with_user_scope() {
        // A user explicitly declaring `__parallel_0` collides with the
        // synth scope for a top-level parallel at index 0.
        let yaml = r#"
name: adapter-parallel-synth-collision
steps:
  - name: fan
    type: parallel
    steps:
      - name: a
        type: cmd
        run: "echo a"
scopes:
  __parallel_0:
    steps:
      - name: oops
        type: cmd
        run: "echo nope"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("`__parallel_*` is reserved"), "msg={msg}");
        assert!(msg.contains("fan"), "msg={msg}");
    }

    #[test]
    fn rejects_parallel_inside_a_scope_body() {
        // `parallel` is a container kind; placing one inside a `call`
        // scope hits the nested-containers guardrail, same as a nested
        // `call`/`repeat`/`map`.
        let yaml = r#"
name: adapter-parallel-rejected-inside-scope
steps:
  - name: outer
    type: call
    scope: setup
scopes:
  setup:
    steps:
      - name: nested_fan
        type: parallel
        steps:
          - name: a
            type: cmd
            run: "echo a"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nested containers are not supported"), "msg={msg}");
        assert!(msg.contains("nested_fan"), "msg={msg}");
        assert!(msg.contains("parallel"), "msg={msg}");
    }

    #[test]
    fn accepts_call_repeat_map_with_valid_scopes() {
        let yaml = r#"
name: adapter-containers
steps:
  - name: once
    type: call
    scope: setup
  - name: loop
    type: repeat
    scope: work
    max_iterations: 3
    initial_value: 0
  - name: fan
    type: map
    scope: per_item
    items: "{{ steps.list.stdout }}"
    parallel: 2
scopes:
  setup:
    steps:
      - name: seed
        type: cmd
        run: "echo seed"
    outputs: "{{ steps.seed.stdout }}"
  work:
    steps:
      - name: tick
        type: cmd
        run: "echo tick"
      - name: maybe_stop
        type: gate
        condition: "false"
        on_pass: break
  per_item:
    steps:
      - name: touch
        type: cmd
        run: "echo {{ item }}"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();

        assert_eq!(wf.steps.len(), 3);

        let call = &wf.steps[0];
        assert_eq!(call.kind, stepyard_harness::StepKind::Call);
        assert_eq!(call.scope.as_deref(), Some("setup"));

        let rep = &wf.steps[1];
        assert_eq!(rep.kind, stepyard_harness::StepKind::Repeat);
        assert_eq!(rep.max_iterations, Some(3));
        assert_eq!(rep.initial_value, Some(serde_json::json!(0)));

        let map = &wf.steps[2];
        assert_eq!(map.kind, stepyard_harness::StepKind::Map);
        assert_eq!(map.items.as_deref(), Some("{{ steps.list.stdout }}"));
        assert_eq!(map.parallel, Some(2));

        let setup = wf.scopes.get("setup").unwrap();
        assert_eq!(setup.outputs.as_deref(), Some("{{ steps.seed.stdout }}"));

        // Scoped gate accepted `break` on on_pass.
        let work = wf.scopes.get("work").unwrap();
        let scoped_gate = &work.steps[1];
        assert_eq!(scoped_gate.kind, stepyard_harness::StepKind::Gate);
        assert_eq!(scoped_gate.on_pass.as_deref(), Some("break"));
    }

    #[test]
    fn container_requires_scope_field() {
        for kind in ["call", "repeat", "map"] {
            let yaml = format!(
                r#"
name: no-scope-{kind}
steps:
  - name: c
    type: {kind}
{extra}
"#,
                extra = if kind == "map" {
                    "    items: \"a,b\""
                } else {
                    ""
                }
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let err = adapt(&def).unwrap_err();
            assert!(
                matches!(err, AdapterError::ContainerMissingScope { kind: k, .. } if k == kind),
                "kind={kind} got {err:?}"
            );
        }
    }

    #[test]
    fn container_with_unknown_scope_is_rejected() {
        let yaml = r#"
name: unknown-scope
steps:
  - name: c
    type: call
    scope: ghost
scopes:
  real:
    steps:
      - name: s
        type: cmd
        run: "true"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::UnknownScope { kind: "call", ref scope, .. } if scope == "ghost"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn map_without_items_is_rejected() {
        let yaml = r#"
name: map-no-items
steps:
  - name: fan
    type: map
    scope: body
scopes:
  body:
    steps:
      - name: s
        type: cmd
        run: "true"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(err, AdapterError::MapMissingItems { ref name } if name == "fan"),
            "got {err:?}"
        );
    }

    #[test]
    fn nested_container_inside_scope_is_rejected() {
        for nested in ["call", "repeat", "map"] {
            let yaml = format!(
                r#"
name: nested-{nested}
steps:
  - name: outer
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: inner
        type: {nested}
        scope: other
{extra}
  other:
    steps:
      - name: s
        type: cmd
        run: "true"
"#,
                extra = if nested == "map" {
                    "        items: \"a\""
                } else {
                    ""
                }
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let err = adapt(&def).unwrap_err();
            assert!(
                matches!(
                    err,
                    AdapterError::NestedScopesNotSupported { kind: k, .. } if k == nested
                ),
                "nested={nested} got {err:?}"
            );
        }
    }

    #[test]
    fn template_and_script_inside_scope_are_rejected_until_scoped_semantics_land() {
        for (kind, body) in [
            ("template", "        prompt: greet"),
            ("script", "        run: \"40 + 2\""),
        ] {
            let yaml = format!(
                r#"
name: scoped-{kind}
steps:
  - name: outer
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: inner
        type: {kind}
{body}
"#
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let err = adapt(&def).unwrap_err();
            assert!(
                matches!(
                    err,
                    AdapterError::ScopedStepUnsupported {
                        ref step_type,
                        ..
                    } if step_type.to_string() == kind
                ),
                "kind={kind} got {err:?}"
            );
        }
    }

    #[test]
    fn scoped_gate_accepts_skip_and_break() {
        for (field, value) in [
            ("on_pass", "skip"),
            ("on_pass", "break"),
            ("on_fail", "skip"),
            ("on_fail", "break"),
        ] {
            let yaml = format!(
                r#"
name: scoped-gate-{value}-{field}
steps:
  - name: run
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: g
        type: gate
        condition: "true"
        {field}: {value}
"#
            );
            let file = write_tmp(&yaml);
            let def = parser::parse_file(file.path()).unwrap();
            let wf = adapt(&def).expect("scoped gate with skip/break should adapt");
            let scope = wf.scopes.get("body").unwrap();
            let g = &scope.steps[0];
            match field {
                "on_pass" => assert_eq!(g.on_pass.as_deref(), Some(value)),
                "on_fail" => assert_eq!(g.on_fail.as_deref(), Some(value)),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn accepts_template_step_with_prompt_and_threads_prompts_dir() {
        // PR 4 of #31: `template` adapts, and `prompts_dir:` at workflow
        // level threads through to `Workflow.prompts_dir` so the harness
        // reads the same directory v1 did.
        let yaml = r#"
name: adapter-template
prompts_dir: ./custom-prompts
steps:
  - name: greet
    type: template
    prompt: "fix-lint/{{ vars.stack }}"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        assert_eq!(wf.prompts_dir.as_deref(), Some("./custom-prompts"));
        let step = &wf.steps[0];
        assert_eq!(step.kind, stepyard_harness::StepKind::Template);
        assert_eq!(step.prompt.as_deref(), Some("fix-lint/{{ vars.stack }}"));
    }

    #[test]
    fn template_without_prompt_falls_back_to_step_name() {
        // v1 parity (`src/steps/template_step.rs`): a template step may
        // omit the `prompt:` field; the harness then uses `step.name` as
        // the file basename.
        let yaml = r#"
name: adapter-template-no-prompt
steps:
  - name: bare
    type: template
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.kind, stepyard_harness::StepKind::Template);
        assert!(step.prompt.is_none());
        // `prompts_dir` absent in YAML → harness falls back to its
        // `DEFAULT_PROMPTS_DIR` ("prompts"); adapter leaves it `None`.
        assert!(wf.prompts_dir.is_none());
    }

    #[test]
    fn accepts_script_step_with_run_source() {
        // PR 4 of #31 commit 2: `script` adapts, reusing the YAML `run:`
        // field as the Rhai source (v1 parity with `src/steps/script.rs`).
        let yaml = r#"
name: adapter-script
steps:
  - name: compute
    type: script
    run: "40 + 2"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        let step = &wf.steps[0];
        assert_eq!(step.kind, stepyard_harness::StepKind::Script);
        assert_eq!(step.command, "40 + 2");
    }

    #[test]
    fn rejects_script_without_run_source() {
        let yaml = r#"
name: adapter-script-no-run
steps:
  - name: naked
    type: script
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(err, AdapterError::ScriptMissingRun { ref name } if name == "naked"),
            "got {err:?}"
        );
    }

    #[test]
    fn scoped_gate_rejects_unknown_action() {
        let yaml = r#"
name: scoped-gate-garbage
steps:
  - name: run
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: g
        type: gate
        condition: "true"
        on_pass: explode
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ScopedGateUnsupportedAction {
                    action: "on_pass",
                    ref value,
                    ..
                } if value == "explode"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn accepts_agent_step_translating_config_bag_to_typed_fields() {
        let yaml = r#"
name: adapter-agent
steps:
  - name: plan
    type: agent
    prompt: "Summarize {{ target }}"
    config:
      model: claude-sonnet-4-6
      system_prompt_append: "Be concise."
      permissions: skip
      session: isolated
  - name: refine
    type: agent
    prompt: "Continue"
    config:
      resume: plan
  - name: branch
    type: agent
    prompt: "Alt"
    config:
      fork_session: plan
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();

        assert_eq!(wf.steps.len(), 3);

        let plan = &wf.steps[0];
        assert_eq!(plan.kind, stepyard_harness::StepKind::Agent);
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
    fn agent_defaults_to_no_typed_fields_when_config_bag_empty() {
        // Smallest valid agent: just prompt. The six typed fields stay
        // absent so the harness's executor falls back to its defaults.
        let yaml = r#"
name: agent-bare
steps:
  - name: ask
    type: agent
    prompt: "hi"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        let ask = &wf.steps[0];
        assert_eq!(ask.kind, stepyard_harness::StepKind::Agent);
        assert!(ask.model.is_none());
        assert!(ask.system_prompt_append.is_none());
        assert!(ask.permissions.is_none());
        assert!(ask.resume.is_none());
        assert!(ask.fork_session.is_none());
        assert!(ask.agent_session.is_none());
        assert!(ask.agent_command.is_none());
    }

    #[test]
    fn agent_translates_config_command_to_typed_field() {
        // v1 parity: `config.command` carries the CLI binary path
        // (default "claude", but operators override with wrappers,
        // path-pinned binaries, corporate scripts, or mocks). v2
        // surfaces it as `step.agent_command`.
        let yaml = r#"
name: agent-with-command
steps:
  - name: ask
    type: agent
    prompt: "Hi"
    config:
      command: "/usr/local/bin/claude"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let wf = adapt(&def).unwrap();
        assert_eq!(
            wf.steps[0].agent_command.as_deref(),
            Some("/usr/local/bin/claude")
        );
    }

    #[test]
    fn agent_rejects_non_string_config_command_value() {
        // Mirrors `agent_rejects_non_string_config_value` for `model`:
        // the same tightening applies to `command` so a misspelled
        // YAML shape (`command: 42`) fails loudly instead of silently
        // falling back to "claude" at spawn time.
        let yaml = r#"
name: agent-bad-command
steps:
  - name: weirdbin
    type: agent
    prompt: "hi"
    config:
      command: 42
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::AgentConfigNotString { ref name, key: "command" }
                    if name == "weirdbin"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn agent_rejects_missing_prompt() {
        let yaml = r#"
name: agent-no-prompt
steps:
  - name: naked
    type: agent
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(err, AdapterError::AgentMissingPrompt { ref name } if name == "naked"),
            "got {err:?}"
        );
    }

    #[test]
    fn agent_rejects_blank_prompt() {
        let yaml = r#"
name: agent-blank-prompt
steps:
  - name: hollow
    type: agent
    prompt: "   "
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(err, AdapterError::AgentMissingPrompt { ref name } if name == "hollow"),
            "got {err:?}"
        );
    }

    #[test]
    fn agent_rejects_resume_and_fork_simultaneously() {
        let yaml = r#"
name: agent-conflict
steps:
  - name: first
    type: agent
    prompt: "seed"
  - name: two
    type: agent
    prompt: "both"
    config:
      resume: first
      fork_session: first
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::AgentResumeAndForkConflict { ref name } if name == "two"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn agent_rejects_invalid_permissions_value() {
        let yaml = r#"
name: agent-bad-perms
steps:
  - name: weird
    type: agent
    prompt: "hi"
    config:
      permissions: superuser
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::AgentInvalidPermissions { ref name, ref value }
                    if name == "weird" && value == "superuser"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn agent_rejects_invalid_session_value() {
        let yaml = r#"
name: agent-bad-session
steps:
  - name: weird
    type: agent
    prompt: "hi"
    config:
      session: sticky
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::AgentInvalidSession { ref name, ref value }
                    if name == "weird" && value == "sticky"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn agent_rejects_non_string_config_value() {
        // v2 tightens v1's silent `None` fallback on non-string config
        // entries — misspelled YAML shapes surface as adapter errors.
        let yaml = r#"
name: agent-non-string-config
steps:
  - name: wonky
    type: agent
    prompt: "hi"
    config:
      model: 42
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::AgentConfigNotString { ref name, key: "model" }
                    if name == "wonky"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn agent_rejected_inside_scope_body_for_now() {
        // Scoped agent dispatch lands in a follow-up; for PR 5a we
        // restrict to top-level so the engine's session-map replay
        // matches the top-level-only semantics asserted by the
        // progress scan's `scoped_completion_with_agent_session_id_is_ignored`.
        let yaml = r#"
name: agent-in-scope
steps:
  - name: run
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: inner
        type: agent
        prompt: "hi"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ScopedStepUnsupported { ref inner_name, .. }
                    if inner_name == "inner"
            ),
            "got {err:?}"
        );
    }

    // ------------------------------------------------------------------
    // parse_chat_step helper — wired into adapt_step in PR 5c commit 3.
    // The first test below pins the public dispatch arm; the rest
    // exercise the helper directly so the parsing contract stays
    // frozen as new knobs land.
    // ------------------------------------------------------------------

    /// Returns the first step from a single-step chat YAML fixture.
    /// Used by helper tests that exercise `parse_chat_step` directly
    /// rather than the full `adapt(&def)` path.
    fn chat_step_def(yaml: &str) -> StepDef {
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        def.steps.into_iter().next().unwrap()
    }

    #[test]
    fn chat_kind_accepted_by_adapt_step() {
        let yaml = r#"
name: chat-accepted
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: openai
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let plan = adapt(&def).unwrap();
        let step = &plan.steps[0];
        assert_eq!(step.name, "ask");
        assert_eq!(step.kind, stepyard_harness::StepKind::Chat);
    }

    #[test]
    fn chat_rejected_inside_scope_body_for_now() {
        // Mirrors `agent_rejected_inside_scope_body_for_now`: scoped
        // chat dispatch is deferred because the engine's chat-session
        // map only commits on a non-scoped chat completion (see
        // `engine.rs:1837`). Surfacing chat in a scope body here would
        // stage turns the progress scan never drains, so the adapter
        // pins top-level-only semantics with `ScopedStepUnsupported`.
        let yaml = r#"
name: chat-in-scope
steps:
  - name: run
    type: call
    scope: body
scopes:
  body:
    steps:
      - name: inner
        type: chat
        prompt: "hi"
"#;
        let file = write_tmp(yaml);
        let def = parser::parse_file(file.path()).unwrap();
        let err = adapt(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ScopedStepUnsupported { ref inner_name, .. }
                    if inner_name == "inner"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_chat_step_accepts_full_knob_set() {
        let yaml = r#"
name: chat-full
steps:
  - name: ask
    type: chat
    prompt: "summarize"
    env:
      OPENAI_API_KEY: shh
    config:
      provider: openai
      model: gpt-4o-mini
      max_tokens: 1024
      temperature: 0.7
      api_key_env: OPENAI_API_KEY
      base_url: "https://api.openai.com/v1"
      session: assistant
      truncation_strategy: sliding_window
      truncation_max_tokens: 8000
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();

        assert_eq!(step.name, "ask");
        assert_eq!(step.kind, stepyard_harness::StepKind::Chat);
        assert_eq!(step.prompt.as_deref(), Some("summarize"));
        assert_eq!(step.chat_provider, Some(ChatProvider::OpenAi));
        assert_eq!(step.model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(step.max_tokens, Some(1024));
        assert_eq!(step.temperature, Some(0.7));
        assert_eq!(step.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(step.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(step.chat_session.as_deref(), Some("assistant"));
        assert_eq!(
            step.truncation,
            Some(ChatTruncation::SlidingWindow { max_tokens: 8000 })
        );
        assert_eq!(
            step.env.get("OPENAI_API_KEY").map(String::as_str),
            Some("shh")
        );
    }

    #[test]
    fn parse_chat_step_defaults_provider_to_anthropic_when_absent() {
        // Checklist #2: absent `provider:` must promote to an explicit
        // ChatProvider::Anthropic at the adapter boundary so the
        // runtime never has to guess the default.
        let yaml = r#"
name: chat-default-provider
steps:
  - name: ask
    type: chat
    prompt: "hello"
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.chat_provider, Some(ChatProvider::Anthropic));
    }

    #[test]
    fn parse_chat_step_rejects_missing_prompt() {
        let yaml = r#"
name: chat-no-prompt
steps:
  - name: hollow
    type: chat
    config:
      provider: anthropic
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(err, AdapterError::ChatMissingPrompt { ref name } if name == "hollow"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_chat_step_rejects_blank_prompt() {
        let yaml = r#"
name: chat-blank-prompt
steps:
  - name: blank
    type: chat
    prompt: "   "
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(err, AdapterError::ChatMissingPrompt { ref name } if name == "blank"),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_chat_step_maps_google_to_gemini() {
        // Checklist #4: `google` alias mirrors v1's match arm at
        // `src/steps/chat.rs:346` and maps to ChatProvider::Gemini.
        let yaml = r#"
name: chat-google
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: google
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.chat_provider, Some(ChatProvider::Gemini));
    }

    #[test]
    fn parse_chat_step_maps_grok_to_xai() {
        // Checklist #4: `grok` alias mirrors v1's match arm and maps
        // to ChatProvider::Xai.
        let yaml = r#"
name: chat-grok
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: grok
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.chat_provider, Some(ChatProvider::Xai));
    }

    #[test]
    fn parse_chat_step_unknown_provider_with_base_url_becomes_openai_compatible() {
        // Checklist #3 (positive): v1's escape hatch — unknown provider
        // + explicit base_url is treated as an OpenAI-compatible endpoint.
        let yaml = r#"
name: chat-unknown-with-base-url
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: vllm-selfhost
      base_url: "http://localhost:8000/v1"
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.chat_provider, Some(ChatProvider::OpenAiCompatible));
        assert_eq!(step.base_url.as_deref(), Some("http://localhost:8000/v1"));
    }

    #[test]
    fn parse_chat_step_unknown_provider_without_base_url_errors() {
        // Checklist #3 (negative): no base_url means the operator typoed
        // a provider name — surface it instead of silently accepting.
        let yaml = r#"
name: chat-unknown-no-base-url
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: vllm-selfhost
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ChatUnknownProvider { ref name, ref value }
                    if name == "ask" && value == "vllm-selfhost"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_chat_step_openai_compatible_requires_base_url() {
        let yaml = r#"
name: chat-openai-compatible-no-url
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: openai_compatible
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ChatOpenAiCompatibleMissingBaseUrl { ref name }
                    if name == "ask"
            ),
            "got {err:?}"
        );
        // The error message has to literally name `base_url:` so the
        // operator knows the escape hatch (advisor lock-in).
        assert!(
            err.to_string().contains("base_url:"),
            "missing `base_url:` literal: {err}"
        );
    }

    #[test]
    fn parse_chat_step_rejects_non_string_provider() {
        let yaml = r#"
name: chat-non-string-provider
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: 42
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ChatConfigNotString { ref name, key: "provider" }
                    if name == "ask"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_chat_step_rejects_non_integer_max_tokens() {
        let yaml = r#"
name: chat-non-int-max-tokens
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      max_tokens: "1024"
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ChatConfigNotU64 { ref name, key: "max_tokens" }
                    if name == "ask"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_chat_step_rejects_non_numeric_temperature() {
        let yaml = r#"
name: chat-non-numeric-temp
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      temperature: "hot"
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ChatConfigNotF64 { ref name, key: "temperature" }
                    if name == "ask"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_chat_step_accepts_integer_temperature() {
        // v1 parity: `temperature: 0` must round-trip (float coercion).
        let yaml = r#"
name: chat-int-temp
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      temperature: 0
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.temperature, Some(0.0));
    }

    #[test]
    fn parse_chat_step_truncation_none_normalizes_to_none() {
        // Checklist #5: explicit `truncation_strategy: none` must
        // normalize to `Option<ChatTruncation>::None` at the adapter
        // boundary — the enum never carries a `None` variant.
        let yaml = r#"
name: chat-trunc-none
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: none
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.truncation, None);
    }

    #[test]
    fn parse_chat_step_truncation_absent_is_none() {
        let yaml = r#"
name: chat-trunc-absent
steps:
  - name: ask
    type: chat
    prompt: "hi"
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.truncation, None);
    }

    #[test]
    fn parse_chat_step_truncation_variants_roundtrip_with_v1_defaults() {
        // Table-driven cover of the four enum variants — once with
        // overrides to confirm the flat `truncation_*` keys are wired
        // to the right enum fields, once with defaults to lock v1
        // parity at `src/steps/chat.rs:34-57` (10, 10, 2+5, 50_000).
        struct Case {
            yaml: &'static str,
            expected: ChatTruncation,
        }

        let cases = [
            // Explicit overrides
            Case {
                yaml: r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: last
      truncation_count: 3
"#,
                expected: ChatTruncation::Last { count: 3 },
            },
            Case {
                yaml: r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: first
      truncation_count: 7
"#,
                expected: ChatTruncation::First { count: 7 },
            },
            Case {
                yaml: r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: first_last
      truncation_first: 1
      truncation_last: 2
"#,
                expected: ChatTruncation::FirstLast { first: 1, last: 2 },
            },
            Case {
                yaml: r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: sliding_window
      truncation_max_tokens: 8000
"#,
                expected: ChatTruncation::SlidingWindow { max_tokens: 8000 },
            },
            // v1 defaults when count/first/last/max_tokens are absent
            Case {
                yaml: r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: last
"#,
                expected: ChatTruncation::Last { count: 10 },
            },
            Case {
                yaml: r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: first
"#,
                expected: ChatTruncation::First { count: 10 },
            },
            Case {
                yaml: r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: first_last
"#,
                expected: ChatTruncation::FirstLast { first: 2, last: 5 },
            },
            Case {
                yaml: r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: sliding_window
"#,
                expected: ChatTruncation::SlidingWindow { max_tokens: 50_000 },
            },
        ];

        for (idx, case) in cases.iter().enumerate() {
            let def = chat_step_def(case.yaml);
            let step = parse_chat_step(&def).unwrap_or_else(|e| panic!("case {idx}: {e}"));
            assert_eq!(step.truncation, Some(case.expected.clone()), "case {idx}");
        }
    }

    #[test]
    fn parse_chat_step_rejects_invalid_truncation_strategy() {
        let yaml = r#"
name: chat-bad-trunc
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      truncation_strategy: weird
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ChatInvalidTruncationStrategy { ref name, ref value }
                    if name == "ask" && value == "weird"
            ),
            "got {err:?}"
        );
    }

    // ------------------------------------------------------------------
    // Default-cascade tests (model, api_key_env, max_tokens, temperature,
    // timeout). The adapter cristallizes v1's "missing = default" behavior
    // into explicit typed values so the runtime receives a fully resolved
    // Step. Each test pins one default so a drift surfaces as a targeted
    // failure instead of a generic "step doesn't match" diff.
    // ------------------------------------------------------------------

    /// Builds a minimal chat StepDef with an optional `config:` provider
    /// line. Used by the default-per-provider tables so every case stays
    /// a one-line YAML patch.
    fn chat_step_def_with_provider(provider: Option<&str>) -> StepDef {
        let config = match provider {
            Some(p) => format!("    config:\n      provider: {p}\n"),
            None => String::new(),
        };
        let yaml = format!(
            r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
{config}"#
        );
        chat_step_def(&yaml)
    }

    #[test]
    fn parse_chat_step_model_default_per_provider() {
        // Pins the v1 fallback at `src/steps/chat.rs:341-348`. Providers
        // v1 didn't enumerate (anthropic, cohere, perplexity, xai,
        // mistral, openai_compatible) share the catch-all
        // `"claude-3-haiku-20240307"` — the v1 quirk the adapter
        // preserves.
        let cases = [
            (Some("anthropic"), "claude-3-haiku-20240307"),
            (Some("openai"), "gpt-4o-mini"),
            (Some("ollama"), "llama3.2"),
            (Some("groq"), "llama-3.3-70b-versatile"),
            (Some("deepseek"), "deepseek-chat"),
            (Some("gemini"), "gemini-2.0-flash"),
            (Some("google"), "gemini-2.0-flash"),
            (Some("cohere"), "claude-3-haiku-20240307"),
            (Some("perplexity"), "claude-3-haiku-20240307"),
            (Some("xai"), "claude-3-haiku-20240307"),
            (Some("grok"), "claude-3-haiku-20240307"),
            (Some("mistral"), "claude-3-haiku-20240307"),
            (None, "claude-3-haiku-20240307"), // absent → anthropic → haiku
        ];
        for (provider, expected_model) in cases {
            let def = chat_step_def_with_provider(provider);
            let step = parse_chat_step(&def).unwrap();
            assert_eq!(
                step.model.as_deref(),
                Some(expected_model),
                "provider={provider:?}"
            );
        }
    }

    #[test]
    fn parse_chat_step_api_key_env_default_per_provider() {
        // Pins the v1 fallback at `src/steps/chat.rs:363-373`. Ollama is
        // the sole `None` case (v1 skipped the lookup entirely for local
        // endpoints). OpenAI-compatible mirrors v1's catch-all
        // `ANTHROPIC_API_KEY` — operators targeting a non-Anthropic
        // gateway must set `api_key_env:` explicitly.
        let cases = [
            (Some("anthropic"), Some("ANTHROPIC_API_KEY")),
            (Some("openai"), Some("OPENAI_API_KEY")),
            (Some("ollama"), None),
            (Some("groq"), Some("GROQ_API_KEY")),
            (Some("deepseek"), Some("DEEPSEEK_API_KEY")),
            (Some("gemini"), Some("GEMINI_API_KEY")),
            (Some("google"), Some("GEMINI_API_KEY")),
            (Some("cohere"), Some("COHERE_API_KEY")),
            (Some("perplexity"), Some("PERPLEXITY_API_KEY")),
            (Some("xai"), Some("XAI_API_KEY")),
            (Some("grok"), Some("XAI_API_KEY")),
            (Some("mistral"), Some("MISTRAL_API_KEY")),
            (None, Some("ANTHROPIC_API_KEY")),
        ];
        for (provider, expected_env) in cases {
            let def = chat_step_def_with_provider(provider);
            let step = parse_chat_step(&def).unwrap();
            assert_eq!(
                step.api_key_env.as_deref(),
                expected_env,
                "provider={provider:?}"
            );
        }
    }

    #[test]
    fn parse_chat_step_ollama_has_no_default_api_key_env() {
        // Duplicates one case from the table above, but on its own so
        // "Ollama is special" is greppable from the test name when
        // someone debugs "why is my env var None here?".
        let def = chat_step_def_with_provider(Some("ollama"));
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.chat_provider, Some(ChatProvider::Ollama));
        assert_eq!(step.api_key_env, None);
    }

    #[test]
    fn parse_chat_step_openai_compatible_default_api_key_env_is_anthropic() {
        // v1 parity quirk: openai_compatible falls into `_ =>
        // ANTHROPIC_API_KEY` at `src/steps/chat.rs:372`. The adapter
        // preserves that so pre-existing workflows don't behave
        // differently on v2. Operators targeting a real gateway are
        // expected to set `api_key_env:` explicitly.
        let yaml = r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: openai_compatible
      base_url: "https://gateway.internal/v1"
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.chat_provider, Some(ChatProvider::OpenAiCompatible));
        assert_eq!(step.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn parse_chat_step_max_tokens_defaults_to_1024() {
        // v1 default at `src/steps/chat.rs:349`.
        let def = chat_step_def_with_provider(None);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.max_tokens, Some(1024));
    }

    #[test]
    fn parse_chat_step_temperature_defaults_to_zero() {
        // v1 default at `src/steps/chat.rs:350-354`.
        let def = chat_step_def_with_provider(None);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.temperature, Some(0.0));
    }

    #[test]
    fn parse_chat_step_timeout_defaults_to_120_seconds() {
        // v1 default at `src/steps/chat.rs:355-357` (120s). The adapter
        // stores it as a `Duration` after Round 3 Story 1 — the harness
        // never re-resolves the unit.
        let def = chat_step_def_with_provider(None);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.timeout, Some(Duration::from_secs(120)));
    }

    #[test]
    fn parse_chat_step_timeout_parses_v1_duration_formats() {
        // Table-driven cover of the v1 duration shapes
        // (`src/config/mod.rs:42-57`): `ms` / `s` / `m` suffixes plus
        // bare numeric strings (treated as seconds).
        let cases = [
            ("60000ms", 60_000),
            ("120s", 120_000),
            ("2m", 120_000),
            ("120", 120_000), // bare number → seconds
            ("0ms", 0),
            ("1m", 60_000),
        ];
        for (raw, expected_ms) in cases {
            let yaml = format!(
                r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      timeout: "{raw}"
"#
            );
            let def = chat_step_def(&yaml);
            let step = parse_chat_step(&def).unwrap();
            assert_eq!(
                step.timeout,
                Some(Duration::from_millis(expected_ms)),
                "raw={raw}"
            );
        }
    }

    #[test]
    fn parse_chat_step_timeout_rejects_malformed_string() {
        // Tightens the v1 quirk where a bad `timeout:` silently fell
        // back to the 120s default — operators learn about the typo at
        // load time instead.
        let yaml = r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      timeout: "zzz"
"#;
        let def = chat_step_def(yaml);
        let err = parse_chat_step(&def).unwrap_err();
        assert!(
            matches!(
                err,
                AdapterError::ChatInvalidTimeout { ref name, ref value }
                    if name == "ask" && value == "zzz"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn parse_chat_step_explicit_values_override_defaults() {
        // Sanity: explicit values in `config:` aren't clobbered by the
        // default cascade. Guards against a future refactor accidentally
        // reordering the `.unwrap_or(default)` against `explicit`.
        let yaml = r#"
name: t
steps:
  - name: ask
    type: chat
    prompt: "hi"
    config:
      provider: openai
      model: gpt-4-turbo
      api_key_env: CUSTOM_KEY
      max_tokens: 4096
      temperature: 0.9
      timeout: "30s"
"#;
        let def = chat_step_def(yaml);
        let step = parse_chat_step(&def).unwrap();
        assert_eq!(step.chat_provider, Some(ChatProvider::OpenAi));
        assert_eq!(step.model.as_deref(), Some("gpt-4-turbo"));
        assert_eq!(step.api_key_env.as_deref(), Some("CUSTOM_KEY"));
        assert_eq!(step.max_tokens, Some(4096));
        assert_eq!(step.temperature, Some(0.9));
        assert_eq!(step.timeout, Some(Duration::from_millis(30_000)));
    }
}
