---
stepsCompleted: [1, 2, 3, 4, 5, 6, 7, 8]
inputDocuments:
  - _bmad-output/planning-artifacts/prd.md
  - _bmad-output/planning-artifacts/epics.md
  - _bmad-output/project-context.md
  - docs/index.md
  - docs/architecture.md
  - docs/project-overview.md
  - docs/component-inventory.md
  - docs/development-guide.md
  - docs/CONFIG.md
  - docs/DOCKER-SANDBOX.md
  - docs/STEP-TYPES.md
  - docs/YAML-SPEC.md
  - docs/EXAMPLES.md
  - features.md
  - .hive/summary.md
  - ARCHITECTURE-MINION-ENGINE.md
workflowType: 'architecture'
lastStep: 8
status: 'complete'
completedAt: '2026-03-13'
project_name: 'minion-engine'
user_name: 'Bruno'
date: '2026-03-13'
---

# Architecture Decision Document — Minion Engine

**Author:** Bruno
**Date:** 2026-03-13
**Version:** 0.2.1 → 0.3.0
**Scope:** Epic 11 (Prompt Registry) + Documentation Audit Gaps

---

## Project Context Analysis

### Requirements Overview

**Functional Requirements:**

This is a **brownfield audit** — the core engine (Features 1-26) is implemented and stable. The architecture decisions focus on:

1. **Epic 11 — Prompt Registry (Features 27-40):** 14 new features implementing stack-aware prompt resolution with language-specific templates and fallback chains. This is the only remaining code epic.
2. **Documentation Parity (FR-DOC-01 through FR-DOC-08):** 7 undocumented features need documentation — no code changes.
3. **Missing Guides (FR-GUIDE-01 through FR-GUIDE-04):** 4 new documentation files — no code changes.

**Non-Functional Requirements:**

| NFR | Architectural Impact |
|-----|---------------------|
| NFR-SEC-01: Sandbox security model documented | Documentation only |
| NFR-PERF-01: Default timeouts documented per step | Documentation only |
| NFR-TEST-01: Coverage 58% → 70% | New test files for Prompt Registry satisfy this |
| NFR-CON-01: Binary name consistency | Documentation only |

**Scale & Complexity:**

- Primary domain: CLI Tool + Library (Rust)
- Complexity level: Medium-High (existing codebase: 41 files, 9,504 LOC)
- New code scope: ~6 new source files, ~1 modified file, ~15 template files, ~2 workflow files
- Estimated new LOC: ~800-1,200 Rust + templates

### Technical Constraints & Dependencies

| Constraint | Detail |
|-----------|--------|
| Rust Edition 2021 | All new code must use edition 2021 |
| tokio 1.x async runtime | All I/O must be async via tokio |
| Tera 1.x template engine | Templates use `.md.tera` extension with custom preprocessing |
| serde_yaml 0.9 | Registry YAML parsing uses serde_yaml 0.9 (NOT 0.8) |
| StepExecutor trait | Any new executor must implement the existing trait pattern |
| 4-layer config merge | New config fields must participate in global → type → pattern → step merge |
| Backward compatibility | All existing workflows must continue working unchanged |

### Cross-Cutting Concerns Identified

1. **Template preprocessing pipeline** — `?`, `!`, `from()` preprocessing happens BEFORE Tera rendering. New `{{ prompts.* }}` and `{{ stack.* }}` variables must integrate at the correct stage.
2. **Context hierarchy** — Stack variables and resolved prompts must be available at the right context scope level.
3. **Error propagation** — New StepError scenarios (missing registry, undetected stack, missing prompt) must use existing error variants.
4. **Testing pattern** — Inline `#[cfg(test)]` modules with `tokio::test`, `tempfile`, no external test framework.

---

## Starter Template Evaluation

### Primary Technology Domain

**Existing Rust CLI project** — no starter template needed. The project is already published on crates.io as v0.2.1 with a mature build system.

### Technology Stack (Already Established)

| Layer | Technology | Version | Status |
|-------|-----------|---------|--------|
| Language | Rust | Edition 2021 | Established |
| Runtime | tokio | 1.x (full) | Established |
| CLI | clap | 4.x (derive) | Established |
| Templates | Tera | 1.x | Established |
| Serialization | serde + serde_yaml | 1.x / 0.9 | Established |
| HTTP | reqwest | 0.12 | Established |
| Scripting | Rhai | 1.x | Established |
| Plugins | libloading | 0.8 | Established |
| Error handling | thiserror 2.x + anyhow 1.x | Established |
| Testing | built-in + wiremock + tempfile | Established |

### New Dependencies Required

None. The Prompt Registry uses only existing dependencies:
- `serde_yaml` 0.9 for `registry.yaml` parsing
- `serde` derives for `Registry`, `StackDef`, `StackInfo` structs
- `tokio::fs` for async file reading
- `regex` for `content_match` patterns in stack detection
- `tera` for template rendering (already in engine)

---

## Core Architectural Decisions

### Decision Priority Analysis

**Critical Decisions (Block Implementation):**

1. Prompt Registry data model and YAML schema
2. Fallback chain resolution algorithm
3. Integration point in Engine initialization
4. Context injection strategy for `{{ stack.* }}` and `{{ prompts.* }}`

**Important Decisions (Shape Architecture):**

5. `TemplateStepExecutor` dynamic path mechanism
6. Stack detection file marker vs content match priority
7. Error handling for missing prompts (fail vs fallback)
8. Pre-flight validation strategy

**Deferred Decisions (Post-MVP):**

9. Prompt template caching strategy
10. Custom stack registration by users (beyond registry.yaml)
11. Remote prompt template repositories

### ADR-01: Prompt Registry Data Model

**Decision:** Declarative YAML manifest at `prompts/registry.yaml`

**Schema:**

```yaml
version: 1
detection_order:
  - java-spring    # most specific first
  - java
  - react
  - typescript
  - javascript
  - python
  - rust

stacks:
  _default:
    tools:
      lint: "echo 'no linter configured'"
      test: "echo 'no test runner configured'"
      build: "echo 'no build configured'"
      install: "echo 'no installer configured'"

  java:
    parent: _default
    file_markers: ["pom.xml", "build.gradle", "build.gradle.kts"]
    tools:
      lint: "mvn checkstyle:check"
      test: "mvn test"
      build: "mvn package -DskipTests"
      install: "mvn dependency:resolve"

  java-spring:
    parent: java
    file_markers: ["pom.xml", "build.gradle"]
    content_match:
      "pom.xml": "spring-boot"
      "build.gradle": "org.springframework.boot"
    tools:
      test: "mvn test -Dspring.profiles.active=test"

  javascript:
    parent: _default
    file_markers: ["package.json"]
    tools:
      lint: "npx eslint ."
      test: "npm test"
      build: "npm run build"
      install: "npm ci"

  typescript:
    parent: javascript
    file_markers: ["tsconfig.json"]
    tools:
      lint: "npx eslint . --ext .ts,.tsx"

  react:
    parent: typescript
    content_match:
      "package.json": "react"
    tools:
      test: "npx react-scripts test --watchAll=false"

  python:
    parent: _default
    file_markers: ["pyproject.toml", "setup.py", "requirements.txt"]
    tools:
      lint: "ruff check ."
      test: "pytest"
      build: "python -m build"
      install: "pip install -r requirements.txt"

  rust:
    parent: _default
    file_markers: ["Cargo.toml"]
    tools:
      lint: "cargo clippy -- -D warnings"
      test: "cargo test"
      build: "cargo build --release"
      install: "cargo fetch"
```

**Rationale:** YAML is already the project's workflow definition language. Using serde_yaml 0.9 keeps parsing consistent. The `parent` field enables inheritance chains without duplicating tool definitions.

**Affects:** Features 27, 28, 29, 31, 32

### ADR-02: Fallback Chain Resolution Algorithm

**Decision:** Linear parent chain traversal with explicit ordering

**Algorithm:**

```
resolve(function, stack) → path:
  1. Check: prompts/{function}/{stack.name}.md.tera
  2. If not found AND stack has parent:
     → resolve(function, stack.parent)
  3. If not found AND no parent:
     → Check: prompts/{function}/_default.md.tera
  4. If still not found:
     → StepError::Fail with actionable message
```

**Example chain for `fix-lint` with `react` stack:**

```
prompts/fix-lint/react.md.tera      → exists? use it
prompts/fix-lint/typescript.md.tera → exists? use it
prompts/fix-lint/javascript.md.tera → exists? use it
prompts/fix-lint/_default.md.tera   → exists? use it
→ ERROR: "No prompt for fix-lint/react — create prompts/fix-lint/react.md.tera or prompts/fix-lint/_default.md.tera"
```

**Rationale:** Linear parent chain is simple, predictable, and debuggable. No diamond inheritance problems. The error message tells the user exactly what file to create.

**Affects:** Features 29, 32, 39

### ADR-03: Engine Integration Point

**Decision:** Stack detection runs once during `Engine::new()` when `prompts/registry.yaml` exists

**Sequence:**

```
Engine::new(workflow, options)
  ├── parse workflow YAML
  ├── initialize config manager
  ├── initialize sandbox (if enabled)
  ├── NEW: detect_stack_if_registry_exists()
  │     ├── read prompts/registry.yaml
  │     ├── StackDetector::detect(registry, workspace_path)
  │     └── store StackInfo in Engine
  └── create root context
        └── NEW: inject stack.* variables into context
```

**Rationale:** Stack detection is a workspace-level concern, not a per-step concern. Running once in Engine initialization prevents redundant filesystem scans. The detected `StackInfo` is immutable after detection.

**Affects:** Features 28, 31, 39

### ADR-04: Context Injection for Stack and Prompt Variables

**Decision:** Inject `stack.*` as context variables during root context creation. Inject `prompts.*` as lazy-resolved template functions.

**Stack variables (eager, at Engine init):**

```rust
// In Context::new() or engine initialization
ctx.set("stack.name", "react");
ctx.set("stack.parent", "typescript");
ctx.set("stack.tools.lint", "npx eslint . --ext .ts,.tsx");
ctx.set("stack.tools.test", "npx react-scripts test --watchAll=false");
ctx.set("stack.tools.build", "npm run build");
ctx.set("stack.tools.install", "npm ci");
```

**Prompt variables (resolved on access):**

`{{ prompts.fix-lint }}` resolves to the rendered content of the resolved prompt file for the detected stack. This is implemented in the template preprocessing layer (`engine/template.rs`) alongside `?`, `!`, and `from()`.

**Rationale:** Stack tools are simple string values — eager injection is efficient. Prompt content requires filesystem reads + Tera rendering, so lazy resolution avoids loading unused prompts.

**Affects:** Features 31, 32

### ADR-05: Dynamic Template Path in TemplateStepExecutor

**Decision:** When `step.prompt` is set, use it as a Tera-renderable path instead of `step.name`

**Current behavior (preserved as fallback):**

```rust
// template_step.rs line 32-33
let file_path = PathBuf::from(&self.prompts_dir)
    .join(format!("{}.md.tera", step.name));
```

**New behavior (~5 lines change):**

```rust
let template_name = if let Some(ref prompt) = step.prompt {
    // Render the prompt field as a Tera template (e.g., "fix-lint/{{ stack.name }}")
    ctx.render_template(prompt)?
} else {
    step.name.clone()
};
let file_path = PathBuf::from(&self.prompts_dir)
    .join(format!("{}.md.tera", template_name));
```

**Rationale:** Minimal change to existing code. Backward-compatible — `prompt` field is optional and already exists in `StepDef` schema. Uses existing `ctx.render_template()` for dynamic path rendering.

**Affects:** Feature 30

### ADR-06: Error Strategy for Missing Prompts

**Decision:** Use `StepError::Fail` with actionable error messages

**Error scenarios and messages:**

| Scenario | Error Message |
|----------|--------------|
| No `registry.yaml` but workflow uses `{{ stack.* }}` | "Workflow references stack variables but prompts/registry.yaml not found. Create it or remove stack references." |
| Stack not detected | "Could not detect project stack. Checked markers: [list]. Create prompts/registry.yaml with your stack definition." |
| Prompt not found after full fallback chain | "No prompt for {function}/{stack} — create prompts/{function}/{stack}.md.tera or prompts/{function}/_default.md.tera" |
| Circular parent reference | "Circular parent chain detected: {chain}. Check registry.yaml parent fields." |

**Rationale:** `StepError::Fail` is the correct variant — these are configuration errors, not control flow. Actionable messages tell the user exactly what to create/fix.

**Affects:** Features 29, 39

### ADR-07: Pre-Flight Validation

**Decision:** Extend `validate_environment()` in `cli/commands.rs` to check Prompt Registry prerequisites

**Validation checks:**

1. If workflow YAML contains `{{ stack.` or `{{ prompts.` → verify `prompts/registry.yaml` exists
2. If registry exists → verify it parses correctly
3. If stack detectable → verify referenced prompt files exist in fallback chain
4. If prompt files referenced → verify they contain valid Tera syntax

**Rationale:** Pre-flight catches misconfiguration before the workflow starts, preventing confusing mid-execution failures.

**Affects:** Feature 39

### Decision Impact Analysis

**Implementation Sequence:**

1. Feature 27: Registry YAML schema + parser (foundation — everything depends on this)
2. Feature 28: Stack Detector (depends on 27)
3. Feature 29: Prompt Resolver with fallback chain (depends on 27)
4. Feature 30: Dynamic template path (independent — can parallel with 28/29)
5. Feature 31: Stack context variables (depends on 28)
6. Feature 32: Auto-resolved prompt variables (depends on 29, 31)
7. Feature 33: Base _default prompt templates (depends on 29)
8. Features 34-36: Language-specific templates (depends on 33)
9. Feature 37: fix-ci.yaml workflow (depends on 31, 32)
10. Feature 38: fix-test.yaml workflow (depends on 37)
11. Feature 39: Pre-flight validation (depends on 28, 29)
12. Feature 40: Integration tests (depends on 29, 30)

**Cross-Component Dependencies:**

```
registry.yaml (27) ──→ StackDetector (28) ──→ Stack Context (31) ──→ fix-ci.yaml (37)
                   └──→ PromptResolver (29) ──→ Auto-Prompts (32) ──→ fix-test.yaml (38)
                                            └──→ _default templates (33) ──→ lang templates (34-36)
                   TemplateStep dynamic path (30) ── independent ──→ tests (40)
                   StackDetector (28) + PromptResolver (29) ──→ Pre-flight (39)
```

---

## Implementation Patterns & Consistency Rules

### Naming Patterns

**New Module Naming:**

```
src/prompts/mod.rs          # pub mod registry; pub mod detector; pub mod resolver;
src/prompts/registry.rs     # Registry struct, StackDef, parsing
src/prompts/detector.rs     # StackDetector struct
src/prompts/resolver.rs     # PromptResolver struct
```

**Struct Naming (follows existing PascalCase + descriptive suffix):**

| Struct | Purpose |
|--------|---------|
| `Registry` | Parsed registry.yaml data |
| `StackDef` | Single stack definition from YAML |
| `StackInfo` | Detection result: name + parent chain + tools |
| `StackDetector` | Detects stack from workspace files |
| `PromptResolver` | Resolves prompt path via fallback chain |

**Template File Naming:**

```
prompts/{function}/{stack}.md.tera
prompts/fix-lint/_default.md.tera
prompts/fix-lint/java.md.tera
prompts/fix-lint/java-spring.md.tera
prompts/fix-lint/react.md.tera
prompts/fix-test/_default.md.tera
prompts/code-review/_default.md.tera
```

### Structure Patterns

**New files follow existing patterns exactly:**

- New module directory: `src/prompts/` with `mod.rs` (matches `src/events/`, `src/plugins/`)
- Tests inline: `#[cfg(test)] mod tests { ... }` at bottom of each file
- Integration tests: `tests/prompt_resolver.rs` (one file for the epic)
- Test fixtures: `tests/fixtures/` for registry.yaml and marker files

**Serde patterns for new structs:**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Registry {
    pub version: u32,
    pub detection_order: Vec<String>,
    pub stacks: HashMap<String, StackDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_markers: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub content_match: HashMap<String, String>,
    #[serde(default)]
    pub tools: HashMap<String, String>,
}
```

### Format Patterns

**Error messages follow existing format:**

```
StepError::Fail("descriptive message: '{value}': {underlying_error}")
```

Examples:
```
StepError::Fail("Registry file not found: 'prompts/registry.yaml': No such file")
StepError::Fail("No prompt for fix-lint/react — create prompts/fix-lint/react.md.tera")
StepError::Config("Invalid registry.yaml: missing 'stacks' field")
```

**Context variable naming:**

```
{{ stack.name }}            # "react"
{{ stack.parent }}          # "typescript"
{{ stack.tools.lint }}      # "npx eslint . --ext .ts,.tsx"
{{ stack.tools.test }}      # "npx react-scripts test --watchAll=false"
{{ stack.tools.build }}     # "npm run build"
{{ stack.tools.install }}   # "npm ci"
{{ prompts.fix-lint }}      # rendered content of resolved prompt file
{{ prompts.fix-test }}      # rendered content of resolved prompt file
{{ prompts.code-review }}   # rendered content of resolved prompt file
```

### Process Patterns

**Stack Detection Process:**

1. Check if `prompts/registry.yaml` exists — if not, skip silently (backward compat)
2. Parse registry.yaml with `serde_yaml::from_str`
3. Iterate `detection_order` (most specific first)
4. For each stack: check `file_markers` (any match), then `content_match` (all must match)
5. First fully matching stack wins
6. Build `StackInfo` by walking parent chain, merging tools (child overrides parent)

**Prompt Resolution Process:**

1. Receive function name (e.g., "fix-lint") and StackInfo
2. Build candidate path: `prompts/{function}/{stack.name}.md.tera`
3. Check file existence with `tokio::fs::metadata`
4. If missing, walk parent chain repeating check
5. Final fallback: `prompts/{function}/_default.md.tera`
6. If found: read + render with Tera context
7. If not found: `StepError::Fail` with actionable message

### Enforcement Guidelines

**All AI Agents MUST:**

- Use `tokio::fs` for all file operations (never `std::fs`)
- Return `Result<_, StepError>` with semantic variants (Fail, Config), never `Other`
- Follow the derive order: `Debug, Clone, Serialize, Deserialize`
- Put tests in `#[cfg(test)] mod tests { ... }` at file bottom
- Use `crate::` imports, not `super::` except within prompts module submodules
- Implement `Send + Sync` on all new structs (satisfied by default for simple data structs)

---

## Project Structure & Boundaries

### New Files (Epic 11)

```
minion-engine/
├── src/
│   └── prompts/                    # NEW module
│       ├── mod.rs                  # pub mod registry, detector, resolver
│       ├── registry.rs             # Registry, StackDef structs + YAML parser
│       ├── detector.rs             # StackDetector — file marker + content match
│       └── resolver.rs             # PromptResolver — fallback chain resolution
│
├── prompts/                        # NEW template directory (already in .gitignore via exclude)
│   ├── registry.yaml               # Stack detection manifest
│   ├── fix-lint/
│   │   ├── _default.md.tera        # Universal fallback
│   │   ├── java.md.tera
│   │   ├── java-spring.md.tera
│   │   ├── react.md.tera
│   │   ├── typescript.md.tera
│   │   ├── python.md.tera
│   │   └── rust.md.tera
│   ├── fix-test/
│   │   ├── _default.md.tera
│   │   ├── java.md.tera
│   │   ├── java-spring.md.tera
│   │   ├── react.md.tera
│   │   ├── python.md.tera
│   │   └── rust.md.tera
│   └── code-review/
│       ├── _default.md.tera
│       ├── java.md.tera
│       ├── react.md.tera
│       ├── python.md.tera
│       └── rust.md.tera
│
├── workflows/
│   ├── fix-ci.yaml                 # NEW — generic CI fix workflow
│   └── fix-test.yaml               # NEW — generic test fix workflow
│
└── tests/
    ├── prompt_resolver.rs           # NEW — integration tests for Epic 11
    └── fixtures/
        ├── registry.yaml            # Test registry
        └── prompts/                 # Test prompt templates
```

### Modified Files (Epic 11)

| File | Change | Scope |
|------|--------|-------|
| `src/steps/template_step.rs` | Add dynamic path via `step.prompt` field (~5 lines) | Feature 30 |
| `src/engine/mod.rs` | Add stack detection in `Engine::new()` + inject stack vars (~30 lines) | Features 31, 32 |
| `src/engine/context.rs` | Add `stack.*` to context variables (~15 lines) | Feature 31 |
| `src/engine/template.rs` | Add `{{ prompts.* }}` preprocessing (~20 lines) | Feature 32 |
| `src/cli/commands.rs` | Add pre-flight validation for stack/prompt refs (~25 lines) | Feature 39 |
| `src/lib.rs` | Add `pub mod prompts;` (1 line) | Feature 27 |

### Architectural Boundaries

**Prompt Registry Module Boundary:**

```
src/prompts/ is SELF-CONTAINED:
  - Reads registry.yaml independently
  - Detects stack independently
  - Resolves prompts independently
  - Returns StackInfo and resolved paths to Engine
  - Does NOT access Engine internals, Context, or StepExecutor
```

**Integration Contract:**

```rust
// Engine calls into prompts module via these public APIs:
pub fn Registry::from_file(path: &Path) -> Result<Registry, StepError>;
pub fn StackDetector::detect(registry: &Registry, workspace: &Path) -> Result<StackInfo, StepError>;
pub fn PromptResolver::resolve(function: &str, stack: &StackInfo, prompts_dir: &Path) -> Result<PathBuf, StepError>;
```

**Data Flow:**

```
prompts/registry.yaml → Registry::from_file()
                              │
                         StackDetector::detect()
                              │
                         StackInfo { name, parent_chain, tools }
                              │
                    ┌─────────┴─────────┐
                    │                   │
              Engine Context      PromptResolver::resolve()
              {{ stack.* }}       prompts/{fn}/{stack}.md.tera
                    │                   │
              Tera rendering      Template content
                    │                   │
              Workflow steps       {{ prompts.* }}
```

### Requirements to Structure Mapping

| Feature | Primary File | Supporting Files |
|---------|-------------|-----------------|
| F27: Registry Schema | `src/prompts/registry.rs` | `prompts/registry.yaml` |
| F28: Stack Detector | `src/prompts/detector.rs` | |
| F29: Prompt Resolver | `src/prompts/resolver.rs` | |
| F30: Dynamic Template Path | `src/steps/template_step.rs` | |
| F31: Stack Context Vars | `src/engine/mod.rs`, `src/engine/context.rs` | |
| F32: Auto-Resolved Prompts | `src/engine/template.rs` | |
| F33: Default Templates | `prompts/*/_ default.md.tera` | |
| F34: Java Templates | `prompts/*/java*.md.tera` | |
| F35: React/TS Templates | `prompts/*/react.md.tera`, `prompts/*/typescript.md.tera` | |
| F36: Python/Rust Templates | `prompts/*/python.md.tera`, `prompts/*/rust.md.tera` | |
| F37: fix-ci.yaml | `workflows/fix-ci.yaml` | |
| F38: fix-test.yaml | `workflows/fix-test.yaml` | |
| F39: Pre-flight Validation | `src/cli/commands.rs` | |
| F40: Integration Tests | `tests/prompt_resolver.rs` | `tests/fixtures/` |

---

## Architecture Validation Results

### Coherence Validation

**Decision Compatibility:**

- All new code uses existing dependencies (serde_yaml, tokio::fs, tera, regex) — no new crate additions needed
- The `StackDef` struct uses standard serde patterns consistent with `WorkflowDef` and `StepDef`
- Fallback chain resolution uses `tokio::fs::metadata` for async file checks — consistent with engine's async-first pattern
- Error handling uses `StepError::Fail` and `StepError::Config` — no new error variants needed

**Pattern Consistency:**

- New `src/prompts/` module follows `src/events/` and `src/plugins/` directory pattern
- All structs use `#[derive(Debug, Clone, Serialize, Deserialize)]` — matches project convention
- Tests use inline `#[cfg(test)]` modules — consistent with 25 existing test modules
- Integration test in `tests/prompt_resolver.rs` — follows `tests/integration.rs` pattern

**Structure Alignment:**

- `prompts/registry.yaml` sits alongside existing `prompts/*.md.tera` files — natural location
- New workflow files in `workflows/` — standard location
- `src/prompts/mod.rs` registered in `src/lib.rs` — follows existing module registration

### Requirements Coverage Validation

**Epic 11 Coverage:**

| Feature | Architectural Support | Decision |
|---------|----------------------|----------|
| F27: Registry YAML | ADR-01 schema + `src/prompts/registry.rs` | Covered |
| F28: Stack Detector | ADR-03 engine integration + `src/prompts/detector.rs` | Covered |
| F29: Fallback Chain | ADR-02 algorithm + `src/prompts/resolver.rs` | Covered |
| F30: Dynamic Path | ADR-05 template_step.rs change | Covered |
| F31: Stack Context | ADR-04 context injection | Covered |
| F32: Auto Prompts | ADR-04 lazy resolution | Covered |
| F33-36: Templates | File structure defined | Covered |
| F37-38: Workflows | Uses `{{ stack.* }}` and `{{ prompts.* }}` | Covered |
| F39: Pre-flight | ADR-07 validation checks | Covered |
| F40: Tests | Test structure + fixtures defined | Covered |

**PRD Audit Gap Coverage:**

The architecture document itself closes FR-ALIGN-01 and FR-ALIGN-02 by providing the implementation design for Epic 11. Documentation-only gaps (FR-DOC-*, FR-GUIDE-*) are out of scope for architecture but should be addressed in parallel.

**Non-Functional Requirements:**

| NFR | Covered By |
|-----|-----------|
| Backward compatibility | ADR-05 fallback to `step.name` when no `prompt` field |
| Performance | Stack detection once at init, lazy prompt resolution |
| Testing | Feature 40 integration tests + inline tests per module |
| Security | Prompts are read-only templates, no code execution risk |

### Implementation Readiness Validation

**Decision Completeness:**

- All 7 ADRs have clear rationale, affected features, and code-level detail
- Schema examples, algorithm pseudocode, and Rust struct definitions provided
- Error messages specified for all failure scenarios

**Structure Completeness:**

- All 4 new source files defined with module structure
- All ~15 template files listed with directory structure
- All 2 new workflow files defined
- All modified files identified with estimated line changes

**Pattern Completeness:**

- Naming patterns for structs, files, and templates defined
- Serde attributes specified for all new structs
- Error message format consistent with existing patterns
- Context variable naming documented with examples

### Architecture Completeness Checklist

**Requirements Analysis**

- [x] Project context thoroughly analyzed (PRD audit, 42 project-context rules, features.md)
- [x] Scale and complexity assessed (6 new files, ~1,000 LOC)
- [x] Technical constraints identified (Rust edition, tokio async, serde_yaml 0.9)
- [x] Cross-cutting concerns mapped (template preprocessing, context hierarchy, error propagation)

**Architectural Decisions**

- [x] 7 ADRs documented with rationale and affected features
- [x] Technology stack fully specified (no new dependencies)
- [x] Integration patterns defined (Engine init, context injection, template preprocessing)
- [x] Error handling strategy complete with actionable messages

**Implementation Patterns**

- [x] Naming conventions established (structs, files, templates, variables)
- [x] Serde patterns defined for all new structs
- [x] Testing patterns consistent with project conventions
- [x] Process patterns for detection and resolution documented

**Project Structure**

- [x] Complete directory structure for new and modified files
- [x] Module boundaries established (prompts module is self-contained)
- [x] Integration contract defined (3 public APIs)
- [x] Requirements-to-structure mapping complete (14 features → files)

### Architecture Readiness Assessment

**Overall Status:** READY FOR IMPLEMENTATION

**Confidence Level:** High

**Key Strengths:**

- Minimal modification to existing code (~5 lines in template_step.rs, ~90 lines across engine)
- Self-contained new module (`src/prompts/`) with clear boundary
- Fully backward-compatible — no changes to existing workflows
- Declarative registry.yaml eliminates hardcoded language logic
- Fallback chain makes system gracefully extensible

**Design Principles:**

1. **Zero hardcoded languages** — All language knowledge lives in `registry.yaml` and prompt templates
2. **Graceful fallback** — Parent chain + `_default` ensures something always works
3. **Minimal engine changes** — Core engine gets stack detection at init + context injection
4. **Template-driven** — Language expertise lives in `.md.tera` files, not Rust code
5. **Backward compatible** — Existing workflows unaffected; registry.yaml is opt-in

### Implementation Handoff

**AI Agent Guidelines:**

- Follow all architectural decisions (ADR-01 through ADR-07) exactly as documented
- Use implementation patterns consistently across all new files
- Respect the prompts module boundary — it does NOT import engine internals
- Refer to this document for all architectural questions

**Parallelization Opportunities (for `/hive:worktree-bmad -4`):**

| Worktree | Features | Dependencies |
|----------|----------|-------------|
| **WT-1: Registry + Detector** | F27, F28 | None (foundation) |
| **WT-2: Resolver + Dynamic Path** | F29, F30 | None (independent of detector) |
| **WT-3: Engine Integration** | F31, F32, F39 | Depends on WT-1 + WT-2 output |
| **WT-4: Templates + Workflows + Tests** | F33-38, F40 | Depends on WT-1 + WT-2 for contract |

**Recommended split for `/hive:worktree-bmad -4 --fork --auto-approve`:**

- WT-1 and WT-2 can run in parallel (no shared files)
- WT-3 depends on WT-1 + WT-2 merging first
- WT-4 only needs the public API contract (can start with stubs)

**First Implementation Priority:** Feature 27 (Registry YAML Schema + Parser) — everything else depends on this.

---

_Architecture Decision Document Complete. This document serves as the single source of truth for all Epic 11 implementation decisions._
