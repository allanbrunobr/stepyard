# Features

<!-- Generated from BMAD artifacts by /hive:from-bmad -->
<!-- Source: EPICS-AND-STORIES.md -->
<!-- Date: 2026-03-12 -->

## Feature 1: Project Bootstrap & CLI Skeleton
- Description: [Epic 1: MVP Foundation, Story 1.1] As a developer, I want to initialize the Rust project with a functional CLI, so that I have the base structure to add features. Source: EPICS-AND-STORIES.md
- Dependencies: none
- Status: pending

## Feature 2: YAML Workflow Parser
- Description: [Epic 1: MVP Foundation, Story 1.2] As a developer, I want to parse YAML workflow files into typed Rust structs, so that the engine can read and validate workflow definitions. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 1
- Status: pending

## Feature 3: Workflow Validator
- Description: [Epic 1: MVP Foundation, Story 1.3] As a developer, I want to validate the workflow before execution, so that configuration errors are detected before runtime. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 2
- Status: pending

## Feature 4: Context Store & Template Engine
- Description: [Epic 1: MVP Foundation, Story 1.4] As a developer, I want a context system that stores outputs and renders templates, so that steps can reference previous step outputs via {{ steps.name.field }}. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 2
- Status: pending

## Feature 5: Engine Core — Dispatch Loop
- Description: [Epic 1: MVP Foundation, Story 1.5] As a developer, I want the main engine loop that executes steps sequentially, so that each step is dispatched to the correct executor. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 4
- Status: pending

## Feature 6: Step — cmd (Shell Commands)
- Description: [Epic 1: MVP Foundation, Story 1.6] As a developer, I want to execute shell commands as workflow steps, so that deterministic steps (lint, test, git) work. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 7: Step — agent (Claude Code CLI Integration)
- Description: [Epic 1: MVP Foundation, Story 1.7] As a developer, I want to invoke Claude Code CLI as a workflow step, so that agentic steps (implement, fix) work. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 8: Step — gate (Conditional Flow Control)
- Description: [Epic 1: MVP Foundation, Story 1.8] As a developer, I want to evaluate conditions and control flow, so that the engine can decide whether to continue, stop, or skip steps. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 9: Step — repeat (Bounded Retry Loop)
- Description: [Epic 1: MVP Foundation, Story 1.9] As a developer, I want to execute a scope repeatedly until a gate breaks or max_iterations is reached, so that lint-fix-lint loops work like Stripe (max 2-3 rounds). Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 10: MVP Integration — fix-issue Workflow
- Description: [Epic 1: MVP Foundation, Story 1.10] As a user, I want to run `minion execute fix-issue.yaml -- 247` and get a PR, so that the MVP is demonstrable end-to-end. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 9
- Status: pending

## Feature 11: Step — chat (Direct LLM API)
- Description: [Epic 2: Complete Engine, Story 2.1] As a developer, I want to call LLM APIs directly without invoking Claude Code CLI, so that planning and summarization steps are fast and cheap. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 12: Step — map (Collection Processing)
- Description: [Epic 2: Complete Engine, Story 2.2] As a developer, I want to iterate over a collection executing a scope for each item, so that multi-file analysis works (like security audit). Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 13: Step — parallel (Independent Concurrent Steps)
- Description: [Epic 2: Complete Engine, Story 2.3] As a developer, I want to run independent steps in parallel, so that non-dependent analyses run simultaneously. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 14: Step — call (Scope Invocation)
- Description: [Epic 2: Complete Engine, Story 2.4] As a developer, I want to invoke a named scope as a sub-workflow, so that reusable logic can be organized into scopes. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 15: Config Manager — 4-Layer Merge
- Description: [Epic 2: Complete Engine, Story 2.5] As a developer, I want to resolve config with 4 priority layers, so that global, per-type, per-pattern and per-step config work like Roast. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: done

## Feature 16: Claude Code Session Management
- Description: [Epic 2: Complete Engine, Story 2.6] As a developer, I want to reuse Claude Code sessions between agent steps, so that agent context persists and responses are smarter. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 7
- Status: done

## Feature 17: Rich Terminal Display
- Description: [Epic 2: Complete Engine, Story 2.7] As a user, I want beautiful and informative terminal output, so that I can follow workflow progress. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 18: Step — template (Tera File Rendering)
- Description: [Epic 2: Complete Engine, Story 2.8] As a developer, I want to render .md.tera files as steps, so that long prompts can live in separate files. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 5
- Status: pending

## Feature 19: Docker Sandbox Integration
- Description: [Epic 3: Polish & Production, Story 3.1] As a user, I want to run workflows or agent steps in Docker Sandbox, so that I have Stripe devbox-style isolation. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 10
- Status: pending

## Feature 20: Dry-Run Mode
- Description: [Epic 3: Polish & Production, Story 3.2] As a user, I want to see which steps would be executed without executing anything, so that I can validate and understand the workflow before running. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 10
- Status: pending

## Feature 21: Resume From Step
- Description: [Epic 3: Polish & Production, Story 3.3] As a user, I want to resume a workflow from a specific step, so that I don't need to re-execute steps that already passed. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 10
- Status: pending

## Feature 22: JSON Output Mode
- Description: [Epic 3: Polish & Production, Story 3.4] As a user, I want workflow output in JSON format, so that I can integrate with other tools and pipelines. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 10
- Status: pending

## Feature 23: CLI — init & list & inspect Commands
- Description: [Epic 3: Polish & Production, Story 3.5] As a user, I want auxiliary commands to manage workflows, so that I can create, list and inspect workflows easily. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 10
- Status: pending

## Feature 24: Integration Tests
- Description: [Epic 3: Polish & Production, Story 3.6] As a developer, I want an integration test suite, so that I have confidence the engine works end-to-end. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 10
- Status: pending

## Feature 25: Documentation
- Description: [Epic 3: Polish & Production, Story 3.7] As a user and contributor, I want complete documentation, so that I can use and contribute to the engine. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 10
- Status: pending

## Feature 26: cargo install
- Description: [Epic 4: Distribution, Story 4.1] As a Rust user, I want to install via `cargo install minion-engine`, so that I don't need to clone the repository. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 25
- Status: pending

## Feature 27: Pre-compiled Binaries
- Description: [Epic 4: Distribution, Story 4.2] As a non-Rust user, I want to download a pre-compiled binary for my OS, so that I don't need to install the Rust toolchain. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 26
- Status: pending

## Feature 28: Homebrew Formula
- Description: [Epic 4: Distribution, Story 4.3] As a macOS user, I want to install via `brew install minion-engine`, so that I have easy install and updates. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 27
- Status: pending

## Feature 29: Workflow Gallery
- Description: [Epic 4: Distribution, Story 4.4] As a user, I want a collection of ready-to-use workflows, so that I can start quickly without writing YAML from scratch. Source: EPICS-AND-STORIES.md
- Dependencies: Feature 25
- Status: pending
