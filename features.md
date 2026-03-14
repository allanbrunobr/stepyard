# Features

<!-- Generated from BMAD artifacts by /hive:from-bmad -->
<!-- Source: _bmad-output/planning-artifacts/epics.md -->
<!-- Date: 2026-03-12 -->

## Feature 1: Define OutputType enum and extend StepDef schema
- Description: [Epic 1: Typed Output Parsing, Story 1.1] As a workflow author, I want to declare an output type on any step definition, so that the engine knows how to parse the step's raw output into structured data. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/workflow/schema.rs, src/steps/mod.rs
- Status: done

## Feature 2: Implement output parsing logic in the engine
- Description: [Epic 1: Typed Output Parsing, Story 1.2] As a workflow engine, I want to automatically parse step outputs based on their declared output_type, so that downstream steps receive correctly typed data. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 1
- Files: src/engine/mod.rs, src/engine/context.rs
- Status: done

## Feature 3: Add output type support to template rendering
- Description: [Epic 1: Typed Output Parsing, Story 1.3] As a workflow author, I want to access parsed output fields in templates using dot notation, so that I can reference specific values from JSON outputs. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 2
- Files: src/engine/context.rs, src/engine/template.rs
- Status: done

## Feature 4: Add session resume support to agent executor
- Description: [Epic 2: Agent Session Continuity, Story 2.1] As a workflow author, I want an agent step to resume a previous Claude session, so that the Claude agent retains full context from earlier steps without re-reading everything. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 3
- Files: src/steps/agent.rs, src/workflow/schema.rs
- Status: done

## Feature 5: Add session fork support to agent executor
- Description: [Epic 2: Agent Session Continuity, Story 2.2] As a workflow author, I want to fork an existing Claude session into parallel investigation paths, so that multiple agents can explore different approaches with the same baseline context. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 4
- Files: src/steps/agent.rs, src/workflow/schema.rs
- Status: done

## Feature 6: Expose session_id in context for template access
- Description: [Epic 2: Agent Session Continuity, Story 2.3] As a workflow author, I want to access a step's session_id in templates, so that I can dynamically reference sessions in subsequent steps. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 4
- Files: src/engine/context.rs
- Status: done

## Feature 7: Add async flag to StepDef and track pending futures
- Description: [Epic 3: Per-Step Async Execution, Story 3.1] As a workflow author, I want to mark any step as async: true, so that it starts executing in the background without blocking subsequent steps. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/workflow/schema.rs, src/engine/mod.rs
- Status: done

## Feature 8: Implement automatic await on output reference
- Description: [Epic 3: Per-Step Async Execution, Story 3.2] As a workflow engine, I want to automatically await an async step when its output is needed, so that data dependencies are resolved transparently without manual synchronization. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 7
- Files: src/engine/mod.rs, src/engine/context.rs
- Status: done

## Feature 9: Await all remaining async steps at workflow end
- Description: [Epic 3: Per-Step Async Execution, Story 3.3] As a workflow engine, I want to await all pending async steps before the workflow completes, so that no background work is silently lost. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 7
- Files: src/engine/mod.rs
- Status: done

## Feature 10: Add Rhai scripting engine dependency and ScriptExecutor skeleton
- Description: [Epic 4: Embedded Script Step, Story 4.1] As a developer, I want the Rhai scripting engine integrated into the project, so that we have a sandboxed, Rust-native scripting runtime available for inline code evaluation. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: Cargo.toml, src/workflow/schema.rs, src/steps/script.rs, src/steps/mod.rs
- Status: done

## Feature 11: Implement script execution with context access
- Description: [Epic 4: Embedded Script Step, Story 4.2] As a workflow author, I want to write inline Rhai scripts that can read and write to the workflow context, so that I can perform data transformations without spawning external processes. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 10
- Files: src/steps/script.rs, src/engine/mod.rs
- Status: done

## Feature 12: Add script step to engine dispatch and sandbox support
- Description: [Epic 4: Embedded Script Step, Story 4.3] As a workflow engine, I want script steps to be dispatched correctly and work in both host and sandbox modes, so that scripts integrate seamlessly with the existing execution pipeline. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 11
- Files: src/engine/mod.rs
- Status: done

## Feature 13: Add ChatHistory struct with message storage
- Description: [Epic 5: Chat Session Management, Story 5.1] As a workflow engine, I want to maintain a conversation history per chat session, so that multi-turn chat workflows can build on previous messages. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/steps/chat.rs, src/engine/context.rs
- Status: done

## Feature 14: Implement truncation strategies
- Description: [Epic 5: Chat Session Management, Story 5.2] As a workflow author, I want to configure truncation strategies for chat history, so that long conversations don't exceed the model's context window. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 13
- Files: src/steps/chat.rs
- Status: done

## Feature 15: Define plugin trait interface and registry
- Description: [Epic 6: Plugin System, Story 6.1] As a plugin developer, I want a clear trait interface to implement custom step types, so that I can create plugins that integrate with the engine's execution pipeline. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/plugins/mod.rs, src/plugins/registry.rs
- Status: done

## Feature 16: Implement dynamic plugin loading from shared libraries
- Description: [Epic 6: Plugin System, Story 6.2] As a workflow author, I want to load plugins from .dylib/.so files specified in the workflow config, so that custom step types are available without recompiling the engine. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 15
- Files: Cargo.toml, src/plugins/loader.rs, src/engine/mod.rs
- Status: done

## Feature 17: Add plugin configuration and validation
- Description: [Epic 6: Plugin System, Story 6.3] As a workflow author, I want plugins to declare their configuration schema, so that the engine can validate plugin step configs before execution. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 15
- Files: src/plugins/mod.rs, src/workflow/validator.rs
- Status: done

## Feature 18: Add collect operation to map step
- Description: [Epic 7: Map Collect/Reduce Helpers, Story 7.1] As a workflow author, I want map results to be automatically collected into a single output, so that I don't need a separate step to aggregate iteration results. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/steps/map.rs, src/workflow/schema.rs
- Status: done

## Feature 19: Add reduce operation to map step
- Description: [Epic 7: Map Collect/Reduce Helpers, Story 7.2] As a workflow author, I want to apply a reduce operation on collected map results, so that iteration outputs are consolidated into a single meaningful value. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 18
- Files: src/steps/map.rs, src/workflow/schema.rs
- Status: done

## Feature 20: Implement safe accessor (?) for template expressions
- Description: [Epic 8: Three-Accessor Pattern, Story 8.1] As a workflow author, I want to use {{step.output?}} to safely access potentially missing data, so that my workflow doesn't fail when a conditional step was skipped. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/engine/context.rs, src/engine/template.rs
- Status: done

## Feature 21: Implement strict accessor (!) for template expressions
- Description: [Epic 8: Three-Accessor Pattern, Story 8.2] As a workflow author, I want to use {{step.output!}} to explicitly require data to exist, so that I catch configuration errors early with clear error messages. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 20
- Files: src/engine/context.rs, src/engine/template.rs
- Status: done

## Feature 22: Define event types and create EventBus
- Description: [Epic 9: Event & Instrumentation System, Story 9.1] As a developer, I want a structured event system with defined event types, so that the engine can emit lifecycle events for each step execution. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/events/mod.rs, src/events/types.rs
- Status: done

## Feature 23: Integrate event emission into engine execution loop
- Description: [Epic 9: Event & Instrumentation System, Story 9.2] As a workflow engine, I want to emit events at key points in the execution lifecycle, so that subscribers receive real-time updates about workflow progress. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 22
- Files: src/engine/mod.rs, src/events/mod.rs
- Status: done

## Feature 24: Add configurable webhook and file subscriber
- Description: [Epic 9: Event & Instrumentation System, Story 9.3] As a workflow author, I want to configure event subscribers in the workflow YAML, so that events are sent to external systems like webhooks or log files. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 22
- Files: src/events/subscribers.rs, src/workflow/schema.rs
- Status: done

## Feature 25: Implement from() template function
- Description: [Epic 10: Cross-Scope Output Access, Story 10.1] As a workflow author, I want to use from(step_name).output in templates, so that I can access outputs from steps outside my current scope. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: none
- Files: src/engine/context.rs, src/engine/template.rs
- Status: done

## Feature 26: Add from() support for deep field access
- Description: [Epic 10: Cross-Scope Output Access, Story 10.2] As a workflow author, I want to combine from() with dot notation for deep field access, so that I can reference specific fields from cross-scope outputs. Source: _bmad-output/planning-artifacts/epics.md
- Dependencies: Feature 25
- Files: src/engine/context.rs, src/engine/template.rs
- Status: done

<!-- ═══════════════════════════════════════════════════════════════ -->
<!-- Epic 11: Prompt Registry with Language-Specific Agents         -->
<!-- Date: 2026-03-13                                               -->
<!-- ═══════════════════════════════════════════════════════════════ -->

## Feature 27: Stack Registry YAML Schema and Parser
- Description: [Epic 11: Prompt Registry, Story 11.1] Create prompts/registry.yaml defining stack detection rules (file markers, content_match patterns), parent inheritance chains (react→typescript→javascript), tool commands (lint, test, build per stack), and detection_order priority. Implement Registry struct and YAML parser in new src/prompts/registry.rs.
- Dependencies: none
- Files: prompts/registry.yaml, src/prompts/mod.rs, src/prompts/registry.rs
- Status: pending

## Feature 28: Stack Detector
- Description: [Epic 11: Prompt Registry, Story 11.2] Implement StackDetector that reads registry.yaml and auto-detects the project's technology stack by checking file markers (pom.xml, package.json, Cargo.toml) and content patterns (spring-boot in pom.xml, react in package.json). Follows detection_order priority for specificity (java-spring before java). Returns StackInfo with name, parent chain, and tool commands.
- Dependencies: Feature 27
- Files: src/prompts/detector.rs
- Status: pending

## Feature 29: Prompt Resolver with Fallback Chain
- Description: [Epic 11: Prompt Registry, Story 11.3] Implement PromptResolver that resolves a prompt file given a function name (fix-lint, fix-test, code-review) and detected stack. Follows the parent inheritance chain (react→typescript→javascript→_default.md.tera) until a .md.tera file is found. Returns resolved file path or descriptive error with suggestion to create the missing file.
- Dependencies: Feature 27
- Files: src/prompts/resolver.rs
- Status: pending

## Feature 30: Dynamic Template Path in TemplateStepExecutor
- Description: [Epic 11: Prompt Registry, Story 11.4] Modify TemplateStepExecutor to support dynamic path via the step.prompt field. When prompt is set (e.g., "fix-lint/{{ lang }}"), render it as a Tera template and use the result as the file path instead of hardcoded step.name. Backward-compatible — falls back to step.name.md.tera when prompt is absent. Approximately 5 lines changed in template_step.rs.
- Dependencies: none
- Files: src/steps/template_step.rs
- Status: pending

## Feature 31: Stack Context Variables in Engine
- Description: [Epic 11: Prompt Registry, Story 11.5] After stack detection, inject stack variables into the Tera context: {{ stack.name }}, {{ stack.tools.lint }}, {{ stack.tools.test }}, {{ stack.tools.build }}, {{ stack.parent }}. Workflows can reference detected stack info without hardcoding commands. Integrate StackDetector into Engine initialization when prompts/registry.yaml exists.
- Dependencies: Feature 28
- Files: src/engine/mod.rs, src/engine/context.rs
- Status: done

## Feature 32: Auto-Resolved Prompt Variables
- Description: [Epic 11: Prompt Registry, Story 11.6] Expose {{ prompts.fix-lint }}, {{ prompts.fix-test }}, {{ prompts.code-review }} in the template context. Each variable auto-resolves to the rendered content of the appropriate prompt .md.tera file for the detected stack using PromptResolver + fallback chain. Combines detection + resolution + rendering into a single template variable access.
- Dependencies: Feature 29, Feature 31
- Files: src/engine/context.rs, src/prompts/mod.rs
- Status: done

## Feature 33: Base Prompt Templates (_default)
- Description: [Epic 11: Prompt Registry, Story 11.7] Create language-agnostic fallback prompts: prompts/fix-lint/_default.md.tera, prompts/fix-test/_default.md.tera, and prompts/code-review/_default.md.tera. These serve as universal fallbacks when no stack-specific prompt exists. Include Tera placeholders for {{ steps.run_lint.stdout }} and other contextual data.
- Dependencies: Feature 29
- Files: prompts/fix-lint/_default.md.tera, prompts/fix-test/_default.md.tera, prompts/code-review/_default.md.tera
- Status: pending

## Feature 34: Java Prompt Templates
- Description: [Epic 11: Prompt Registry, Story 11.8] Create Java-specific prompts: fix-lint/java.md.tera (Checkstyle, SpotBugs, PMD, Google Java Style), fix-test/java.md.tera (JUnit5, Mockito, AssertJ), code-review/java.md.tera (SOLID, Spring patterns, Maven/Gradle). Add java-spring.md.tera variants with Spring Boot Test, MockMvc, and @Configuration expertise.
- Dependencies: Feature 33
- Files: prompts/fix-lint/java.md.tera, prompts/fix-lint/java-spring.md.tera, prompts/fix-test/java.md.tera, prompts/fix-test/java-spring.md.tera, prompts/code-review/java.md.tera
- Status: pending

## Feature 35: React and TypeScript Prompt Templates
- Description: [Epic 11: Prompt Registry, Story 11.9] Create React/TS prompts: fix-lint/react.md.tera (ESLint + hooks rules, JSX), fix-lint/typescript.md.tera (ESLint, Prettier, strict mode), fix-test/react.md.tera (React Testing Library, Cypress), code-review/react.md.tera (hooks patterns, rendering optimization, state management). React inherits from TypeScript via registry.yaml.
- Dependencies: Feature 33
- Files: prompts/fix-lint/react.md.tera, prompts/fix-lint/typescript.md.tera, prompts/fix-test/react.md.tera, prompts/code-review/react.md.tera
- Status: pending

## Feature 36: Python and Rust Prompt Templates
- Description: [Epic 11: Prompt Registry, Story 11.10] Create Python prompts: fix-lint/python.md.tera (ruff, mypy, flake8), fix-test/python.md.tera (pytest, unittest), code-review/python.md.tera. Create Rust prompts: fix-lint/rust.md.tera (clippy), fix-test/rust.md.tera (cargo test), code-review/rust.md.tera. Completes initial coverage for 5 stacks.
- Dependencies: Feature 33
- Files: prompts/fix-lint/python.md.tera, prompts/fix-lint/rust.md.tera, prompts/fix-test/python.md.tera, prompts/fix-test/rust.md.tera, prompts/code-review/python.md.tera, prompts/code-review/rust.md.tera
- Status: pending

## Feature 37: fix-ci.yaml Workflow
- Description: [Epic 11: Prompt Registry, Story 11.11] Create workflows/fix-ci.yaml — generic CI fix workflow that: checks out PR branch, auto-detects stack via registry, installs deps using {{ stack.tools.install }}, runs lint via {{ stack.tools.lint }}, loads specialized prompt via {{ prompts.fix-lint }}, fixes errors in repeat loop (max 3), commits and pushes fix. Zero hardcoded language logic in the YAML.
- Dependencies: Feature 31, Feature 32
- Files: workflows/fix-ci.yaml
- Status: pending

## Feature 38: fix-test.yaml Workflow
- Description: [Epic 11: Prompt Registry, Story 11.12] Create workflows/fix-test.yaml — similar to fix-ci but for test failures. Runs {{ stack.tools.test }}, loads {{ prompts.fix-test }} for detected stack, fixes failing tests in repeat loop (max 3), pushes fix. Supports both unit and integration test modes via args.mode parameter.
- Dependencies: Feature 37
- Files: workflows/fix-test.yaml
- Status: pending

## Feature 39: Integrate Stack Detection in Pre-Flight Validation
- Description: [Epic 11: Prompt Registry, Story 11.13] Add stack detection to validate_environment() in cli/commands.rs. When a workflow references {{ stack.* }} or {{ prompts.* }}, pre-flight validates that registry.yaml exists, stack can be detected, and required prompt files are found. Provides actionable error: "No prompt for fix-lint/go — create prompts/fix-lint/go.md.tera or prompts/fix-lint/_default.md.tera".
- Dependencies: Feature 28, Feature 29
- Files: src/cli/commands.rs
- Status: done

## Feature 40: Integration Tests for Prompt Resolver
- Description: [Epic 11: Prompt Registry, Story 11.14] Add integration tests covering: registry.yaml parsing, stack detection from fixture projects (Java, React, Python, Rust), fallback chain traversal (react→typescript→_default), dynamic template path loading, missing prompt error messages, and circular inheritance detection. Use tempdir fixtures with marker files.
- Dependencies: Feature 29, Feature 30
- Files: tests/prompt_resolver.rs, tests/fixtures/registry.yaml, tests/fixtures/prompts/
- Status: pending
