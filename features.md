# Features

<!-- Generated from BMAD artifacts by /hive:from-bmad -->
<!-- Source: _bmad-output/planning-artifacts/epics.md -->
<!-- Date: 2026-03-12 -->

## Feature 1: Define OutputType enum and extend StepDef schema
- Description: [Epic 1: Typed Output Parsing, Story 1.1] As a workflow author, I want to declare an output type on any step definition, so that the engine knows how to parse the step's raw output into structured data. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/workflow/schema.rs, src/steps/mod.rs
- Status: in_progress

## Feature 2: Implement output parsing logic in the engine
- Description: [Epic 1: Typed Output Parsing, Story 1.2] As a workflow engine, I want to automatically parse step outputs based on their declared output_type, so that downstream steps receive correctly typed data. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 1
- Files: src/engine/mod.rs, src/engine/context.rs
- Status: in_progress

## Feature 3: Add output type support to template rendering
- Description: [Epic 1: Typed Output Parsing, Story 1.3] As a workflow author, I want to access parsed output fields in templates using dot notation, so that I can reference specific values from JSON outputs. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 2
- Files: src/engine/context.rs, src/engine/template.rs
- Status: in_progress

## Feature 4: Add session resume support to agent executor
- Description: [Epic 2: Agent Session Continuity, Story 2.1] As a workflow author, I want an agent step to resume a previous Claude session, so that the Claude agent retains full context from earlier steps without re-reading everything. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 3
- Files: src/steps/agent.rs, src/workflow/schema.rs
- Status: in_progress

## Feature 5: Add session fork support to agent executor
- Description: [Epic 2: Agent Session Continuity, Story 2.2] As a workflow author, I want to fork an existing Claude session into parallel investigation paths, so that multiple agents can explore different approaches with the same baseline context. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 4
- Files: src/steps/agent.rs, src/workflow/schema.rs
- Status: in_progress

## Feature 6: Expose session_id in context for template access
- Description: [Epic 2: Agent Session Continuity, Story 2.3] As a workflow author, I want to access a step's session_id in templates, so that I can dynamically reference sessions in subsequent steps. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 4
- Files: src/engine/context.rs
- Status: in_progress

## Feature 7: Add async flag to StepDef and track pending futures
- Description: [Epic 3: Per-Step Async Execution, Story 3.1] As a workflow author, I want to mark any step as async: true, so that it starts executing in the background without blocking subsequent steps. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/workflow/schema.rs, src/engine/mod.rs
- Status: in_progress

## Feature 8: Implement automatic await on output reference
- Description: [Epic 3: Per-Step Async Execution, Story 3.2] As a workflow engine, I want to automatically await an async step when its output is needed, so that data dependencies are resolved transparently without manual synchronization. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 7
- Files: src/engine/mod.rs, src/engine/context.rs
- Status: in_progress

## Feature 9: Await all remaining async steps at workflow end
- Description: [Epic 3: Per-Step Async Execution, Story 3.3] As a workflow engine, I want to await all pending async steps before the workflow completes, so that no background work is silently lost. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 7
- Files: src/engine/mod.rs
- Status: in_progress

## Feature 10: Add Rhai scripting engine dependency and ScriptExecutor skeleton
- Description: [Epic 4: Embedded Script Step, Story 4.1] As a developer, I want the Rhai scripting engine integrated into the project, so that we have a sandboxed, Rust-native scripting runtime available for inline code evaluation. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: Cargo.toml, src/workflow/schema.rs, src/steps/script.rs, src/steps/mod.rs
- Status: in_progress

## Feature 11: Implement script execution with context access
- Description: [Epic 4: Embedded Script Step, Story 4.2] As a workflow author, I want to write inline Rhai scripts that can read and write to the workflow context, so that I can perform data transformations without spawning external processes. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 10
- Files: src/steps/script.rs, src/engine/mod.rs
- Status: in_progress

## Feature 12: Add script step to engine dispatch and sandbox support
- Description: [Epic 4: Embedded Script Step, Story 4.3] As a workflow engine, I want script steps to be dispatched correctly and work in both host and sandbox modes, so that scripts integrate seamlessly with the existing execution pipeline. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 11
- Files: src/engine/mod.rs
- Status: in_progress

## Feature 13: Add ChatHistory struct with message storage
- Description: [Epic 5: Chat Session Management, Story 5.1] As a workflow engine, I want to maintain a conversation history per chat session, so that multi-turn chat workflows can build on previous messages. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/steps/chat.rs, src/engine/context.rs
- Status: in_progress

## Feature 14: Implement truncation strategies
- Description: [Epic 5: Chat Session Management, Story 5.2] As a workflow author, I want to configure truncation strategies for chat history, so that long conversations don't exceed the model's context window. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 13
- Files: src/steps/chat.rs
- Status: in_progress

## Feature 15: Define plugin trait interface and registry
- Description: [Epic 6: Plugin System, Story 6.1] As a plugin developer, I want a clear trait interface to implement custom step types, so that I can create plugins that integrate with the engine's execution pipeline. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/plugins/mod.rs, src/plugins/registry.rs
- Status: in_progress

## Feature 16: Implement dynamic plugin loading from shared libraries
- Description: [Epic 6: Plugin System, Story 6.2] As a workflow author, I want to load plugins from .dylib/.so files specified in the workflow config, so that custom step types are available without recompiling the engine. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 15
- Files: Cargo.toml, src/plugins/loader.rs, src/engine/mod.rs
- Status: in_progress

## Feature 17: Add plugin configuration and validation
- Description: [Epic 6: Plugin System, Story 6.3] As a workflow author, I want plugins to declare their configuration schema, so that the engine can validate plugin step configs before execution. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 15
- Files: src/plugins/mod.rs, src/workflow/validator.rs
- Status: in_progress

## Feature 18: Add collect operation to map step
- Description: [Epic 7: Map Collect/Reduce Helpers, Story 7.1] As a workflow author, I want map results to be automatically collected into a single output, so that I don't need a separate step to aggregate iteration results. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/steps/map.rs, src/workflow/schema.rs
- Status: in_progress

## Feature 19: Add reduce operation to map step
- Description: [Epic 7: Map Collect/Reduce Helpers, Story 7.2] As a workflow author, I want to apply a reduce operation on collected map results, so that iteration outputs are consolidated into a single meaningful value. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 18
- Files: src/steps/map.rs, src/workflow/schema.rs
- Status: in_progress

## Feature 20: Implement safe accessor (?) for template expressions
- Description: [Epic 8: Three-Accessor Pattern, Story 8.1] As a workflow author, I want to use {{step.output?}} to safely access potentially missing data, so that my workflow doesn't fail when a conditional step was skipped. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/engine/context.rs, src/engine/template.rs
- Status: in_progress

## Feature 21: Implement strict accessor (!) for template expressions
- Description: [Epic 8: Three-Accessor Pattern, Story 8.2] As a workflow author, I want to use {{step.output!}} to explicitly require data to exist, so that I catch configuration errors early with clear error messages. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 20
- Files: src/engine/context.rs, src/engine/template.rs
- Status: in_progress

## Feature 22: Define event types and create EventBus
- Description: [Epic 9: Event & Instrumentation System, Story 9.1] As a developer, I want a structured event system with defined event types, so that the engine can emit lifecycle events for each step execution. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/events/mod.rs, src/events/types.rs
- Status: in_progress

## Feature 23: Integrate event emission into engine execution loop
- Description: [Epic 9: Event & Instrumentation System, Story 9.2] As a workflow engine, I want to emit events at key points in the execution lifecycle, so that subscribers receive real-time updates about workflow progress. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 22
- Files: src/engine/mod.rs, src/events/mod.rs
- Status: in_progress

## Feature 24: Add configurable webhook and file subscriber
- Description: [Epic 9: Event & Instrumentation System, Story 9.3] As a workflow author, I want to configure event subscribers in the workflow YAML, so that events are sent to external systems like webhooks or log files. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 22
- Files: src/events/subscribers.rs, src/workflow/schema.rs
- Status: in_progress

## Feature 25: Implement from() template function
- Description: [Epic 10: Cross-Scope Output Access, Story 10.1] As a workflow author, I want to use from(step_name).output in templates, so that I can access outputs from steps outside my current scope. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/engine/context.rs, src/engine/template.rs
- Status: in_progress

## Feature 26: Add from() support for deep field access
- Description: [Epic 10: Cross-Scope Output Access, Story 10.2] As a workflow author, I want to combine from() with dot notation for deep field access, so that I can reference specific fields from cross-scope outputs. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 25
- Files: src/engine/context.rs, src/engine/template.rs
- Status: in_progress
