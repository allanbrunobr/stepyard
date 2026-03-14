---
project_name: 'minion-engine'
user_name: 'Bruno'
date: '2026-03-13'
sections_completed:
  ['technology_stack', 'language_rules', 'framework_rules', 'testing_rules', 'quality_rules', 'workflow_rules', 'anti_patterns']
status: 'complete'
rule_count: 42
optimized_for_llm: true
---

# Project Context for AI Agents

_This file contains critical rules and patterns that AI agents must follow when implementing code in minion-engine. Focus on unobvious details that agents might otherwise miss._

---

## Technology Stack & Versions

| Technology | Version | Role |
|---|---|---|
| **Rust** | Edition 2021 | Core language |
| **tokio** | 1.x (full features) | Async runtime — ALL async code uses tokio |
| **clap** | 4.x (derive macros) | CLI framework |
| **tera** | 1.x | Template engine (Jinja2-like) |
| **serde** | 1.x | Serialization framework |
| **serde_yaml** | 0.9 | YAML workflow parsing |
| **serde_json** | 1.x | JSON handling for step outputs |
| **reqwest** | 0.12 (json feature) | HTTP client for chat API calls |
| **rhai** | 1.x | Embedded scripting engine |
| **libloading** | 0.8 | Dynamic plugin loading (C ABI) |
| **thiserror** | 2.x | Error enum derive macros |
| **anyhow** | 1.x | Error context/propagation |
| **chrono** | 0.4 (serde feature) | Timestamps |
| **colored** | 2.x | Terminal color output |
| **indicatif** | 0.17 | Progress bars |
| **regex** | 1.x | Pattern matching |
| **async-trait** | 0.1 | Async trait support |
| **futures** | 0.3 | Async utilities |

**Version constraints:** `serde_yaml` 0.9 (not 0.8) — breaking API differences. `thiserror` 2.x (not 1.x) — different derive syntax.

---

## Critical Implementation Rules

### Rust Language Rules

- **Async-first**: ALL step execution is async via tokio. Never use `std::thread::spawn` — use `tokio::spawn` or `tokio::task::spawn_blocking` for CPU-bound work.
- **Error pattern**: Use `thiserror` for defining error enums, `anyhow` for propagation in non-library code. Step executors return `Result<StepOutput, StepError>`, NOT `anyhow::Result`.
- **StepError variants**: `Fail`, `ControlFlow`, `Timeout`, `Template`, `Sandbox`, `Config`, `Other`. Use the specific variant, never default to `Other` when a semantic variant exists.
- **Arc for shared state**: Context hierarchy uses `Arc<Context>` for parent references. Sandbox uses `Arc<Mutex<DockerSandbox>>` (type alias `SharedSandbox`).
- **Import style**: Use `crate::` absolute imports, NOT relative `super::` except within the same module's submodules. Group imports: std → external crates → `crate::`.
- **String ownership**: Prefer `&str` parameters, return `String`. Use `.into()` for String conversions, not `.to_string()` on literals.
- **Derive order**: `#[derive(Debug, Clone, Serialize, Deserialize)]` — Debug first, then Clone, then serde derives.
- **serde attributes**: Use `#[serde(rename_all = "snake_case")]` on enums. Use `#[serde(skip_serializing_if = "Option::is_none")]` on optional fields.

### Architecture Rules (Engine-Agent Separation)

- **Fundamental principle**: "Engine decides what runs, Agent only works when engine commands." The engine orchestrates; step executors execute.
- **Step Executor trait pattern**: Every step type implements `StepExecutor` trait with signature `async fn execute(&self, step: &StepDef, config: &StepConfig, ctx: &Context) -> Result<StepOutput, StepError>`.
- **SandboxAwareExecutor**: Steps that can run in Docker (currently `cmd` and `agent`) ALSO implement `SandboxAwareExecutor` with `execute_sandboxed()`.
- **StepOutput wrapping**: Every step returns a specific `StepOutput` variant. Template steps return `StepOutput::Agent(AgentOutput { ... })` — NOT a custom variant.
- **Context tree**: Hierarchical parent-child scopes. Child contexts inherit parent variables. Use `ctx.render_template()` for Tera rendering. `ctx.store()` to persist step outputs.
- **4-layer config merge**: global → type-level → pattern-match → step inline. Use `StepConfig` for resolved values, never read raw `HashMap` directly.
- **Dispatch in engine/mod.rs**: Step type dispatch is in `Engine::dispatch_step()`. New step types MUST be added there AND registered in `src/steps/mod.rs`.

### Template Engine Rules

- **Tera with custom preprocessing**: The engine extends standard Tera with `?` (optional/safe access), `!` (assertion/required), and `from()` (step output injection). These are preprocessed in `engine/template.rs` BEFORE Tera rendering.
- **Template files**: Use `.md.tera` extension. Placed in `prompts/` directory (configurable via `prompts_dir` in workflow YAML).
- **Template resolution**: `TemplateStepExecutor` resolves path as `{prompts_dir}/{step.name}.md.tera`. The step name IS the filename.
- **Context variables in templates**: Access via `{{ target }}` (workflow target), `{{ steps.step_name.stdout }}`, `{{ steps.step_name.response }}`, `{{ steps.step_name.exit_code }}`.

### Workflow YAML Rules

- **Schema types**: `WorkflowDef` → `StepDef` → `StepType` enum (10 variants: Cmd, Agent, Chat, Gate, Repeat, Map, Parallel, Call, Template, Script).
- **YAML field naming**: All snake_case. Step type is `type:` field. Prompt is `prompt:` (inline string or multiline `|`).
- **Scopes**: Defined at workflow level in `scopes:` block. Referenced by `scope:` field in steps. Scopes contain their own `steps:` array.
- **Config hierarchy in YAML**: `config.global`, `config.agent`, `config.cmd`, `config.chat` — type-level configs apply to all steps of that type.
- **Output types**: `output_type:` field supports `text`, `json`, `integer`, `lines`, `boolean`. Maps to `ParsedValue` enum.

### Testing Rules

- **Inline tests**: ALL tests go in `#[cfg(test)] mod tests { ... }` at the bottom of the source file. Do NOT create separate test files per module.
- **Integration tests**: Only `tests/integration.rs` exists for cross-module integration tests. Keep integration tests there.
- **Test async**: Use `#[tokio::test]` for async tests, NOT `#[test]` with manual runtime.
- **Test helpers**: Define helper functions like `make_step()`, `make_context()` inside the test module, NOT as public utilities.
- **tempfile for filesystem tests**: Use `tempfile::tempdir()` for any test that needs filesystem access. Always clean up.
- **wiremock for HTTP tests**: Use `wiremock` crate for mocking HTTP endpoints (chat step tests).
- **Assert pattern**: Use `assert!(result.is_err())` + `unwrap_err().to_string()` for error assertions. Use descriptive assert messages.

### Code Quality & Style Rules

- **No rustfmt.toml**: Uses default Rust formatting. Run `cargo fmt` before committing.
- **No clippy config**: Uses default Clippy rules. Run `cargo clippy` before committing.
- **File naming**: All snake_case. Module files use `mod.rs` pattern (not single-file modules).
- **Struct naming**: PascalCase. Executors named `{Type}Executor` (e.g., `CmdExecutor`, `AgentExecutor`).
- **Visibility**: Use `pub(crate)` for internal-only functions. Use `pub` only for the public API surface.
- **Documentation**: Use `///` doc comments on public API items. Internal functions do NOT require doc comments unless the logic is non-obvious.
- **Module organization**: Each step type gets its own file in `src/steps/`. Each major subsystem gets a directory with `mod.rs`.

### Development Workflow Rules

- **Binary name**: The binary is `minion` (defined in `[[bin]]` section of Cargo.toml), NOT `minion-engine`.
- **Workflow files**: Stored in `workflows/` directory at project root. Named descriptively: `fix-issue.yaml`, `code-review.yaml`, etc.
- **Prompt templates**: Stored in `prompts/` directory at project root.
- **Published on crates.io**: Version bumps require `cargo publish`. Respect the `exclude` list in Cargo.toml.
- **No CI/CD config**: Manual testing and publishing workflow currently.

### Critical Don't-Miss Rules

- **NEVER add a new StepOutput variant** without updating ALL match arms in `StepOutput::text()`, `StepOutput::exit_code()`, `StepOutput::success()`, and `StepOutput::lines()`. There are 4 match blocks to update.
- **NEVER use blocking I/O** in step executors. Use `tokio::fs` not `std::fs`. Use `tokio::process::Command` not `std::process::Command`.
- **NEVER unwrap() in production code**. Use `?` operator or explicit error handling with `StepError` variants.
- **ControlFlow is NOT an error**: `ControlFlow::Skip`, `Break`, `Next` are wrapped in `StepError::ControlFlow` but they represent NORMAL flow control (gate on_pass/on_fail). Handle them in the engine dispatch loop, don't treat them as failures.
- **Sandbox auto-detection**: `DockerSandbox::auto_detect_gh_token()` runs automatically. Don't manually set GH_TOKEN in sandbox env.
- **Template preprocessing happens BEFORE Tera**: If you add new template syntax, it must be handled in `engine/template.rs::preprocess_template()`, not in Tera filters/functions.
- **StepConfig is immutable**: Resolved once by `ConfigManager::resolve()`. Don't modify it during execution.
- **Duration serialization**: `StepOutput` uses custom `serialize_duration`/`deserialize_duration` for Duration fields. Any new Duration field MUST use these same serde attributes.
- **Session IDs are optional**: `AgentOutput.session_id` and `SessionManager` support conversation continuity but are not always present. Always handle `None`.

---

## Usage Guidelines

**For AI Agents:**

- Read this file before implementing any code in minion-engine
- Follow ALL rules exactly as documented
- When in doubt, prefer the more restrictive option
- When adding a new step type: create executor file in `src/steps/`, implement `StepExecutor`, add variant to `StepType` enum, register in `Engine::dispatch_step()`

**For Humans:**

- Keep this file lean and focused on agent needs
- Update when technology stack changes
- Review quarterly for outdated rules
- Remove rules that become obvious over time

Last Updated: 2026-03-13
