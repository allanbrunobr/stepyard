# Minion Engine — Documentation Index

_Generated: 2026-03-13 | Scan: Quick | Mode: Initial Scan_

---

## Project Overview

- **Type:** Monolith (CLI + Library)
- **Language:** Rust (Edition 2021)
- **Architecture:** Pipeline / Step Executor
- **Version:** 0.2.1
- **Binary:** `minion`
- **License:** MIT

## Quick Reference

- **Tech Stack:** Rust + tokio + clap + tera + serde_yaml + reqwest + rhai
- **Entry Point:** `src/main.rs` → `cli::Cli::run()`
- **Step Types:** 10 (cmd, agent, chat, gate, repeat, map, parallel, call, template, script)
- **Source Files:** 41 files, 9,504 LOC
- **Test Coverage:** 58% (25/41 files with inline tests)

## Generated Documentation

- [Project Overview](./project-overview.md) — Summary, tech stack, core concepts
- [Architecture](./architecture.md) — System design, components, data flow
- [Source Tree Analysis](./source-tree-analysis.md) — Annotated directory structure
- [Component Inventory](./component-inventory.md) — All executors, types, traits, workflows
- [Development Guide](./development-guide.md) — Setup, commands, conventions, release process

## Existing Documentation

- [README.md](../README.md) — User-facing documentation with install, usage, examples
- [ARCHITECTURE-MINION-ENGINE.md](../ARCHITECTURE-MINION-ENGINE.md) — Original detailed architecture (Portuguese)
- [EPICS-AND-STORIES.md](../EPICS-AND-STORIES.md) — Legacy feature tracking
- [PROMPT.md](../PROMPT.md) — Prompt engineering reference
- [features.md](../features.md) — Feature tracker (40 features, 26 done + 14 pending)

## CI/CD

- [release.yml](../.github/workflows/release.yml) — GitHub Actions: Build 5 targets + GitHub Release on tag push

## AI Context Files

- [project-context.md](../_bmad-output/project-context.md) — Critical rules for AI agents (42 rules)
- [.hive/summary.md](../.hive/summary.md) — Codebase analysis summary
- [.hive/codebase-map.json](../.hive/codebase-map.json) — Full structural map (JSON)

## Getting Started

1. **Understand the project**: Read [Project Overview](./project-overview.md)
2. **Setup development**: Follow [Development Guide](./development-guide.md)
3. **Study architecture**: Review [Architecture](./architecture.md)
4. **Browse components**: Check [Component Inventory](./component-inventory.md)
5. **Explore code structure**: See [Source Tree Analysis](./source-tree-analysis.md)

## AI-Assisted Development

When using AI agents to implement features in this project:

1. Load `_bmad-output/project-context.md` as context first (42 critical rules)
2. Reference `docs/architecture.md` for structural decisions
3. Check `features.md` for current feature status and dependencies
4. Use `docs/component-inventory.md` to understand existing components
