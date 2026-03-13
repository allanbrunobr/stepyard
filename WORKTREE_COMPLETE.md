# Worktree Completion Reports

## wt1 — Epic 1 MVP Foundation
All 10 stories (1.1-1.10) completed.

## wt2 — Epic 2 New Step Types
All 5 stories (2.1, 2.2, 2.3, 2.4, 2.8) completed.

## wt3 — Epic 2 Cross-cutting
All 3 stories (2.5, 2.6, 2.7) completed.

## wt3 — Epic 4 Distribution
All 4 stories (4.1, 4.2, 4.3, 4.4) completed.

### Story 4.1: cargo install
- `Cargo.toml`: added full metadata (description, license MIT, repository, homepage, documentation, keywords, categories, readme, authors, exclude)
- `README.md`: added multi-method installation section (cargo, binaries, homebrew, source)
- `cargo publish --dry-run --allow-dirty` passes

### Story 4.2: Pre-compiled Binaries
- `.github/workflows/release.yml`: GitHub Actions workflow building for 5 targets (linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64, windows-x86_64)
- Triggers on `v*` tags; creates GitHub Release with checksums

### Story 4.3: Homebrew Formula
- `Formula/minion-engine.rb`: Homebrew formula pointing to GitHub Releases pre-compiled binaries
- Supports macOS (arm64 + x86_64) and Linux (arm64 + x86_64)
- Includes `head` block for source builds as fallback

## wt4 — Epic 6 (Plugin System) & Epic 9 (Event Bus)
All 6 stories (6.1, 6.2, 6.3, 9.1, 9.2, 9.3) completed.
All 109 tests passing (92 unit + 17 integration).

### Story 6.1: Plugin Trait Interface and Registry
- `src/plugins/mod.rs`: `PluginStep` async trait + `PluginConfigSchema` struct
- `src/plugins/registry.rs`: `PluginRegistry` with `register`, `get`, `len`, `is_empty`

### Story 6.2: Dynamic Plugin Loading
- `src/plugins/loader.rs`: `PluginLoader::load_plugin(path)` using `libloading`
- `Cargo.toml`: added `libloading = "0.8"`
- `src/workflow/schema.rs`: `PluginDef` struct + `plugins: Vec<PluginDef>` on `WorkflowConfig`
- `src/engine/mod.rs`: `plugin_registry` field; plugin loading in `with_options`; step dispatch checks registry for unknown step types
- `src/main.rs`: added `mod plugins;` and `mod events;`

### Story 6.3: Plugin Configuration and Validation
- `src/workflow/validator.rs`: added `validate_plugin_configs(steps, registry)` — checks required fields from each plugin's `config_schema()` against step configs

### Story 9.1: Event Types and EventBus
- `src/events/types.rs`: `Event` enum with 7 variants: `StepStarted`, `StepCompleted`, `StepFailed`, `WorkflowStarted`, `WorkflowCompleted`, `SandboxCreated`, `SandboxDestroyed`
- `src/events/mod.rs`: `EventSubscriber` trait + `EventBus` using `tokio::sync::broadcast`
- `Cargo.toml`: added `serde` feature to `chrono`
- `src/lib.rs`: added `pub mod events;` and `pub mod plugins;`

### Story 9.2: Event Emission in Engine Execution Loop
- `src/engine/mod.rs`: added `event_bus: EventBus` field; `run()` emits `WorkflowStarted`/`WorkflowCompleted`; `execute_step()` emits `StepStarted`/`StepCompleted`/`StepFailed`

### Story 9.3: Webhook and File Subscriber
- `src/events/subscribers.rs`: `WebhookSubscriber` (fire-and-forget HTTP POST via tokio::spawn + reqwest) + `FileSubscriber` (append JSONL)
- `src/workflow/schema.rs`: `EventsConfig` struct + `events: Option<EventsConfig>` on `WorkflowConfig`
- `src/engine/mod.rs`: subscribers wired in `with_options` from `workflow.config.events`

### Story 4.4: Workflow Gallery
- `workflows/code-review.yaml`: PR/branch diff review with per-file parallel analysis
- `workflows/security-audit.yaml`: OWASP/CWE security audit with map parallelism
- `workflows/generate-docs.yaml`: AI documentation generator for source files
- `workflows/refactor.yaml`: Plan → implement → lint gate → test gate
- `workflows/flaky-test-fix.yaml`: 5-run flakiness detection + AI fix + 3-run verification
- `workflows/weekly-report.yaml`: git log + GitHub activity → polished Markdown report
- `prompts/`: 7 `.md.tera` template files for reusable prompts
