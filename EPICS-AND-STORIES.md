# Minion Engine — Epics & Stories

> Source: [ARCHITECTURE-MINION-ENGINE.md](./ARCHITECTURE-MINION-ENGINE.md)
> Project: Rust-based workflow engine that orchestrates Claude Code CLI
> Methodology: Incremental delivery — each epic produces a usable artifact

---

## Overview

| Epic | Nome | Fase | Objetivo |
|------|------|------|----------|
| E1 | MVP Foundation | Fase 1 | Engine roda `fix-issue.yaml` end-to-end |
| E2 | Complete Engine | Fase 2 | Feature-parity com Roast (todos step types + config) |
| E3 | Polish & Production | Fase 3 | Docker Sandbox, dry-run, resume, testes, docs |
| E4 | Distribution | Fase 4 | cargo install, binarios, homebrew, workflow gallery |

**Definicao de Done (global)**: Story esta completa quando:
- Codigo compila sem warnings
- Testes unitarios passam
- Funcionalidade pode ser demonstrada com um workflow YAML de exemplo
- Documentacao inline (doc comments) nos tipos publicos

---

## Epic 1: MVP Foundation

**Objetivo**: Ser capaz de executar um workflow YAML simples que roda comandos shell, invoca Claude Code, avalia gates, e repete em loop. Ao final deste epic, `minion execute fix-issue.yaml -- 247` funciona end-to-end.

**Estimativa**: ~1,200 linhas de Rust

### Story 1.1: Project Bootstrap & CLI Skeleton

**Como** desenvolvedor,
**Quero** inicializar o projeto Rust com CLI funcional,
**Para que** tenha a estrutura base onde adicionar funcionalidades.

**Acceptance Criteria:**
- [ ] `cargo new minion-engine` com estrutura de diretorios conforme arquitetura (src/cli/, src/workflow/, src/engine/, src/steps/, src/claude/, src/config/)
- [ ] `Cargo.toml` com dependencias: tokio, clap, serde, serde_yaml, serde_json, tera, anyhow, thiserror, tracing, tracing-subscriber, colored, indicatif, regex, chrono
- [ ] CLI com clap derive: subcomando `execute` aceita `<workflow_path>`, `-- <target>`, `--verbose`, `--quiet`
- [ ] Subcomando `validate` aceita `<workflow_path>`
- [ ] Subcomando `version` imprime versao
- [ ] `minion --help` mostra ajuda formatada
- [ ] `minion execute nonexistent.yaml` retorna erro amigavel
- [ ] Tracing subscriber configurado (RUST_LOG=debug para verbose)

**Arquivos:**
- `Cargo.toml`
- `src/main.rs`
- `src/lib.rs`
- `src/cli/mod.rs`
- `src/cli/commands.rs`

**Estimativa:** ~120 linhas

---

### Story 1.2: YAML Workflow Parser

**Como** desenvolvedor,
**Quero** parsear arquivos YAML de workflow em structs Rust tipadas,
**Para que** o engine possa ler e validar definicoes de workflow.

**Acceptance Criteria:**
- [ ] Struct `WorkflowDef` com campos: name, version, description, config, prompts_dir, scopes, steps
- [ ] Struct `StepDef` com campos: name, type (enum StepType), run, prompt, condition, scope, max_iterations, items, parallel, steps (aninhados), config, on_pass, on_fail, message, initial_value, outputs
- [ ] Enum `StepType`: Cmd, Agent, Chat, Gate, Repeat, Map, Parallel, Call, Template
- [ ] Struct `ScopeDef` com campos: steps, outputs
- [ ] Parse do YAML completo do `fix-issue.yaml` (secao 4 da arquitetura) sem erros
- [ ] Campos opcionais sao `Option<T>` e nao falham se ausentes
- [ ] Teste unitario: parse de YAML minimo (1 step cmd)
- [ ] Teste unitario: parse de YAML completo com scopes
- [ ] Teste unitario: YAML invalido retorna erro descritivo

**Arquivos:**
- `src/workflow/mod.rs`
- `src/workflow/schema.rs`
- `src/workflow/parser.rs`
- `tests/fixtures/minimal.yaml`
- `tests/fixtures/fix-issue.yaml`

**Estimativa:** ~200 linhas

---

### Story 1.3: Workflow Validator

**Como** desenvolvedor,
**Quero** validar o workflow antes de executar,
**Para que** erros de configuracao sejam detectados antes do runtime.

**Acceptance Criteria:**
- [ ] Funcao `validate(workflow: &WorkflowDef) -> Result<(), Vec<ValidationError>>`
- [ ] Validacao: todos os scopes referenciados por repeat/map/call existem no `scopes:`
- [ ] Validacao: nomes de steps sao unicos dentro de um scope
- [ ] Validacao: steps do tipo `cmd` tem campo `run` preenchido
- [ ] Validacao: steps do tipo `agent` ou `chat` tem campo `prompt` preenchido
- [ ] Validacao: steps do tipo `repeat`/`map`/`call` tem campo `scope` preenchido
- [ ] Validacao: steps do tipo `gate` tem campo `condition` preenchido
- [ ] Validacao: `max_iterations` em repeat e > 0
- [ ] Validacao: sem ciclos em call → scope → call (deteccao de referencia circular)
- [ ] Retorna TODAS as validacoes falhas de uma vez (nao para no primeiro)
- [ ] Subcomando `minion validate workflow.yaml` usa esta funcao
- [ ] Teste unitario: workflow valido passa
- [ ] Teste unitario: scope inexistente detectado
- [ ] Teste unitario: ciclo detectado

**Arquivos:**
- `src/workflow/validator.rs`

**Estimativa:** ~150 linhas

---

### Story 1.4: Context Store & Template Engine

**Como** desenvolvedor,
**Quero** um sistema de contexto que armazena outputs e renderiza templates,
**Para que** steps possam referenciar outputs de steps anteriores via `{{ steps.name.field }}`.

**Acceptance Criteria:**
- [ ] Struct `Context` com: steps (HashMap<String, StepOutput>), variables (HashMap<String, Value>), parent (Option<Arc<Context>>), scope_value, scope_index
- [ ] Metodo `store(name, output)` — armazena output de um step
- [ ] Metodo `get_step(name) -> Option<&StepOutput>` — busca, procurando no parent se nao achar
- [ ] Metodo `child(scope_value, index) -> Context` — cria contexto filho
- [ ] Metodo `to_tera_context() -> tera::Context` — converte para uso com Tera
- [ ] Enum `StepOutput` com variantes: Cmd(CmdOutput), Agent(AgentOutput), Chat(ChatOutput), Gate(GateOutput), Scope(ScopeOutput), Template(TemplateOutput), Empty
- [ ] Accessors universais no StepOutput: `text()`, `json()`, `lines()`, `exit_code()`, `success()`
- [ ] Wrapper do Tera que renderiza templates com o contexto
- [ ] Template `{{ target }}` resolve para variavel do workflow
- [ ] Template `{{ steps.fetch.stdout }}` resolve para output de step anterior
- [ ] Template `{{ scope.value }}` resolve em contextos de map/repeat
- [ ] Teste unitario: store e retrieve
- [ ] Teste unitario: heranca de parent context
- [ ] Teste unitario: render de template simples
- [ ] Teste unitario: render de template com steps aninhados

**Arquivos:**
- `src/engine/context.rs`
- `src/engine/template.rs`
- `src/steps/mod.rs` (StepOutput enum + structs)
- `src/control_flow.rs`
- `src/error.rs`

**Estimativa:** ~300 linhas

---

### Story 1.5: Engine Core — Dispatch Loop

**Como** desenvolvedor,
**Quero** o loop principal do engine que executa steps sequencialmente,
**Para que** cada step seja despachado para o executor correto.

**Acceptance Criteria:**
- [ ] Struct `Engine` com: workflow (WorkflowDef), context (Context), config_manager (placeholder para agora)
- [ ] Metodo `async fn run(&mut self) -> Result<StepOutput>`
- [ ] Loop sequencial sobre `workflow.steps`
- [ ] Para cada step: renderiza templates → despacha para executor → armazena output
- [ ] Dispatch por StepType: Cmd → CmdExecutor, Agent → AgentExecutor, Gate → GateExecutor, Repeat → RepeatExecutor (demais tipos retornam erro "not implemented" por agora)
- [ ] Handle de ControlFlow::Skip — armazena Empty, continua
- [ ] Handle de ControlFlow::Fail — aborta com mensagem
- [ ] Handle de ControlFlow::Break — propaga para scope pai
- [ ] Logging com tracing: step name, tipo, duracao, status (OK/SKIP/FAIL)
- [ ] Terminal output: nome do step, spinner enquanto roda, resultado
- [ ] Retorna output do ultimo step
- [ ] Teste unitario: engine com 2 cmd steps sequenciais

**Arquivos:**
- `src/engine/mod.rs`
- `src/engine/executor.rs`
- `src/cli/display.rs`

**Estimativa:** ~250 linhas

---

### Story 1.6: Step — cmd (Shell Commands)

**Como** desenvolvedor,
**Quero** executar comandos shell como steps de workflow,
**Para que** passos deterministicos (lint, test, git) funcionem.

**Acceptance Criteria:**
- [ ] Struct `CmdExecutor` implementando trait `StepExecutor`
- [ ] Spawna `bash -c "<command>"` via `tokio::process::Command`
- [ ] Captura stdout e stderr separadamente via `Stdio::piped()`
- [ ] Captura exit code
- [ ] Mede duracao da execucao
- [ ] Retorna `CmdOutput { stdout, stderr, exit_code, duration }`
- [ ] Se `fail_on_error` (config) e exit_code != 0: retorna Err(StepError::Fail)
- [ ] Se `fail_on_error` e false: retorna Ok mesmo com exit != 0
- [ ] Timeout via `tokio::time::timeout` (default 60s, configuravel)
- [ ] Working directory configuravel via config
- [ ] Teste unitario: `echo hello` retorna stdout "hello\n"
- [ ] Teste unitario: `exit 1` com fail_on_error retorna Fail
- [ ] Teste unitario: `exit 1` sem fail_on_error retorna Ok com exit_code=1
- [ ] Teste unitario: timeout em comando que demora

**Arquivos:**
- `src/steps/cmd.rs`

**Estimativa:** ~100 linhas

---

### Story 1.7: Step — agent (Claude Code CLI Integration)

**Como** desenvolvedor,
**Quero** invocar o Claude Code CLI como step de workflow,
**Para que** passos agenticos (implementar, corrigir) funcionem.

**Acceptance Criteria:**
- [ ] Struct `AgentExecutor` implementando trait `StepExecutor`
- [ ] Constroi command line: `claude -p --verbose --output-format stream-json`
- [ ] Flags opcionais: `--model`, `--append-system-prompt`, `--dangerously-skip-permissions`
- [ ] Session management: `--fork-session --resume <id>` quando session disponivel
- [ ] Envia prompt via stdin (write + drop)
- [ ] Parse streaming JSON linha por linha do stdout
- [ ] Tipos de mensagem: `AssistantMessage`, `ResultMessage`, `ToolUseMessage`, `TextMessage`
- [ ] Extrai do `ResultMessage`: response text, session_id, stats (tokens, cost, duration)
- [ ] Retorna `AgentOutput { response, session_id, stats }`
- [ ] Display de progresso no terminal: mostra TextMessage content em tempo real
- [ ] Display de tool use: mostra nome da tool sendo usada
- [ ] Timeout configuravel (default 600s)
- [ ] Teste unitario: mock do Claude CLI com script que emite JSON lines
- [ ] Teste de integracao (manual): roda `claude` real em sandbox

**Arquivos:**
- `src/steps/agent.rs`
- `src/claude/mod.rs`
- `src/claude/invocation.rs`
- `src/claude/messages.rs`
- `src/claude/session.rs`

**Estimativa:** ~450 linhas (step + claude integration)

**Nota:** Esta e a story mais critica do MVP. O parse do stream JSON precisa ser robusto.

---

### Story 1.8: Step — gate (Conditional Flow Control)

**Como** desenvolvedor,
**Quero** avaliar condicoes e controlar o fluxo,
**Para que** o engine possa decidir se continua, para, ou pula steps.

**Acceptance Criteria:**
- [ ] Struct `GateExecutor` implementando trait `StepExecutor`
- [ ] Renderiza template da `condition` com Tera
- [ ] Avalia resultado como boolean: "true"/"1"/"yes" = true, demais = false
- [ ] Se passed (true): executa acao de `on_pass` (break/continue/skip_next)
- [ ] Se failed (false): executa acao de `on_fail` (continue/fail/skip_next)
- [ ] `break` → retorna Err(ControlFlow::Break)
- [ ] `fail` → retorna Err(ControlFlow::Fail)
- [ ] `continue` → retorna Ok(GateOutput)
- [ ] `skip_next` → seta flag no contexto para pular proximo step
- [ ] Retorna `GateOutput { passed, message }`
- [ ] Teste unitario: condicao true com on_pass=break retorna Break
- [ ] Teste unitario: condicao false com on_fail=continue retorna Ok
- [ ] Teste unitario: condicao false com on_fail=fail retorna Fail
- [ ] Teste unitario: template com referencia a step anterior

**Arquivos:**
- `src/steps/gate.rs`

**Estimativa:** ~80 linhas

---

### Story 1.9: Step — repeat (Bounded Retry Loop)

**Como** desenvolvedor,
**Quero** executar um scope repetidamente ate um gate dar break ou atingir max_iterations,
**Para que** loops lint→fix→lint funcionem como na Stripe (max 2-3 rounds).

**Acceptance Criteria:**
- [ ] Struct `RepeatExecutor` implementando trait `StepExecutor`
- [ ] Le `scope` name e busca no `workflow.scopes`
- [ ] Le `max_iterations` (default: 3)
- [ ] Le `initial_value` (opcional, default: null)
- [ ] Loop: cria child context → executa todos steps do scope → verifica ControlFlow
- [ ] Se um step retorna Break: sai do loop, retorna Break.value como output
- [ ] Se max_iterations atingido sem Break: warning log, retorna ultimo output
- [ ] Cada iteracao recebe como `scope.value` o output da iteracao anterior
- [ ] Primeira iteracao recebe `initial_value` como `scope.value`
- [ ] Retorna `ScopeOutput { iterations: Vec<IterationOutput>, final_value }`
- [ ] Display: mostra "Iteration 1/3", "Iteration 2/3", etc.
- [ ] Teste unitario: scope com gate break na 1a iteracao → 1 iteracao
- [ ] Teste unitario: scope sem break → max_iterations iteracoes
- [ ] Teste unitario: scope_value flui entre iteracoes

**Arquivos:**
- `src/steps/repeat.rs`

**Estimativa:** ~120 linhas

---

### Story 1.10: MVP Integration — fix-issue Workflow

**Como** usuario,
**Quero** executar `minion execute fix-issue.yaml -- 247` e obter um PR,
**Para que** o MVP seja demonstravel end-to-end.

**Acceptance Criteria:**
- [ ] Arquivo `workflows/fix-issue.yaml` funcional (copia da secao 4 da arquitetura)
- [ ] Scopes `lint_fix` e `test_fix` definidos e funcionais
- [ ] Pipeline completo: fetch_issue → find_files → read_rules → plan(chat placeholder→cmd) → validate_plan → implement → lint_gate(repeat) → test_gate(repeat) → create_branch → commit → push → create_pr → result
- [ ] **Nota MVP**: step `chat` ainda nao existe; usar `cmd` com curl para Anthropic API como workaround, OU pular o step de planning e usar agent diretamente
- [ ] Output final mostra URL do PR criado
- [ ] Em caso de falha, mostra qual step falhou e por que
- [ ] `--verbose` mostra output de cada step
- [ ] `--quiet` mostra apenas resultado final ou erro
- [ ] Teste de integracao: workflow com cmd steps dummy (echo) roda sem erros
- [ ] README.md basico com instrucoes de build e uso

**Arquivos:**
- `workflows/fix-issue.yaml`
- `workflows/simple-test.yaml` (workflow de teste com echo)
- `README.md`

**Estimativa:** ~100 linhas (YAML + docs)

---

## Epic 2: Complete Engine

**Objetivo**: Feature-parity com Shopify Roast. Todos os step types funcionando, config hierarquica, chat step com API direta, sessoes do Claude Code, display rico no terminal.

**Pre-requisito**: Epic 1 completo
**Estimativa**: ~1,000 linhas adicionais de Rust

### Story 2.1: Step — chat (Direct LLM API)

**Como** desenvolvedor,
**Quero** chamar LLM APIs diretamente sem invocar Claude Code CLI,
**Para que** steps de planejamento e sumarizacao sejam rapidos e baratos.

**Acceptance Criteria:**
- [ ] Struct `ChatExecutor` implementando trait `StepExecutor`
- [ ] Suporte a providers: `anthropic`, `openai` (via config)
- [ ] Anthropic: POST para `https://api.anthropic.com/v1/messages` com reqwest
- [ ] OpenAI: POST para `https://api.openai.com/v1/chat/completions` com reqwest
- [ ] Config: model, temperature, max_tokens, api_key_env (nome da env var)
- [ ] Le API key da env var especificada (default: ANTHROPIC_API_KEY)
- [ ] Renderiza prompt template antes de enviar
- [ ] Retorna `ChatOutput { response, model, usage: TokenUsage }`
- [ ] TokenUsage: input_tokens, output_tokens
- [ ] Erro amigavel se API key nao encontrada
- [ ] Erro amigavel se API retorna erro (rate limit, invalid key, etc.)
- [ ] Timeout configuravel (default: 120s)
- [ ] Teste unitario: mock HTTP response
- [ ] Teste de integracao (manual): chamada real a Anthropic API

**Arquivos:**
- `src/steps/chat.rs`

**Estimativa:** ~150 linhas

---

### Story 2.2: Step — map (Collection Processing)

**Como** desenvolvedor,
**Quero** iterar sobre uma colecao executando um scope para cada item,
**Para que** analise de multiplos arquivos funcione (como security audit).

**Acceptance Criteria:**
- [ ] Struct `MapExecutor` implementando trait `StepExecutor`
- [ ] Le `items` como template → renderiza → split por linhas (ou parse JSON array)
- [ ] Le `scope` name e busca no `workflow.scopes`
- [ ] Le `parallel`: null=serial, 0=serial, N=N tasks concorrentes
- [ ] Modo serial: itera items sequencialmente, cada um com child context
- [ ] Modo paralelo: usa `tokio::sync::Semaphore(N)` + `tokio::spawn` + `join_all`
- [ ] Cada item e passado como `scope.value` no child context
- [ ] `scope.index` contem o indice (0-based)
- [ ] Resultados sempre retornados na ordem original (mesmo em paralelo)
- [ ] Se scope define `outputs:`, usa como output do item; senao, usa ultimo step
- [ ] Retorna `ScopeOutput { iterations: Vec<IterationOutput> }`
- [ ] Display: "Processing item 3/10: filename.ts"
- [ ] Teste unitario: 3 items serial
- [ ] Teste unitario: 3 items paralelo
- [ ] Teste unitario: ordem preservada

**Arquivos:**
- `src/steps/map.rs`

**Estimativa:** ~180 linhas

---

### Story 2.3: Step — parallel (Independent Concurrent Steps)

**Como** desenvolvedor,
**Quero** rodar steps independentes em paralelo,
**Para que** analises que nao dependem entre si rodem simultaneamente.

**Acceptance Criteria:**
- [ ] Struct `ParallelExecutor` implementando trait `StepExecutor`
- [ ] Le `steps` (lista de StepDef aninhados)
- [ ] Spawna cada step em `tokio::spawn`
- [ ] `join_all` espera todos terminarem
- [ ] Armazena resultado de cada sub-step no contexto principal: context["sub_step_name"] = output
- [ ] Se qualquer sub-step falha (Fail): cancela os demais e retorna erro
- [ ] Se qualquer sub-step da Skip: armazena Empty para ele, demais continuam
- [ ] Retorna output combinado (HashMap dos resultados)
- [ ] Teste unitario: 2 cmd steps paralelos
- [ ] Teste unitario: 1 falha cancela o outro

**Arquivos:**
- `src/steps/parallel.rs`

**Estimativa:** ~100 linhas

---

### Story 2.4: Step — call (Scope Invocation)

**Como** desenvolvedor,
**Quero** invocar um scope nomeado como um sub-workflow,
**Para que** logica reutilizavel possa ser organizada em scopes.

**Acceptance Criteria:**
- [ ] Struct `CallExecutor` implementando trait `StepExecutor`
- [ ] Le `scope` name e busca no `workflow.scopes`
- [ ] Cria child context com valor passado (se houver)
- [ ] Executa todos os steps do scope sequencialmente
- [ ] Se scope define `outputs:`, renderiza e retorna como output
- [ ] Senao, retorna output do ultimo step
- [ ] Retorna `ScopeOutput { iterations: vec![single], final_value }`
- [ ] Teste unitario: call de scope com 2 steps
- [ ] Teste unitario: call com outputs explicito

**Arquivos:**
- `src/steps/call.rs`

**Estimativa:** ~80 linhas

---

### Story 2.5: Config Manager — 4-Layer Merge

**Como** desenvolvedor,
**Quero** resolver config com 4 camadas de prioridade,
**Para que** config global, por tipo, por pattern e por step funcionem como no Roast.

**Acceptance Criteria:**
- [ ] Struct `ConfigManager` que recebe o bloco `config:` do workflow
- [ ] Metodo `resolve(step_name: &str, step_type: StepType, step_config: Option<Config>) -> StepConfig`
- [ ] Camada 1 (Global): `config.global.*` — aplica a todos
- [ ] Camada 2 (Type): `config.agent.*`, `config.cmd.*`, etc. — aplica ao tipo
- [ ] Camada 3 (Pattern): `config.patterns."regex".*` — aplica se nome do step match regex
- [ ] Camada 4 (Step inline): `step.config.*` — maior prioridade
- [ ] Merge: campo mais especifico sobrescreve menos especifico
- [ ] `StepConfig` com metodos: `get_str()`, `get_bool()`, `get_duration()`, `get_u64()`
- [ ] Teste unitario: global timeout=300 + step timeout=10 → resolve 10
- [ ] Teste unitario: pattern "lint.*" + step "lint_check" → match
- [ ] Teste unitario: pattern "lint.*" + step "test_run" → no match

**Arquivos:**
- `src/config/mod.rs`
- `src/config/schema.rs`
- `src/config/merge.rs`

**Estimativa:** ~180 linhas

---

### Story 2.6: Claude Code Session Management

**Como** desenvolvedor,
**Quero** reutilizar sessoes do Claude Code entre steps agent,
**Para que** o contexto do agente persista e as respostas sejam mais inteligentes.

**Acceptance Criteria:**
- [ ] Struct `SessionManager` armazena session_id por workflow run
- [ ] Apos primeiro AgentOutput: captura session_id e armazena
- [ ] Em steps agent subsequentes: passa `--fork-session --resume <id>`
- [ ] Config `session: shared` (default) ou `session: isolated` por step
- [ ] Se `isolated`: nao passa --resume (sessao nova)
- [ ] Se `shared`: reutiliza sessao
- [ ] Session ID acessivel via `{{ session_id }}` nos templates
- [ ] Teste unitario: session_id capturado de AgentOutput mock
- [ ] Teste unitario: session isolada nao envia --resume

**Arquivos:**
- `src/claude/session.rs` (expandir)

**Estimativa:** ~80 linhas

---

### Story 2.7: Rich Terminal Display

**Como** usuario,
**Quero** output bonito e informativo no terminal,
**Para que** eu possa acompanhar o progresso do workflow.

**Acceptance Criteria:**
- [ ] Spinner (indicatif) enquanto step esta rodando
- [ ] Step concluido: checkmark verde + nome + duracao
- [ ] Step falhou: X vermelho + nome + mensagem de erro
- [ ] Step skipped: seta amarela + nome + "skipped"
- [ ] Repeat: "Iteration 2/3" com indentacao
- [ ] Map: "Item 3/10: filename.ts" com indentacao
- [ ] Parallel: mostra sub-steps com indentacao
- [ ] Agent: stream de texto do Claude em tempo real
- [ ] Agent: tool use mostrado como "[tool: Read file.ts]"
- [ ] Resumo final: steps executados, tempo total, tokens usados (se agent), custo estimado
- [ ] `--quiet`: so imprime resultado final ou erro
- [ ] `--verbose`: imprime stdout/stderr de cada cmd step
- [ ] `--json`: output final como JSON (para integracao com outros tools)

**Arquivos:**
- `src/cli/display.rs` (expandir significativamente)

**Estimativa:** ~150 linhas

---

### Story 2.8: Step — template (Tera File Rendering)

**Como** desenvolvedor,
**Quero** renderizar arquivos .md.tera como steps,
**Para que** prompts longos possam ficar em arquivos separados.

**Acceptance Criteria:**
- [ ] Struct `TemplateStepExecutor` implementando trait `StepExecutor`
- [ ] Le arquivo de `prompts_dir/step_name.md.tera`
- [ ] Renderiza com Tera usando contexto atual
- [ ] Retorna `TemplateOutput { rendered: String }`
- [ ] Fallback: se step tem `prompt:` inline E arquivo existe, arquivo tem prioridade
- [ ] Teste unitario: arquivo .md.tera com {{ target }} renderiza
- [ ] Teste unitario: arquivo nao encontrado retorna erro descritivo

**Arquivos:**
- `src/steps/template_step.rs`

**Estimativa:** ~80 linhas

---

## Epic 3: Polish & Production

**Objetivo**: Tornar o engine pronto para uso real. Docker Sandbox, dry-run, resume, testes de integracao, documentacao.

**Pre-requisito**: Epic 2 completo
**Estimativa**: ~800 linhas adicionais

### Story 3.1: Docker Sandbox Integration

**Como** usuario,
**Quero** rodar workflows ou steps agent em Docker Sandbox,
**Para que** tenha isolamento tipo Stripe devbox.

**Acceptance Criteria:**
- [ ] Flag `--sandbox` no CLI
- [ ] Modo 1 (workflow inteiro): cria sandbox → copia workspace → executa engine dentro → copia resultados → destroi sandbox
- [ ] Modo 2 (agent-only): `config.agent.sandbox: true` → so steps agent rodam no sandbox
- [ ] Modo 3 (devbox): `config.global.sandbox.enabled: true` com image, workspace, network, resources
- [ ] Detecta se Docker Desktop + Docker Sandbox esta disponivel
- [ ] Erro amigavel se Docker Sandbox nao disponivel
- [ ] Config de rede: allow/deny lists para dominos
- [ ] Config de recursos: cpus, memory
- [ ] Limpeza automatica do sandbox apos execucao (mesmo em caso de erro)
- [ ] Teste unitario: mock de docker commands
- [ ] Documentacao: requisitos (Docker Desktop 4.40+) e exemplos

**Arquivos:**
- `src/sandbox/mod.rs`
- `src/sandbox/docker.rs`
- `src/sandbox/config.rs`

**Estimativa:** ~200 linhas

---

### Story 3.2: Dry-Run Mode

**Como** usuario,
**Quero** ver quais steps seriam executados sem executar nada,
**Para que** possa validar e entender o workflow antes de rodar.

**Acceptance Criteria:**
- [ ] Flag `--dry-run` no CLI
- [ ] Percorre todos os steps do workflow mostrando: nome, tipo, command/prompt (renderizado com templates que tem valores, templates sem valor mostram placeholder)
- [ ] Mostra scopes e como repeat/map/call os invocariam
- [ ] Mostra config resolvida para cada step (4 camadas mergeadas)
- [ ] Nao executa nenhum step
- [ ] Nao faz chamadas a rede ou processos
- [ ] Output formatado como arvore visual
- [ ] Teste unitario: dry-run de workflow com todos step types

**Arquivos:**
- Modificacoes em `src/engine/executor.rs`
- Modificacoes em `src/cli/commands.rs`

**Estimativa:** ~100 linhas

---

### Story 3.3: Resume From Step

**Como** usuario,
**Quero** retomar um workflow a partir de um step especifico,
**Para que** nao precise re-executar steps que ja passaram.

**Acceptance Criteria:**
- [ ] Flag `--resume <step_name>` no CLI
- [ ] Salva state de cada step concluido em arquivo JSON (`/tmp/minion-<workflow>-<timestamp>.state.json`)
- [ ] Ao resumir: carrega state, pula steps ate encontrar o especificado, executa a partir dele
- [ ] Steps pulados tem seus outputs carregados do state file
- [ ] Se step de resume nao encontrado: erro amigavel
- [ ] State file inclui: outputs de cada step, session_id, timestamp
- [ ] `minion execute --resume implement fix-issue.yaml -- 247`
- [ ] Teste unitario: resume pula corretamente steps anteriores

**Arquivos:**
- `src/engine/state.rs`
- Modificacoes em `src/engine/executor.rs`

**Estimativa:** ~120 linhas

---

### Story 3.4: JSON Output Mode

**Como** usuario,
**Quero** output do workflow em formato JSON,
**Para que** possa integrar com outros tools e pipelines.

**Acceptance Criteria:**
- [ ] Flag `--json` no CLI
- [ ] Suprime todo output de display (spinners, cores, progress)
- [ ] Ao final: imprime JSON completo com: workflow_name, status, steps (nome, tipo, status, duracao, output resumido), total_duration, total_tokens, total_cost
- [ ] Erro tambem em JSON: { "error": "message", "step": "name", "type": "Fail" }
- [ ] JSON e valido e parseavel (testado com jq)
- [ ] Teste unitario: output JSON e valido

**Arquivos:**
- Modificacoes em `src/cli/display.rs`
- `src/events.rs`

**Estimativa:** ~80 linhas

---

### Story 3.5: CLI — init & list & inspect Commands

**Como** usuario,
**Quero** comandos auxiliares para gerenciar workflows,
**Para que** possa criar, listar e inspecionar workflows facilmente.

**Acceptance Criteria:**
- [ ] `minion init <name>` — cria workflow YAML a partir de template interativo
- [ ] Templates disponiveis: blank, fix-issue, code-review, security-audit
- [ ] `minion list` — lista workflows no diretorio atual e em `~/.minion/workflows/`
- [ ] `minion list` mostra: nome, descricao, numero de steps
- [ ] `minion inspect <workflow.yaml>` — mostra config resolvida, scopes, grafo de dependencias
- [ ] `minion inspect` inclui output do dry-run resumido
- [ ] Teste unitario: init cria arquivo valido

**Arquivos:**
- Modificacoes em `src/cli/commands.rs`
- `src/cli/init_templates.rs`

**Estimativa:** ~150 linhas

---

### Story 3.6: Integration Tests

**Como** desenvolvedor,
**Quero** suite de testes de integracao,
**Para que** tenha confianca de que o engine funciona end-to-end.

**Acceptance Criteria:**
- [ ] Test fixture: workflow com 3 cmd steps → verifica output sequencial
- [ ] Test fixture: workflow com gate break → verifica que steps apos gate nao rodam
- [ ] Test fixture: workflow com repeat → verifica numero de iteracoes
- [ ] Test fixture: workflow com map serial → verifica outputs na ordem
- [ ] Test fixture: workflow com scope invalido → verifica erro de validacao
- [ ] Test fixture: workflow com template → verifica rendering
- [ ] Test fixture: workflow com config 4 camadas → verifica merge
- [ ] Mock do Claude Code CLI: script bash que emite JSON lines esperado
- [ ] Test fixture: workflow com agent (mocked) → verifica parse
- [ ] CI: `cargo test` roda todos os testes (unit + integration)
- [ ] Coverage > 70% dos modulos core (engine, steps, context)

**Arquivos:**
- `tests/integration/mod.rs`
- `tests/integration/cmd_test.rs`
- `tests/integration/gate_test.rs`
- `tests/integration/repeat_test.rs`
- `tests/integration/map_test.rs`
- `tests/integration/config_test.rs`
- `tests/fixtures/` (multiplos YAMLs de teste)
- `tests/mocks/claude_mock.sh`

**Estimativa:** ~300 linhas

---

### Story 3.7: Documentation

**Como** usuario e contribuidor,
**Quero** documentacao completa,
**Para que** possa usar e contribuir ao engine.

**Acceptance Criteria:**
- [ ] README.md: overview, install, quick start, architecture diagram
- [ ] docs/YAML-SPEC.md: especificacao completa do formato YAML com todos step types
- [ ] docs/STEP-TYPES.md: cada step type com exemplos
- [ ] docs/CONFIG.md: sistema de config 4 camadas com exemplos
- [ ] docs/DOCKER-SANDBOX.md: como usar com Docker Sandbox
- [ ] docs/EXAMPLES.md: catalogo de workflows de exemplo
- [ ] Doc comments (`///`) em todos os tipos e metodos publicos
- [ ] `cargo doc --open` gera docs navegaveis

**Arquivos:**
- `README.md` (expandir)
- `docs/YAML-SPEC.md`
- `docs/STEP-TYPES.md`
- `docs/CONFIG.md`
- `docs/DOCKER-SANDBOX.md`
- `docs/EXAMPLES.md`

**Estimativa:** N/A (documentacao)

---

## Epic 4: Distribution

**Objetivo**: Tornar o Minion Engine facilmente instalavel e distribuivel. Colecao de workflows prontos.

**Pre-requisito**: Epic 3 completo

### Story 4.1: cargo install

**Como** usuario Rust,
**Quero** instalar via `cargo install minion-engine`,
**Para que** nao precise clonar o repositorio.

**Acceptance Criteria:**
- [ ] `Cargo.toml` com metadata completa: description, license (MIT), repository, keywords, categories
- [ ] `cargo publish --dry-run` funciona sem erros
- [ ] Binary name: `minion`
- [ ] Publicado em crates.io
- [ ] `cargo install minion-engine` instala e `minion --version` funciona

**Arquivos:**
- Modificacoes em `Cargo.toml`

---

### Story 4.2: Pre-compiled Binaries

**Como** usuario nao-Rust,
**Quero** baixar um binario pronto para meu OS,
**Para que** nao precise instalar toolchain Rust.

**Acceptance Criteria:**
- [ ] GitHub Actions workflow: build + release para linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64, windows-x86_64
- [ ] Release automatico ao criar tag `v*`
- [ ] Binarios assinados (opcional)
- [ ] Download direto: `curl -L https://github.com/.../releases/latest/download/minion-macos-aarch64 -o minion`
- [ ] Instrucoes de instalacao no README

**Arquivos:**
- `.github/workflows/release.yml`

---

### Story 4.3: Homebrew Formula

**Como** usuario macOS,
**Quero** instalar via `brew install minion-engine`,
**Para que** tenha instalacao e update faceis.

**Acceptance Criteria:**
- [ ] Homebrew formula funcional (tap ou core)
- [ ] `brew install <tap>/minion-engine` instala
- [ ] `brew upgrade minion-engine` atualiza
- [ ] Formula aponta para GitHub Releases

**Arquivos:**
- `Formula/minion-engine.rb` (ou repo separado de tap)

---

### Story 4.4: Workflow Gallery

**Como** usuario,
**Quero** uma colecao de workflows prontos para uso,
**Para que** possa comecar rapidamente sem escrever YAML do zero.

**Acceptance Criteria:**
- [ ] `workflows/fix-issue.yaml` — estilo Stripe Minions (fetch issue → plan → implement → lint → test → PR)
- [ ] `workflows/code-review.yaml` — diff → agent review → chat summary
- [ ] `workflows/security-audit.yaml` — find files → map parallel audit → report
- [ ] `workflows/generate-docs.yaml` — find source files → agent generate docs → save
- [ ] `workflows/refactor.yaml` — plan → implement → lint → test
- [ ] `workflows/flaky-test-fix.yaml` — find flaky → agent analyze → fix → test
- [ ] `workflows/weekly-report.yaml` — git log → chat summarize → format
- [ ] Cada workflow com comentarios explicativos
- [ ] `minion init` oferece esses templates

**Arquivos:**
- `workflows/` (7 arquivos YAML)
- `prompts/` (templates .md.tera correspondentes)

---

## Dependency Graph

```
Story 1.1 (bootstrap)
  └──► Story 1.2 (parser)
         └──► Story 1.3 (validator)
         └──► Story 1.4 (context + templates)
                └──► Story 1.5 (engine core)
                       ├──► Story 1.6 (cmd step)
                       ├──► Story 1.7 (agent step) ──► Story 2.6 (sessions)
                       ├──► Story 1.8 (gate step)
                       └──► Story 1.9 (repeat step)
                              └──► Story 1.10 (MVP integration)

Story 1.5 (engine core)
  └──► Story 2.1 (chat step)
  └──► Story 2.2 (map step)
  └──► Story 2.3 (parallel step)
  └──► Story 2.4 (call step)
  └──► Story 2.5 (config manager)
  └──► Story 2.7 (display)
  └──► Story 2.8 (template step)

Epic 2 completo
  └──► Story 3.1 (docker sandbox)
  └──► Story 3.2 (dry-run)
  └──► Story 3.3 (resume)
  └──► Story 3.4 (json output)
  └──► Story 3.5 (CLI commands)
  └──► Story 3.6 (integration tests)
  └──► Story 3.7 (documentation)

Epic 3 completo
  └──► Story 4.1 (cargo install)
  └──► Story 4.2 (binaries)
  └──► Story 4.3 (homebrew)
  └──► Story 4.4 (workflow gallery)
```

---

## Summary

| Epic | Stories | Linhas estimadas | Prazo |
|------|---------|-----------------|-------|
| E1: MVP Foundation | 10 stories | ~1,200 | 1 semana |
| E2: Complete Engine | 8 stories | ~1,000 | 2 semanas |
| E3: Polish & Production | 7 stories | ~800 | 1 semana |
| E4: Distribution | 4 stories | N/A | 1 semana |
| **Total** | **29 stories** | **~3,000 linhas** | **~5 semanas** |

**Critical Path**: 1.1 → 1.2 → 1.4 → 1.5 → 1.7 → 1.9 → 1.10 (MVP funcional)
