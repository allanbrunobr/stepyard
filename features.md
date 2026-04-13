# Features

<!-- Generated from BMAD artifacts by /hive:md-from-bmad -->
<!-- Source: _bmad-output/engine-v2/epics.md -->
<!-- Date: 2026-04-13 -->
<!-- Scope: Minion Engine v0.7.6 -> v2.0.0 refactor (pre-requisito do Agent Dashboard module) -->

## Feature 1: Criar crate minion-session com types publicos e schema SQL
- Description: [Epic 1: Session as First-Class Primitive, Story 1.1] As a engine developer, I want a new `minion-session` crate with `Session`, `SessionEvent`, `StepRecord` types plus SQL schema for `sessions` and `session_events` tables, so that other crates can depend on a stable session contract instead of in-process state. Source: _bmad-output/engine-v2/epics.md
- Dependencies: none
- Status: done

## Feature 2: Implementar Session::new e Session::append_event com persistencia
- Description: [Epic 1: Session as First-Class Primitive, Story 1.2] As a harness engineer, I want `Session::new(workflow_id, tenant_id) -> Session` and `Session::append(event)` persisting to PostgreSQL, so that the harness can create sessions and log events without managing storage itself. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 1
- Status: done

## Feature 3: Implementar Session::load e Session::replay com ordem deterministica
- Description: [Epic 1: Session as First-Class Primitive, Story 1.3] As a harness engineer, I want `Session::load(session_id)` rebuilding the handle from disk and `session.replay() -> Vec<Event>` in seq order, so that the harness can resume any session after process restart. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 2
- Status: done

## Feature 4: Integrar Session no engine v0.7.6 atual (replace in-memory state)
- Description: [Epic 1: Session as First-Class Primitive, Story 1.4] As a engine developer, I want the current v0.7.6 engine to use `Session::append` instead of its in-memory `Vec<Event>`, so that v0.7.6 behaves like v2 even before the full refactor. Source: _bmad-output/engine-v2/epics.md Existing: src/engine/mod.rs
- Dependencies: Feature 3
- Status: done

## Feature 5: Extrair minion-core com types compartilhados
- Description: [Epic 2: Decouple Harness and Sandbox, Story 2.1] As a engine developer, I want a `minion-core` crate containing `Event`, `StepRecord`, `WorkflowDef`, `Subscriber` trait, so that other crates depend only on stable types without I/O. Source: _bmad-output/engine-v2/epics.md Existing: src/events/types.rs ⚠ toca hub file
- Dependencies: Feature 4
- Status: done

## Feature 6: Extrair crate minion-sandbox-orchestrator com trait SandboxLifecycle
- Description: [Epic 2: Decouple Harness and Sandbox, Story 2.2] As a harness developer, I want a separate crate exposing `Sandbox`, `SandboxId` and trait `SandboxLifecycle`, so that the harness does not know Docker directly and tests can mock the lifecycle. Source: _bmad-output/engine-v2/epics.md Existing: src/sandbox/ ⚠ toca hub file
- Dependencies: Feature 5
- Status: pending

## Feature 7: Extrair crate minion-harness com Engine::step e Engine::resume
- Description: [Epic 2: Decouple Harness and Sandbox, Story 2.3] As a engine user, I want `Engine::new(HarnessConfig, Session, Box<dyn SandboxLifecycle>)` with `step`/`resume` methods, so that each step is an atomic transaction reconstructable via session replay. Source: _bmad-output/engine-v2/epics.md Existing: src/engine/mod.rs ⚠ toca hub file
- Dependencies: Feature 6
- Status: pending

## Feature 8: Migrar binario minion execute para usar nova API
- Description: [Epic 2: Decouple Harness and Sandbox, Story 2.4] As a operator, I want the existing `minion execute` subcommand to call `Engine::step` in a loop until completion or failure, so that the CLI UX stays the same after the refactor. Source: _bmad-output/engine-v2/epics.md Existing: src/cli/commands.rs
- Dependencies: Feature 7
- Status: pending

## Feature 9: Teste de stress multi-session concorrente
- Description: [Epic 2: Decouple Harness and Sandbox, Story 2.5] As a engine developer, I want an integration test dispatching 10 concurrent sessions on the same `Engine` instance, so that we prove `Engine: Send + Sync` and there are no races in the orchestrator. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 8
- Status: pending

## Feature 10: Crate minion-mcp-proxy com estrutura base e GET /healthz
- Description: [Epic 3: MCP Proxy as Separate Process, Story 3.1] As a platform engineer, I want a `minion-mcp-proxy` binary listening on a configurable port with a `GET /healthz` endpoint, so that we have validated boilerplate (container, healthcheck, logging) before implementing the real routes. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 9
- Status: pending

## Feature 11: Implementar validacao de JWT session_id HS256 15min
- Description: [Epic 3: MCP Proxy as Separate Process, Story 3.2] As a security engineer, I want every proxy request validated against an HS256 JWT with `session_id` and `tenant_id` claims and 15-minute TTL, so that credentials are scoped per session and never exposed to the agent container (ADR-011). Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 10
- Status: pending

## Feature 12: Implementar POST /mcp/:server/call com vault fetch + forward
- Description: [Epic 3: MCP Proxy as Separate Process, Story 3.3] As a platform engineer, I want `POST /mcp/:server/call` to fetch OAuth credentials from vault by `session_id` and forward the call to the real MCP server, so that the container never sees the raw token (Invariante 1). Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 11
- Status: pending

## Feature 13: Integrar harness para chamar proxy em vez de MCP direto
- Description: [Epic 3: MCP Proxy as Separate Process, Story 3.4] As a harness engineer, I want the harness to route MCP calls to `http://unix-socket/mcp/:server/call` instead of instantiating an MCP client directly, so that credentials cross the process boundary exactly once via the proxy. Source: _bmad-output/engine-v2/epics.md Existing: src/engine/mod.rs ⚠ toca hub file
- Dependencies: Feature 12
- Status: pending

## Feature 14: Subcommand minion serve com POST /workflows/dispatch
- Description: [Epic 4: Dispatch HTTP API, Story 4.1] As a dashboard developer, I want a `minion serve` subcommand exposing `POST /workflows/dispatch` that accepts `{workflow, target, vars}` and returns a `session_id`, so that the Agent Dashboard can trigger workflows over HTTP in addition to the CLI. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 13
- Status: pending

## Feature 15: Endpoint GET /sessions/:id com estado atual
- Description: [Epic 4: Dispatch HTTP API, Story 4.2] As a dashboard developer, I want `GET /sessions/:id` returning `{id, workflow_id, status, started_at, ended_at, step_count}`, so that the dashboard can render live session state without tailing logs. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 14
- Status: pending

## Feature 16: Endpoint GET /sessions/:id/events (SSE)
- Description: [Epic 4: Dispatch HTTP API, Story 4.3] As a dashboard developer, I want `GET /sessions/:id/events` with `Content-Type: text/event-stream` delivering every persisted session event in seq order plus new ones as they arrive, so that the dashboard streams workflow progress in real time. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 15
- Status: pending

## Feature 17: Endpoint POST /sessions/:id/resume para retomar sessions pausadas
- Description: [Epic 4: Dispatch HTTP API, Story 4.4] As a dashboard developer, I want `POST /sessions/:id/resume` calling `Engine::resume(session_id)` in the background, so that crashed or paused sessions can be continued from the last persisted event. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 16
- Status: pending

## Feature 18: Implementar PlanFile com backing S3-compat
- Description: [Epic 5: PLANS.md Persistence and YAML v1 Compat, Story 5.1] As a harness engineer, I want `PlanFile::load(session_id)` and `PlanFile::save(session_id, content)` persisting to S3-compat storage (MinIO local in VPS), so that each session has a durable PLANS.md surviving process restart and context compaction. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 17
- Status: pending

## Feature 19: Expor tools plan.read / plan.write ao agent
- Description: [Epic 5: PLANS.md Persistence and YAML v1 Compat, Story 5.2] As a agent operator, I want `plan.read` and `plan.write` tools exposed to Claude Code CLI within the session, so that the agent can read and edit its plan every turn without keeping it in the context window. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 18
- Status: pending

## Feature 20: Parser WorkflowDef::V1 | V2 com deteccao de versao
- Description: [Epic 5: PLANS.md Persistence and YAML v1 Compat, Story 5.3] As a engine developer, I want a parser producing `WorkflowDef::V1(v1) | V2(v2)` based on a `schema_version` field in the YAML, so that v1 workflows keep running while v2 takes advantage of session/plan/mcp primitives (ADR-012). Source: _bmad-output/engine-v2/epics.md Existing: src/workflow/ ⚠ toca hub file
- Dependencies: Feature 19
- Status: pending

## Feature 21: Subcommand minion migrate workflow.yaml (v1 -> v2)
- Description: [Epic 5: PLANS.md Persistence and YAML v1 Compat, Story 5.4] As a engine user, I want `minion migrate workflow.yaml` rewriting v1 files to v2 format in place (backup first) with semantic equivalence tests, so that we can migrate existing workflows automatically within the 180-day compat window. Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 20
- Status: pending

## Feature 22: Enforce data-limite 2026-10-13 para rejeitar v1
- Description: [Epic 5: PLANS.md Persistence and YAML v1 Compat, Story 5.5] As a engine maintainer, I want the parser to reject `WorkflowDef::V1` after 2026-10-13 with an explicit error linking to `minion migrate`, so that we eliminate the dead code path on schedule (ADR-012). Source: _bmad-output/engine-v2/epics.md
- Dependencies: Feature 21
- Status: pending
