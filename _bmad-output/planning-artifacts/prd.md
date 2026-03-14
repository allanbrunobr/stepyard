---
stepsCompleted:
  [
    'step-01-init',
    'step-02-discovery',
    'step-02b-vision',
    'step-02c-executive-summary',
    'step-03-success',
    'step-04-journeys',
    'step-05-domain',
    'step-07-project-type',
    'step-08-scoping',
    'step-09-functional',
    'step-10-nonfunctional',
    'step-11-polish',
    'step-12-complete',
  ]
inputDocuments:
  [
    'docs/index.md',
    'docs/architecture.md',
    'docs/project-overview.md',
    'docs/component-inventory.md',
    'docs/development-guide.md',
    'docs/CONFIG.md',
    'docs/DOCKER-SANDBOX.md',
    'docs/STEP-TYPES.md',
    'docs/YAML-SPEC.md',
    'docs/EXAMPLES.md',
    '_bmad-output/project-context.md',
    'features.md',
    '.hive/summary.md',
    'README.md',
    'ARCHITECTURE-MINION-ENGINE.md',
  ]
workflowType: 'prd'
prdType: 'audit'
projectType: 'cli'
---

# Product Requirements Document — Minion Engine (Audit)

**Author:** Bruno
**Date:** 2026-03-13
**Version:** 0.2.1
**Type:** Gap Audit — Existing Brownfield Project

---

## Executive Summary

Minion Engine is a Rust-based AI workflow engine (v0.2.1, published on crates.io) that orchestrates Claude Code CLI through declarative YAML workflows. It automates code review, issue fixing, refactoring, and PR creation with 10 step types, Docker sandbox isolation, and a 4-layer configuration system.

This PRD is an **audit document** — it identifies gaps between what is implemented, what is documented, and what users need. It does **not** propose new features. The goal is to bring existing capabilities to full documentation parity and surface quality/safety issues that require attention.

### What Makes This Special

The engine already has a strong foundation: 41 source files (9,504 LOC), 10 step types, trait-based extensibility, Docker sandbox, event bus, plugin system, and 9 built-in workflows. The core architecture is clean and well-documented internally (`project-context.md` has 42 AI agent rules). The audit reveals that **several implemented features are invisible to users** due to documentation gaps, and one entire epic (14 features) remains unimplemented.

## Project Classification

| Attribute | Value |
|-----------|-------|
| **Project Type** | CLI Tool + Library |
| **Domain** | Developer Tooling / AI Automation |
| **Complexity** | Medium-High |
| **Context** | Brownfield — existing codebase, published on crates.io |
| **Repository** | Monolith |
| **Language** | Rust (Edition 2021) |
| **Status** | 65% feature-complete (26/40 features done) |

---

## Success Criteria

### User Success

| Criteria | Current Status | Gap |
|----------|---------------|-----|
| User can install and run a workflow in < 5 min | Achieved (README) | None |
| User can discover all available step types | Partially — `script` step undocumented | **STEP-TYPES.md missing script** |
| User can debug workflow failures | Not achievable — no error docs | **No troubleshooting guide** |
| User can use all template syntax features | Partially — `?`, `!`, `from()` undocumented for users | **YAML-SPEC.md incomplete** |
| User can configure sandbox securely | Partially — no security guidance | **No SECURITY.md** |
| User can create plugins | Not achievable — no plugin dev guide | **No PLUGINS.md** |

### Technical Success

| Criteria | Current Status | Gap |
|----------|---------------|-----|
| Test coverage > 70% | 58% (25/41 files) | **12% below target** |
| All implemented features documented | 7 features undocumented | **Script, session resume, map reduce, async, accessors, truncation, from()** |
| Zero features documented but unimplemented | 14 features pending (Epic 11) | **features.md lists 14 unimplemented features** |
| Security documentation exists | Missing entirely | **Critical gap** |
| Performance guidance exists | Missing entirely | **Medium gap** |

### Measurable Outcomes

- **Documentation Parity Score:** 73% (10/13 files complete or mostly complete, 7 undocumented features)
- **Feature Completion Rate:** 65% (26/40)
- **User-Facing Doc Gaps:** 20 identified (10 critical, 7 medium, 3 minor)

---

## Product Scope

### Current State (v0.2.1 — Implemented)

- 10 step types (cmd, agent, chat, gate, repeat, map, parallel, call, template, script)
- Docker sandbox with 4 modes
- 4-layer configuration merge
- Hierarchical context tree with Tera templates
- Custom template preprocessing (`?`, `!`, `from()`)
- Event bus with webhook/file subscribers
- Plugin system (dynamic C ABI loading)
- Agent session resume/fork
- Chat truncation strategies (5 variants)
- Map collect/reduce
- Per-step async execution
- 9 built-in workflows
- GitHub Release CI (5 targets)
- Homebrew formula

### Unimplemented (Epic 11 — 14 Features)

Features 27-40 in `features.md` describe a Prompt Registry system with:
- Stack detection (Java, React, Python, Rust)
- Language-specific prompt templates
- Fallback chain resolver
- fix-ci.yaml, fix-test.yaml workflows

**Status:** Zero code exists. No `prompts/registry.yaml`, no `StackDetector`, no `PromptResolver`. These features should either be implemented or removed from `features.md` to avoid confusion.

### Documentation Gaps to Close (This Audit's Scope)

No new features. Only:
1. Document undocumented features
2. Create missing guide documents
3. Fix inconsistencies
4. Address security/performance gaps

---

## User Journeys

### Journey 1: New User Installation

**Current Flow:** Clone → `cargo install minion-engine` → `minion --version` → run `hello-world.yaml`

**Gap:** No troubleshooting for:
- Docker not running (sandbox ON by default)
- `claude` CLI not installed
- `ANTHROPIC_API_KEY` not set
- `gh` not authenticated

**Required:** Troubleshooting section in development-guide.md or new TROUBLESHOOTING.md

### Journey 2: User Creates Custom Workflow

**Current Flow:** User reads YAML-SPEC.md → creates .yaml → `minion execute`

**Gaps:**
- Template syntax incomplete (`?`, `!`, `from()` not in YAML-SPEC.md)
- `script` step type not in STEP-TYPES.md (user cannot discover Rhai scripting)
- `async: true` flag not documented
- Map `reduce` operation not documented
- Chat `truncation_strategy` not documented
- Session `resume`/`fork_session` not documented
- Control flow semantics (Break, Skip, Next) not explained for users

**Required:** Update YAML-SPEC.md, STEP-TYPES.md, add examples

### Journey 3: User Debugs a Failing Workflow

**Current Flow:** Workflow fails → user sees error → ???

**Gaps:**
- No error reference guide
- StepError variants not documented for users
- Timeout behavior not explained (what happens to child processes?)
- No log level guidance (`MINION_LOG` mentioned but not explained)

**Required:** TROUBLESHOOTING.md with common errors and solutions

### Journey 4: User Configures Sandbox Security

**Current Flow:** User runs workflow → sandbox ON by default → trusts defaults

**Gaps:**
- No security model explanation
- `--dangerously-skip-permissions` not explained
- Network isolation effectiveness not documented
- Credential injection risks not addressed
- No secrets management guidance

**Required:** SECURITY.md

### Journey 5: User Develops a Plugin

**Current Flow:** User reads architecture.md → sees plugin system → ???

**Gaps:**
- No plugin development guide
- No example plugin
- No distribution/discovery instructions
- PluginStep trait interface only in source code

**Required:** PLUGINS.md with hello-world plugin example

### Journey Requirements Summary

| Capability | Status | Priority |
|-----------|--------|----------|
| Install and run first workflow | Working | - |
| Use all template syntax | Partially documented | Critical |
| Use script step | Undocumented | Critical |
| Debug failures | No guide | Critical |
| Configure security | No guide | Critical |
| Develop plugins | No guide | Medium |
| Use session resume/fork | Undocumented | Medium |
| Use map reduce | Undocumented | Medium |
| Use async execution | Undocumented | Medium |
| Use chat truncation | Undocumented | Medium |
| Use event system | No examples | Low |

---

## CLI-Specific Requirements

### Project-Type Overview

Minion Engine is a CLI tool distributed as:
- Binary: `minion` (via `cargo install minion-engine`)
- Library: `minion-engine` crate (via `lib.rs`)
- Docker: `Dockerfile.sandbox` for sandbox image
- Homebrew: Formula in `Formula/`
- GitHub Releases: Pre-compiled binaries for 5 targets

### Technical Architecture Considerations

| Area | Status | Gap |
|------|--------|-----|
| CLI argument parsing | Complete (clap 4 derive) | None |
| Configuration system | Complete (4-layer merge) | Validation rules undocumented |
| Error reporting | Implemented (StepError) | No user-facing error guide |
| Progress display | Complete (indicatif + colored) | None |
| JSON output mode | Complete (--json) | None |
| Sandbox integration | Complete (Docker) | Security docs missing |
| Plugin system | Implemented | No user guide |
| Event system | Implemented | No usage examples |

### Implementation Considerations

- Binary name `minion` vs crate name `minion-engine` causes confusion — documentation should consistently use `minion` for the command
- `--dangerously-skip-permissions` flag needs explicit documentation with security implications
- Sandbox is ON by default — users without Docker get immediate failures

---

## Project Scoping & Phased Development

### Phase 1: Documentation Parity (Critical)

Close all gaps between implemented features and user-facing documentation.

| Item | Files Affected | Effort |
|------|---------------|--------|
| Document script step type | STEP-TYPES.md, YAML-SPEC.md, EXAMPLES.md | Small |
| Document template accessors (`?`, `!`, `from()`) | YAML-SPEC.md | Small |
| Document map collect/reduce | STEP-TYPES.md, EXAMPLES.md | Small |
| Document chat truncation strategies | STEP-TYPES.md, CONFIG.md | Small |
| Document session resume/fork | STEP-TYPES.md, EXAMPLES.md | Small |
| Document async step execution | YAML-SPEC.md, STEP-TYPES.md | Small |
| Document control flow semantics | YAML-SPEC.md | Small |
| Document output_type field | YAML-SPEC.md | Small |

### Phase 2: Missing Guides (High)

Create entirely new documentation files.

| Item | New File | Effort |
|------|----------|--------|
| Create troubleshooting guide | TROUBLESHOOTING.md | Medium |
| Create security guide | SECURITY.md | Medium |
| Create performance guide | PERFORMANCE.md | Medium |
| Create plugin development guide | PLUGINS.md | Medium |
| Add event system examples | CONFIG.md, EXAMPLES.md | Small |

### Phase 3: Quality & Testing (Medium)

| Item | Effort |
|------|--------|
| Increase test coverage from 58% to 70% | Medium |
| Add sandbox integration tests | Medium |
| Add event system tests | Small |
| Add plugin system tests | Small |
| Fix binary naming consistency across docs | Small |

### Phase 4: Feature Alignment (Decision Required)

| Item | Decision Needed |
|------|----------------|
| Epic 11 (Features 27-40): Implement or remove | **User decision** — 14 pending features with zero implementation |

### Risk Mitigation Strategy

- **Risk:** Users encounter `features.md` and expect Prompt Registry to work → **Mitigation:** Add "Status: Planned" badges to pending features
- **Risk:** Users without Docker fail immediately → **Mitigation:** Better error message + TROUBLESHOOTING.md
- **Risk:** Insecure sandbox configuration → **Mitigation:** SECURITY.md with defaults explanation

---

## Functional Requirements

_Audit mode: These are NOT new feature requests. They are documentation/quality requirements derived from gap analysis._

### FR-DOC: Documentation Requirements

| ID | Requirement | Priority | Gap Type |
|----|-------------|----------|----------|
| FR-DOC-01 | Script step type SHALL be documented in STEP-TYPES.md with output structure, Rhai reference, and example | Critical | Undocumented feature |
| FR-DOC-02 | Template accessors (`?`, `!`, `from()`) SHALL be documented in YAML-SPEC.md with syntax and examples | Critical | Partial documentation |
| FR-DOC-03 | Map collect/reduce operations SHALL be documented in STEP-TYPES.md | Critical | Undocumented feature |
| FR-DOC-04 | Chat truncation strategies (5 variants) SHALL be documented in STEP-TYPES.md and CONFIG.md | Medium | Undocumented feature |
| FR-DOC-05 | Agent session resume/fork SHALL be documented in STEP-TYPES.md with examples | Medium | Undocumented feature |
| FR-DOC-06 | Async step execution (`async: true`) SHALL be documented in YAML-SPEC.md | Medium | Undocumented feature |
| FR-DOC-07 | Control flow semantics (Break, Skip, Next, Fail) SHALL be documented in YAML-SPEC.md | Medium | Missing documentation |
| FR-DOC-08 | Output type field SHALL be documented in YAML-SPEC.md | Low | Missing documentation |

### FR-GUIDE: New Guide Requirements

| ID | Requirement | Priority | Gap Type |
|----|-------------|----------|----------|
| FR-GUIDE-01 | TROUBLESHOOTING.md SHALL document common errors: Docker not running, API key missing, timeout failures, sandbox copy errors | Critical | Missing guide |
| FR-GUIDE-02 | SECURITY.md SHALL document sandbox threat model, credential handling, `--dangerously-skip-permissions` risks, and secrets best practices | Critical | Missing guide |
| FR-GUIDE-03 | PERFORMANCE.md SHALL document timeout guidance per step type, sandbox overhead, and parallel execution tuning | Medium | Missing guide |
| FR-GUIDE-04 | PLUGINS.md SHALL document PluginStep trait interface, example plugin, build/distribution instructions | Medium | Missing guide |

### FR-EXAMPLE: Example Requirements

| ID | Requirement | Priority | Gap Type |
|----|-------------|----------|----------|
| FR-EX-01 | EXAMPLES.md SHALL include a script step workflow example | Critical | Missing example |
| FR-EX-02 | EXAMPLES.md SHALL include a session resume/fork example | Medium | Missing example |
| FR-EX-03 | EXAMPLES.md SHALL include a map reduce example | Medium | Missing example |
| FR-EX-04 | CONFIG.md SHALL include event subscriber configuration examples | Low | Missing example |

### FR-ALIGN: Feature Alignment Requirements

| ID | Requirement | Priority | Gap Type |
|----|-------------|----------|----------|
| FR-ALIGN-01 | features.md SHALL indicate status badges (Done/Planned/In Progress) for all features | Critical | Misleading content |
| FR-ALIGN-02 | Epic 11 (Features 27-40) SHALL be either implemented or clearly marked as "Planned — Not Yet Implemented" | Critical | Expectation mismatch |

---

## Non-Functional Requirements

### Security

| ID | Requirement | Priority |
|----|-------------|----------|
| NFR-SEC-01 | Sandbox security model SHALL be documented with threat boundaries | Critical |
| NFR-SEC-02 | `--dangerously-skip-permissions` SHALL display warning on first use | Medium |
| NFR-SEC-03 | Credential injection (GH_TOKEN, ANTHROPIC_API_KEY) SHALL be documented with best practices | Medium |
| NFR-SEC-04 | Workflow YAML files SHOULD NOT contain hardcoded secrets — docs SHALL warn against this | Medium |

### Performance

| ID | Requirement | Priority |
|----|-------------|----------|
| NFR-PERF-01 | Default timeout values SHALL be documented per step type | Medium |
| NFR-PERF-02 | Sandbox creation overhead SHALL be documented (approximate) | Low |
| NFR-PERF-03 | Map parallelism factor guidance SHALL be provided | Low |

### Testing

| ID | Requirement | Priority |
|----|-------------|----------|
| NFR-TEST-01 | Test coverage SHALL reach 70% of source files (currently 58%) | Medium |
| NFR-TEST-02 | Sandbox integration tests SHALL exist | Medium |
| NFR-TEST-03 | Event system SHALL have tests | Low |
| NFR-TEST-04 | Plugin system SHALL have tests | Low |

### Consistency

| ID | Requirement | Priority |
|----|-------------|----------|
| NFR-CON-01 | Binary name SHALL be consistently `minion` across all documentation | Low |
| NFR-CON-02 | Terminology SHALL be consistent: "step type" (not "step kind" or "executor") | Low |

---

## Audit Summary

### By Severity

| Severity | Count | Examples |
|----------|-------|---------|
| **Critical** | 10 | Script undocumented, no SECURITY.md, template syntax incomplete, Epic 11 unimplemented |
| **Medium** | 7 | Session resume, map reduce, async, chat truncation, control flow, plugins, performance |
| **Minor** | 3 | Binary naming, test coverage metric clarity, terminology |

### By Category

| Category | Gaps |
|----------|------|
| Undocumented implemented features | 7 |
| Missing guide documents | 4 (Troubleshooting, Security, Performance, Plugins) |
| Missing examples | 4 |
| Feature alignment issues | 2 (Epic 11, status badges) |
| Testing gaps | 4 |
| Consistency issues | 2 |

### Documentation Completeness Matrix

| Document | Completeness | Critical Gaps |
|----------|-------------|---------------|
| README.md | 95% | None |
| ARCHITECTURE-MINION-ENGINE.md | 90% | None |
| docs/architecture.md | 85% | Performance, event data flow |
| docs/project-overview.md | 95% | None |
| docs/component-inventory.md | 95% | None |
| docs/development-guide.md | 90% | Troubleshooting, plugin setup |
| docs/CONFIG.md | 85% | Validation errors, event subscribers |
| docs/DOCKER-SANDBOX.md | 90% | Non-git repos, troubleshooting |
| docs/STEP-TYPES.md | **70%** | **Script missing, session/truncation/async undocumented** |
| docs/YAML-SPEC.md | **75%** | **Template accessors, output_type, control flow** |
| docs/EXAMPLES.md | 80% | Script, session, map reduce examples |
| project-context.md | 95% | None (excellent AI agent guide) |
| features.md | **65%** | **14 unimplemented features listed** |

---

_PRD Audit Complete. This document identifies gaps only — no new features proposed._
