# Minion Engine v2 — Architecture

**Status:** Draft (v2.0.0 pending)
**Author:** Winston (BMAD architect)
**Date:** 2026-04-13
**Review cadence:** trimestral (ADR-010 [ref-2])

Este documento descreve a arquitetura do **Minion Engine v2**, o workflow
engine em Rust que orquestra o Claude Code CLI em containers Docker
efemeros e expoe eventos para o modulo Agent Dashboard. Substitui
informalmente o codigo v0.7.6 (binario unico `minion` com harness,
sandbox e subscribers no mesmo crate) pela topologia hibrida aprovada
em ADR-011 e ADR-012.

A audiencia sao engenheiros que trabalham dentro do repositorio
`minion-engine/` e revisores de seguranca que precisam entender os
boundaries de credencial. Nao e um tutorial nem um reference manual
das APIs — estes vivem em `README.md` e `rustdoc`. Seguindo
matklad [ref-2], este documento **so descreve invariantes que nao
mudam com frequencia**: crate boundaries, contratos com o mundo
externo, anti-features. Serializacao, schema SQL, nomes de colunas,
loops internos ficam de fora — mudam a cada sprint e poluem o sinal.

Revise a cada trimestre. Se um PR precisa editar este arquivo mais de
uma vez por trimestre, provavelmente o documento esta especificando
implementacao ao inves de contrato — corte.

## Bird's Eye View

O engine e um orquestrador de agents LLM. Recebe um `WorkflowDef`
(YAML v1 ou v2), dispara steps em ordem, cada step executa o Claude
Code CLI dentro de um container Docker efemero (`Sandbox`), e emite
`Event`s para subscribers (stdout, webhook, SSE, DashboardSubscriber).
Credenciais OAuth para MCP servers nunca entram na memoria do engine
nem do container do agent — vivem no vault e sao proxyficadas por um
**binario separado** `minion-mcp-proxy` em seu proprio container.

Workspace Rust com cinco crates empacotados em **dois binarios**:

```
minion-engine/
├── crates/
│   ├── minion-core/              # types, traits, Event, WorkflowDef
│   ├── minion-session/           # Session append-only + StepRecord
│   ├── minion-harness/           # Engine::step / Engine::resume
│   ├── minion-sandbox-orchestrator/ # container lifecycle, cattle
│   └── minion-mcp-proxy/         # OAuth token broker (binario separado)
├── src/bin/minion.rs             # binario 1: engine + harness + sandbox
└── src/bin/mcp-proxy.rs          # binario 2: minion-mcp-proxy
```

Topologia de runtime (ADR-011):

```
┌─────────────────────────┐        ┌───────────────────────────┐
│ minion (binario unico)  │        │ minion-mcp-proxy          │
│  ├ harness              │  HTTP  │  (container Docker proprio)│
│  ├ session              │◄──────►│  fetch tokens do vault    │
│  └ sandbox-orchestrator │  unix  │  scope por session_id JWT │
└──────────┬──────────────┘ socket └──────────────┬────────────┘
           │ spawn                                │ HTTPS
           ▼                                      ▼
  ┌─────────────────────┐                  ┌──────────────┐
  │ Sandbox (container) │                  │ Vault        │
  │  claude-code CLI    │                  │ MCP servers  │
  └─────────────────────┘                  └──────────────┘
```

## Entry Points

- `minion-engine/src/bin/minion.rs` — CLI principal. Subcommands
  `execute`, `dispatch`, `migrate`, `serve`. Ponto de entrada para
  operadores e para o dashboard (HTTP local).
- `minion-engine/src/bin/mcp-proxy.rs` — binario separado que roda
  no container `minion-mcp-proxy`. HTTP server em unix socket ou
  `localhost:<porta>`; nao fala com o mundo externo alem do vault.
- `minion-engine/crates/minion-harness/src/engine.rs` — `Engine`
  struct com metodos `Engine::step(session_id)` e `Engine::resume(
  session_id)`. Toda execucao de step passa por aqui (fase B do
  refactor).
- `minion-engine/crates/minion-session/src/session.rs` — `Session`
  com `append(event)` e `replay() -> Vec<SessionEvent>`. Fonte de
  verdade para replay e audit.
- `tests/` — testes de integracao end-to-end, cada um dispara o
  binario real via `assert_cmd`.

## Code Map

Tour dos crates. Citamos apenas types e traits publicos **que
definem contratos**. Implementacao interna (modulos privados,
structs auxiliares, traits helper) nao esta aqui — leia o codigo.

### minion-core

O alicerce. Types e traits compartilhados pelos outros crates.
Nenhum I/O, nenhuma dependencia de runtime alem de `serde` e
`chrono`.

Publico:
- `Event` — enum serializavel com variantes `StepStarted`,
  `StepCompleted`, `StepFailed`, `WorkflowStarted`,
  `WorkflowCompleted`, `SandboxCreated`, `SandboxDestroyed`. Contrato
  estavel — `DashboardSubscriber` e downstream dependem disto.
- `StepRecord` — snapshot imutavel do resultado de um step
  (tokens, custo, duracao, erro opcional). Serializavel.
- `WorkflowDef` — enum `V1(WorkflowDefV1) | V2(WorkflowDefV2)`
  (ADR-012). Parser retorna este enum; resto do codigo casa.
- `Subscriber` trait — `fn on_event(&self, event: &Event)`. Stateless
  do ponto de vista do engine (subscriber pode ter estado interno mas
  engine nao gerencia).
- `EngineError` — erro de dominio. Sem `anyhow::Error` em APIs
  publicas.

### minion-session

`Session` e a **primitiva central** do engine v2 (fase A do refactor).
Todo o contexto que o harness reconstroi entre `step`s vem do replay
da session. Persistencia append-only em PostgreSQL.

Publico:
- `Session` — wrapper sobre `SessionId` + handle de persistencia.
  Metodos `append(event)`, `replay()`, `snapshot_at(step_idx)`.
- `SessionEvent` — envelope persistido (id, session_id, seq, created_at,
  payload: `Event`). Contrato de schema minimo — colunas exatas sao
  detalhe de implementacao.
- `SessionId` — UUID newtype.
- **Invariante:** uma vez escrito, nunca editado. Truncar = novo
  session_id.

### minion-harness

O loop de execucao. Consome `WorkflowDef`, emite `Event`s, spawna
sandboxes, chama Claude Code CLI. Versao v2 decompoe o `Engine::run()`
monolitico em `Engine::step()` + `Engine::resume()` (fase B).

Publico:
- `Engine` — orquestrador. `Engine::new(HarnessConfig, Session,
  Box<dyn SandboxLifecycle>)`. Metodos `step(session_id)`,
  `resume(session_id)`, `cancel(session_id)`.
- `HarnessConfig` — paths, timeouts, provider config (LiteLLM base
  URL, modelo default).
- `StepExecutor` trait — abstracao do que "executar um step" significa.
  Implementacao padrao chama Claude Code CLI; testes mockam.
- Plans: `PlanFile` — handle para o `PLANS.md` da session [ref-3]
  (fase E).

**Invariante do harness:** nao carrega credenciais OAuth em memoria.
Todas as tool calls MCP passam pelo `minion-mcp-proxy` via HTTP local.

### minion-sandbox-orchestrator

Responsavel por criar, reutilizar e destruir containers Docker.
Containers sao "cattle" [ref-4] — fungiveis, sem estado persistente
dentro deles.

Publico:
- `Sandbox` — handle opaco de um container vivo. Metodos `exec()`,
  `upload()`, `download()`.
- `SandboxLifecycle` trait — `create`, `destroy`, `reuse_or_create`.
  Implementacao padrao usa Docker daemon local; teste usa mock
  in-memory.
- `SandboxId` — UUID newtype, nao e o nome do container (que e
  derivavel mas privado).

**Invariante:** container pode ser destruido a qualquer momento sem
perda — qualquer estado relevante esta no `Session` log ou em volume
persistente.

### minion-mcp-proxy

**Binario separado** (ADR-011). Roda em container Docker proprio,
com seu proprio SELinux/AppArmor profile. Unica parte do sistema
autorizada a ler credenciais OAuth do vault.

Publico (API de rede, nao Rust):
- `POST /mcp/:server/call` — body: `{session_id, tool, args}`.
  Proxy valida JWT do session_id (15min HS256), busca token no vault,
  chama o MCP server real, retorna resposta.
- `GET /healthz`.

Protocolo com o harness: HTTP sobre unix socket (dev) ou
`localhost:<porta>` interno ao host (prod). Nao exposto fora do host.

**Invariante critica:** tokens OAuth so existem na memoria deste
processo. Um memory bug no harness nao vaza credencial.

## Cross-Cutting Concerns

### Logging

`tracing` em todo o codigo. Niveis: `error`, `warn`, `info`, `debug`,
`trace`. Logs estruturados (JSON em prod, pretty em dev).
`session_id` e `step_name` sempre em span. Nunca logamos conteudo de
tokens, payloads de prompt inteiros, ou dados do cliente.

### Error Handling

`thiserror` para erros de dominio (`EngineError`, `SessionError`,
`SandboxError`), `anyhow` so em binarios (`main.rs`) para context.
Erros de IO em tool calls viram `Event::StepFailed` e nao derrubam
o engine.

### Config

Fonte unica: `config.toml` no diretorio da session + overrides por
env var (`MINION_*`). `HarnessConfig` e imutavel apos `Engine::new`.

### Observability

Tres sinais: (1) `Event`s via subscribers (negocio), (2) `tracing`
logs (debug), (3) metricas Prometheus expostas pelo binario `serve`
(SLO). `DashboardSubscriber` e o consumer canonico dos events.

## Invariants

Invariantes que **devem** ser verdade. Violar qualquer um e bug.

1. **Credenciais OAuth nunca entram no processo do harness nem no
   container do agent.** Tokens vivem no vault; `minion-mcp-proxy` e
   o unico consumer autorizado, rodando em container separado. Racional:
   um memory bug em `unsafe` Rust ou dep C no harness nao pode
   comprometer multi-tenant (ADR-011 [ref-4]).
2. **Session e append-only.** `SessionEvent`s sao escritos, nunca
   editados ou deletados. Reconstruir contexto = replay do log.
   Racional: audit compliance (HIPAA/PCI) e determinismo em retry.
3. **Sandbox e cattle, nao pet.** Qualquer container pode ser destruido
   e recriado sem perda. Estado durevel so em `Session` log e volumes
   nomeados. Racional: crash recovery e isolation PCI/HIPAA [ref-4].
4. **`Event` e contrato publico.** Qualquer mudanca breaking no enum
   exige bump de major version do engine e migracao do
   `DashboardSubscriber`. Novas variantes sao backward-compatible se
   subscribers usam `#[serde(other)]` ou equivalente.
5. **YAML v1 roda ate 2026-10-13** (180 dias apos v2 GA hipotetico
   2026-04-13, ADR-012). Depois dessa data, o parser rejeita v1 com
   erro explicito e link para `minion migrate`.
6. **`WorkflowDef` e enum tagged** (`V1 | V2`). Type system garante
   que nenhum code path esquece de tratar uma das versoes durante a
   janela de compat.
7. **Cada step emite exatamente um par `StepStarted` / (`StepCompleted`
   | `StepFailed`).** Subscribers podem assumir pareamento. Um step
   interrompido via `cancel()` emite `StepFailed` com erro
   `Cancelled`.
8. **Nenhum codigo-fonte do cliente persiste no banco.** So metadata
   (paths, SHAs, nomes de arquivos). Conteudo fica no volume da
   sandbox, destruido com ela.
9. **`Engine` e `Send + Sync`.** Multi-session por processo e expected.
   Um engine por VPS (invariante 10), mas varias sessions em paralelo.
10. **Um engine = um tenant = uma VPS.** Nao ha sharding nem replicacao
    horizontal do harness. Escala multi-tenant via VPS separadas.
11. **O harness nao tem estado em memoria entre `step`s.** Tudo que
    precisa saber para continuar vem do `Session::replay()`. Restart
    do processo nao perde progress.
12. **`minion-mcp-proxy` autentica todo request via JWT `session_id`
    short-lived (15min, HS256).** Sem JWT valido, o proxy recusa
    antes de tocar o vault.

## Anti-Invariants (what we explicitly DON'T do)

Coisas que o engine **nao** faz por design. Se um PR adiciona uma
delas, precisa atualizar este documento primeiro.

- **Nao fazemos horizontal scaling do harness.** Um engine por VPS.
  Scale via tenants separados, nao replicas. Scheduling entre VPSs
  e problema do dashboard, nao do engine.
- **Nao implementamos nossos proprios prompts de agent.** Claude
  Code CLI e o agent [ref-1]; engine e harness externo. O engine
  monta contexto (retrieve-shape-inject [ref-7]) e invoca o CLI.
- **Nao persistimos codigo-fonte do cliente em nenhum banco.** So
  metadata. Conteudo vive em volume da sandbox, que e cattle.
- **Nao rodamos MCP servers dentro do engine.** Todos via
  `minion-mcp-proxy`. Isso vale inclusive para MCP "stdlib"
  (filesystem, git) se algum dia forem OAuth-gated.
- **Nao suportamos cold-start sem PostgreSQL.** Sem `Session`
  backend, engine recusa start. Sem dependencia "opcional" de
  storage.
- **Nao fazemos retry automatico de steps falhos por default.**
  Retry e politica do workflow (v2 permite `on_failure`), nao do
  engine. Racional: retry silencioso mascara bugs.
- **Nao aceitamos plugins nativos** (`.so` / `.dylib`) em producao
  nesta versao. O codigo `libloading` de v0.7 fica gated por feature
  flag desabilitada, e vai sumir se a politica de seguranca continuar
  firme em v3.
- **Nao rodamos o Claude Code CLI fora de sandbox.** Nao ha path
  "local unsafe mode" em producao. Dev-only `--no-sandbox` flag e
  recusado se `MINION_ENV=production`.
- **Nao proxyficamos nenhuma credencial alem do que MCP servers
  pedem.** Nao viramos broker generico de OAuth para o app do cliente.
- **Nao mantemos WebSocket persistente entre dashboard e engine.**
  SSE ou polling. WS adiciona state e reconnect complexity sem ganho.

## Boundaries

Contratos com o mundo externo. Mudancas em qualquer um destes
contratos sao breaking changes.

### Inbound

- **Dashboard → Engine** via `POST /workflows/dispatch` (fase D).
  Body: `{workflow_id, inputs, tenant_id}`. Response: `{session_id}`.
  Autenticacao: mutual TLS ou bearer token de service account.
- **CLI operator → Engine** via `minion execute <file.yaml>` e
  `minion dispatch <workflow-name>`. Mesmo code path que HTTP, so
  que local.
- **Events consumer → Engine** via `GET /sessions/:id/events` (SSE).
  Stream de `Event` serializados. Reconnect faz replay do zero —
  subscribers sao idempotentes por contrato (invariante 7).
- **Migration tool** via `minion migrate workflow.yaml` — converte
  v1 → v2 com teste de equivalencia. Saida em stdout.

### Outbound

- **Engine → Claude Code CLI** via `subprocess` dentro da sandbox.
  Contrato: a CLI aceita stdin JSON, emite JSON streaming, exit code
  != 0 = erro. Upstream owned; engine adapta a quebra de API.
- **Engine → LiteLLM** via HTTP `POST /chat/completions` (compat
  OpenAI). Usado para steps LLM "puros" (sem tool calls), fallback,
  e provider abstraction [ref-8].
- **Engine → PostgreSQL** via `sqlx`. Uma database por tenant;
  schema controlado por migrations em `minion-session/migrations/`.
- **Engine → minion-mcp-proxy** via HTTP unix socket
  (`/var/run/minion/mcp.sock`) ou `localhost:8787`.
- **minion-mcp-proxy → Vault** via HTTP (Hashicorp Vault API ou
  compat). Autenticacao via AppRole no boot do container.
- **minion-mcp-proxy → MCP server externo** via HTTPS (conforme
  registry do tenant). Tokens OAuth injetados pelo proxy, nao pelo
  harness.

## Data Model (minimal)

So as entidades de dominio. Schema SQL exato muda; nomes e
relacionamentos abaixo sao estaveis.

- **Workflow** — `id`, `version` (`v1` | `v2`), `yaml_source`,
  `tenant_id`, `created_at`. Imutavel; edicoes criam nova row com
  `id` novo (FR13 do PRD).
- **Session** — `id` (UUID), `workflow_id`, `tenant_id`, `status`
  (`running` | `completed` | `failed` | `cancelled`), `started_at`,
  `ended_at`. Uma por execucao de workflow.
- **SessionEvent** — `id`, `session_id` (FK), `seq` (monotonico por
  session), `created_at`, `payload` (JSON serializado de `Event`).
  Append-only; indice por `(session_id, seq)`.
- **Sandbox** — efemero, nao persistido no DB de sessions. Metadata
  minima em memoria do orchestrator. Cleanup e GC periodico.
- **PlanFile** — externo ao DB (volume persistente S3-compat ou
  MinIO), chaveado por `session_id`. Conteudo markdown livre editado
  pelo agent [ref-3].

## Process Topology

Dois processos por VPS. Nao mais, nao menos (ate ADR-011 ser
revisitado).

```
                       ┌─────────────────────────┐
   dashboard ──HTTP──► │ minion (binario 1)      │
   (remoto)            │ ┌─────────────────────┐ │
                       │ │ HTTP server (serve) │ │
                       │ ├─────────────────────┤ │
                       │ │ Engine (harness)    │ │
                       │ ├─────────────────────┤ │
                       │ │ Sandbox orchestrator│ │
                       │ └──────────┬──────────┘ │
                       └────────────┼────────────┘
                                    │ spawn
                                    ▼
                       ┌─────────────────────────┐
                       │ Sandbox container       │
                       │  (Docker, efemero)      │
                       │  claude-code CLI        │
                       └────────────┬────────────┘
                                    │ tool call
                                    ▼ via localhost
                       ┌─────────────────────────┐
                       │ minion-mcp-proxy        │
                       │ (binario 2, container)  │
                       │  ┌───────────────────┐  │
                       │  │ JWT validator     │  │
                       │  │ vault client      │  │
                       │  │ MCP forwarder     │  │
                       │  └─────────┬─────────┘  │
                       └────────────┼────────────┘
                                    │ HTTPS
                                    ▼
                    ┌──────────────────────────┐
                    │ vault + MCP servers ext  │
                    └──────────────────────────┘
```

Persistence (shared by both processes, read-only from proxy):

```
  PostgreSQL (sessions, events, workflows)
  Volume persistente (plan.md por session)
```

## File Layout

Estrutura do workspace apos fase A-E completas:

```
minion-engine/
├── Cargo.toml                       # workspace root
├── ARCHITECTURE.md                  # este documento
├── README.md
├── crates/
│   ├── minion-core/
│   │   └── src/{lib.rs, event.rs, workflow.rs, subscriber.rs}
│   ├── minion-session/
│   │   ├── src/{lib.rs, session.rs, store.rs}
│   │   └── migrations/
│   ├── minion-harness/
│   │   └── src/{lib.rs, engine.rs, executor.rs, plan.rs}
│   ├── minion-sandbox-orchestrator/
│   │   └── src/{lib.rs, sandbox.rs, docker.rs}
│   └── minion-mcp-proxy/
│       └── src/{lib.rs, jwt.rs, vault.rs, forwarder.rs}
├── src/
│   ├── bin/
│   │   ├── minion.rs                # binario 1 (agrega crates 1..4)
│   │   └── mcp-proxy.rs             # binario 2 (crate 5)
│   └── cli/                         # subcommands: execute/dispatch/migrate/serve
├── workflows/                       # exemplos v1 e v2
├── tests/                           # integracao end-to-end
└── docs/                            # rustdoc extra, nao este documento
```

## References

- **[ref-1] Harness Engineering (DeepMind, "The Bitter Lesson 2").**
  O harness envelhece mais rapido que o modelo; engine como harness
  externo e aposta consciente.
- **[ref-2] matklad — ARCHITECTURE.md template.**
  https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html —
  "Only specify things that are unlikely to frequently change".
  Influenciou ADR-010 e a politica de revisao trimestral.
- **[ref-3] PLANS.md pattern.** Plano externo editado pelo agent a
  cada turn; fonte de continuidade entre sessions. Fase E do refactor.
- **[ref-4] Anthropic Managed Agents.** Boundary de processo para
  credenciais, containers como cattle, JWT short-lived.
- **[ref-5] Simple building blocks.** Workflow deterministico como
  default, agent autonomo como escape hatch.
- **[ref-7] Retrieve-shape-inject de fragments.** Memory pipeline
  contratual (FR5 do PRD).
- **[ref-8] LiteLLM gateway.** Provider abstraction existente, reuso
  obrigatorio.
- **[ref-11] Event emitter Rust → `DashboardSubscriber`.** Consumer
  canonico do `Event` enum (ja em producao em v0.7.6).
- **PRD Agent Dashboard:**
  `/Users/bruno/Desktop/new-stripe-minions/_bmad-output/agent-dashboard/prd.md`
  — contem ADR-001 a ADR-012, Engine Refactor Roadmap (fases A-E),
  FR1-FR17, NFRs.
- **ADRs chave citados neste documento:** ADR-010 (review trimestral),
  ADR-011 (topologia hibrida), ADR-012 (YAML v1 compat 180d).
