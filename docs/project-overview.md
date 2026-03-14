# Project Overview — Minion Engine

_Generated: 2026-03-13 | Version: 0.2.1_

---

## Summary

**Minion Engine** is a Rust-based AI workflow engine that orchestrates Claude Code CLI through declarative YAML workflows. It automates complex software engineering tasks like code review, issue fixing, refactoring, and PR creation through multi-step workflows combining shell commands, AI agent calls, conditional gates, and retry loops.

**Author:** Allan Bruno
**License:** MIT
**Repository:** https://github.com/allanbrunobr/minion-engine
**Published:** crates.io (`cargo install minion-engine`)

## Architecture Classification

| Attribute | Value |
|-----------|-------|
| **Repository Type** | Monolith |
| **Project Type** | CLI Tool + Library |
| **Language** | Rust (Edition 2021) |
| **Runtime** | Tokio async |
| **Architecture Pattern** | Pipeline / Step Executor |
| **Binary Name** | `minion` |

## Technology Stack

| Category | Technology | Version |
|----------|-----------|---------|
| Language | Rust | Edition 2021 |
| Async Runtime | tokio | 1.x (full) |
| CLI Framework | clap | 4.x (derive) |
| Template Engine | tera | 1.x |
| Serialization | serde + serde_yaml | 1.x / 0.9 |
| HTTP Client | reqwest | 0.12 |
| Scripting | rhai | 1.x |
| Plugin Loading | libloading | 0.8 |
| Error Handling | thiserror + anyhow | 2.x / 1.x |
| Terminal UI | colored + indicatif | 2.x / 0.17 |

## Core Concepts

### Step Types (10)
| Step | Purpose |
|------|---------|
| `cmd` | Execute shell commands |
| `agent` | Invoke Claude Code CLI |
| `chat` | Call Anthropic/OpenAI API |
| `gate` | Conditional branching |
| `repeat` | Retry loops |
| `map` | Parallel iteration |
| `parallel` | Concurrent execution |
| `call` | Scope invocation |
| `template` | Render .md.tera prompts |
| `script` | Execute Rhai scripts |

### Key Features
- **YAML-driven workflows** — Declarative step definitions
- **4-layer config merge** — Global → type → pattern → step inline
- **Docker sandbox** — Isolated execution with workspace copy
- **Hierarchical context** — Parent-child scope inheritance
- **Event bus** — Lifecycle events with webhook/file subscribers
- **Plugin system** — Dynamic loading via C ABI
- **Session continuity** — Claude conversation resume/fork

## Codebase Metrics

| Metric | Value |
|--------|-------|
| Source files | 41 |
| Total LOC | 9,504 |
| Workflow files | 9 |
| Test coverage | 58% (25/41 files) |
| Hub files | 7 |

## Related Documentation

- [Source Tree Analysis](./source-tree-analysis.md)
- [Architecture](./architecture.md)
- [Development Guide](./development-guide.md)
- [ARCHITECTURE-MINION-ENGINE.md](../ARCHITECTURE-MINION-ENGINE.md) _(original detailed architecture)_
- [README.md](../README.md) _(user-facing documentation)_
