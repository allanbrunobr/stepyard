# Minion Engine — Arquitetura Completa

## Um workflow engine em Rust que orquestra Claude Code

> Inspirado em: Stripe Minions (Blueprints) + Shopify Roast (Cogs)
> Linguagem: Rust
> Workflow format: YAML
> Agente: Claude Code CLI

---

## 1. Visao Geral

```
                         ┌──────────────────────────────────────┐
                         │          MINION ENGINE (Rust)         │
                         │                                      │
  workflow.yaml ────────►│  Parser ──► Validator ──► Engine     │
  target + args ────────►│                            │         │
                         │              ┌─────────────┘         │
                         │              ▼                       │
                         │     ┌─────────────────┐              │
                         │     │  Step Executor   │              │
                         │     │  (dispatch loop) │              │
                         │     └────────┬────────┘              │
                         │              │                       │
                         │   ┌──────────┼──────────┐            │
                         │   ▼          ▼          ▼            │
                         │ ┌─────┐  ┌───────┐  ┌──────┐        │
                         │ │ cmd │  │ agent │  │ chat │  ...    │
                         │ └──┬──┘  └───┬───┘  └──┬───┘        │
                         │    │         │         │             │
                         │    ▼         ▼         ▼             │
                         │ ┌─────────────────────────┐          │
                         │ │    Context Store         │          │
                         │ │ (outputs de cada step)   │          │
                         │ └─────────────────────────┘          │
                         └──────────────────────────────────────┘
                                        │
                         ┌──────────────┼──────────────┐
                         ▼              ▼              ▼
                    Shell (sh)    Claude Code CLI   LLM API
                    npm, git,     claude -p          Anthropic
                    gh, curl...   (filesystem)       OpenAI
```

---

## 2. Principio Fundamental

```
O ENGINE decide o que roda.
O AGENTE (Claude) so trabalha quando o engine manda.

Engine: "Claude, implemente o plano"
Claude: *implementa*
Engine: "Obrigado. Agora EU rodo lint."        ← Claude nao tem voz
Engine: "Lint falhou. Claude, corrija."
Claude: *corrige*
Engine: "EU rodo lint de novo."
Engine: "Passou. EU rodo testes."
Engine: "Tudo OK. EU crio o PR."
```

---

## 3. Step Types (equivalentes aos Cogs do Roast)

```
┌─────────────────────────────────────────────────────────────┐
│                      STEP TYPES                             │
├──────────────┬──────────────────────────────────────────────┤
│              │                                              │
│  EXECUTION   │  cmd ........ Shell command (deterministico) │
│  (fazem      │  agent ...... Claude Code CLI (agentico)     │
│   trabalho)  │  chat ....... LLM API call (agentico leve)  │
│              │  template ... Render arquivo .md.tera        │
│              │                                              │
├──────────────┼──────────────────────────────────────────────┤
│              │                                              │
│  CONTROL     │  gate ....... Avalia condicao → break/skip   │
│  FLOW        │  repeat ..... Loop com max_iterations        │
│  (controlam  │  map ........ Itera colecao (serial/paralelo)│
│   a ordem)   │  parallel ... Steps independentes em paralelo│
│              │  call ....... Invoca scope nomeado           │
│              │                                              │
└──────────────┴──────────────────────────────────────────────┘
```

### Mapeamento Roast → Minion Engine

| Roast (Ruby)        | Minion Engine (Rust/YAML)     |
|---------------------|-------------------------------|
| `cmd(:name) { }` | `type: cmd`                     |
| `agent(:name) { }` | `type: agent`                 |
| `chat(:name) { }` | `type: chat`                   |
| `ruby(:name) { }` | `type: cmd` (ou script inline)  |
| `call(run: :x)` | `type: call, scope: x`           |
| `map(run: :x)` | `type: map, scope: x`             |
| `repeat(run: :x)` | `type: repeat, scope: x`       |
| `skip!` / `break!` | `type: gate`                   |
| `outputs { }` | `outputs:` no scope                 |
| ERB templates       | Tera templates `{{}}`          |

---

## 4. YAML Workflow Format — Especificacao Completa

```yaml
# ============================================================
# HEADER
# ============================================================
name: fix-github-issue
version: 1
description: "Recebe uma issue, planeja, implementa, valida e cria PR"

# ============================================================
# CONFIG (4 camadas: global → tipo → pattern → step inline)
# ============================================================
config:
  # Camada 1: Global (todos os steps)
  global:
    timeout: 300s
    working_directory: "."

  # Camada 2: Por tipo de step
  agent:
    command: claude
    flags: ["-p", "--output-format", "stream-json"]
    permissions: skip          # skip | apply
    model: claude-sonnet-4-20250514
    sandbox: false             # true = roda dentro de Docker Sandbox
    system_prompt_append: |
      Follow project conventions. Do not run tests yourself.

  chat:
    provider: anthropic        # anthropic | openai | google
    model: claude-sonnet-4-20250514
    api_key_env: ANTHROPIC_API_KEY
    temperature: 0.3
    max_tokens: 4096

  cmd:
    fail_on_error: true
    timeout: 60s
    shell: "/bin/bash"

  # Camada 3: Por pattern de nome (regex)
  patterns:
    "lint.*":
      timeout: 30s
    "test.*":
      timeout: 600s
    ".*_quick":
      timeout: 10s

# ============================================================
# PROMPTS (arquivos externos, opcionais)
# ============================================================
prompts_dir: ./prompts        # diretorio com arquivos .md.tera

# ============================================================
# SCOPES (sub-workflows nomeados, usados por call/map/repeat)
# ============================================================
scopes:

  # ── Scope: lint fix loop ──────────────────────────────────
  lint_fix:
    steps:
      - name: lint
        type: cmd
        run: "npm run lint 2>&1"
        config:
          fail_on_error: false

      - name: check_lint
        type: gate
        condition: "{{ steps.lint.exit_code == 0 }}"
        on_pass: break
        message: "Lint passed"

      - name: fix_lint
        type: agent
        prompt: |
          Fix ONLY these lint errors. Do not change any logic.
          Do not add new features. Just fix the errors:

          {{ steps.lint.stdout }}

  # ── Scope: test fix loop ──────────────────────────────────
  test_fix:
    steps:
      - name: test
        type: cmd
        run: "npm test 2>&1"
        config:
          fail_on_error: false
          timeout: 300s

      - name: check_test
        type: gate
        condition: "{{ steps.test.exit_code == 0 }}"
        on_pass: break
        message: "Tests passed"

      - name: fix_test
        type: agent
        prompt: |
          Fix ONLY these test failures. Do not modify test expectations
          unless the test is clearly wrong:

          {{ steps.test.stdout }}

  # ── Scope: analyze single file (para uso com map) ────────
  analyze_file:
    # scope.value = nome do arquivo (passado pelo map)
    steps:
      - name: read_file
        type: cmd
        run: "cat {{ scope.value }}"

      - name: review
        type: agent
        prompt: |
          Review this file for security issues:
          File: {{ scope.value }}
          Content:
          {{ steps.read_file.stdout }}

    # Output explicito do scope
    outputs: "{{ steps.review.response }}"

# ============================================================
# STEPS (pipeline principal)
# ============================================================
steps:

  # ── 1. Context Curation (deterministico) ──────────────────
  - name: fetch_issue
    type: cmd
    run: >
      gh issue view {{ target }}
      --json title,body,comments,labels
      -q '{title: .title, body: .body, labels: [.labels[].name],
           comments: [.comments[].body]}'

  - name: find_relevant_files
    type: cmd
    run: >
      gh issue view {{ target }} --json body -q .body |
      grep -oE '[a-zA-Z0-9_/]+\.(ts|js|py|rs|rb)' |
      sort -u | head -20
    config:
      fail_on_error: false

  - name: read_project_rules
    type: cmd
    run: "cat CLAUDE.md 2>/dev/null || cat .cursorrules 2>/dev/null || echo 'No rules found'"
    config:
      fail_on_error: false

  # ── 2. Planning (agentico — LLM leve) ────────────────────
  - name: plan
    type: chat
    prompt: |
      You are a senior software architect. Create a detailed
      implementation plan for the following GitHub issue.

      ## Issue
      {{ steps.fetch_issue.stdout }}

      ## Relevant Files
      {{ steps.find_relevant_files.stdout }}

      ## Project Rules
      {{ steps.read_project_rules.stdout }}

      ## Instructions
      - List each file to modify/create
      - For each file, describe the exact changes
      - Include test strategy
      - Be specific, not vague

      Output a structured plan in markdown.

  # ── 3. Plan Validation (deterministico) ───────────────────
  - name: validate_plan
    type: gate
    condition: "{{ steps.plan.response | length > 100 }}"
    on_fail: fail
    message: "Plan too short or empty — aborting"

  # ── 4. Implementation (agentico — Claude Code full) ───────
  - name: implement
    type: agent
    prompt: |
      Implement the following plan exactly as specified.
      Do NOT run tests, lint, or any validation commands.
      Just write the code.

      ## Plan
      {{ steps.plan.response }}

      ## Project Rules
      {{ steps.read_project_rules.stdout }}
    config:
      model: claude-sonnet-4-20250514
      timeout: 600s

  # ── 5. Lint Gate (deterministico + retry agentico) ────────
  - name: lint_gate
    type: repeat
    scope: lint_fix
    max_iterations: 3

  # ── 6. Test Gate (deterministico + retry agentico) ────────
  - name: test_gate
    type: repeat
    scope: test_fix
    max_iterations: 2

  # ── 7. Create PR (deterministico) ─────────────────────────
  - name: create_branch
    type: cmd
    run: "git checkout -b minion/issue-{{ target }}-$(date +%s)"

  - name: commit
    type: cmd
    run: |
      git add -A
      git commit -m "fix: resolve issue #{{ target }}

      Implemented by Minion Engine
      Plan: {{ steps.plan.response | truncate(200) }}"

  - name: push
    type: cmd
    run: "git push -u origin HEAD"

  - name: create_pr
    type: cmd
    run: |
      gh pr create \
        --title "Minion: Fix #{{ target }}" \
        --body "## Plan
      {{ steps.plan.response }}

      ---
      *Generated by Minion Engine*"

  # ── 8. Result ─────────────────────────────────────────────
  - name: result
    type: cmd
    run: "gh pr view --json url -q .url"
```

---

## 5. Arquitetura Rust — Modulos

```
minion-engine/
│
├── Cargo.toml
│
├── src/
│   │
│   ├── main.rs                          # Entry point
│   ├── lib.rs                           # Public API (para uso como library)
│   │
│   │   ┌─────────────────────────────────────────────┐
│   │   │  CAMADA 1: Interface                        │
│   │   └─────────────────────────────────────────────┘
│   ├── cli/
│   │   ├── mod.rs                       # CLI com clap
│   │   ├── commands.rs                  # execute, validate, list, init
│   │   └── display.rs                   # Terminal output (colored, spinners)
│   │
│   │   ┌─────────────────────────────────────────────┐
│   │   │  CAMADA 2: Workflow (definicao)             │
│   │   └─────────────────────────────────────────────┘
│   ├── workflow/
│   │   ├── mod.rs                       # Workflow struct
│   │   ├── schema.rs                    # Serde structs para YAML
│   │   ├── parser.rs                    # YAML file → Workflow
│   │   └── validator.rs                 # Valida antes de executar
│   │
│   │   ┌─────────────────────────────────────────────┐
│   │   │  CAMADA 3: Engine (execucao)                │
│   │   └─────────────────────────────────────────────┘
│   ├── engine/
│   │   ├── mod.rs                       # Engine struct (orquestrador)
│   │   ├── executor.rs                  # Dispatch loop (step por step)
│   │   ├── context.rs                   # Context store (arvore de outputs)
│   │   └── template.rs                  # Tera template rendering
│   │
│   │   ┌─────────────────────────────────────────────┐
│   │   │  CAMADA 4: Steps (tipos de step)            │
│   │   └─────────────────────────────────────────────┘
│   ├── steps/
│   │   ├── mod.rs                       # StepExecutor trait + StepOutput enum
│   │   ├── cmd.rs                       # Shell commands
│   │   ├── agent.rs                     # Claude Code CLI
│   │   ├── chat.rs                      # LLM API direta
│   │   ├── gate.rs                      # Condicional (break/skip/fail)
│   │   ├── repeat.rs                    # Loop com max_iterations
│   │   ├── map.rs                       # Colecao (serial/paralelo)
│   │   ├── parallel.rs                  # Steps independentes em paralelo
│   │   ├── call.rs                      # Invoca scope nomeado
│   │   └── template_step.rs             # Render template .md.tera
│   │
│   │   ┌─────────────────────────────────────────────┐
│   │   │  CAMADA 5: Integracao Claude Code           │
│   │   └─────────────────────────────────────────────┘
│   ├── claude/
│   │   ├── mod.rs                       # Interface publica
│   │   ├── invocation.rs                # Spawn processo + parse stream
│   │   ├── messages.rs                  # Tipos de mensagem JSON
│   │   └── session.rs                   # Gerenciamento de sessao
│   │
│   │   ┌─────────────────────────────────────────────┐
│   │   │  CAMADA 6: Configuracao                     │
│   │   └─────────────────────────────────────────────┘
│   ├── config/
│   │   ├── mod.rs                       # ConfigManager
│   │   ├── schema.rs                    # Serde structs para config
│   │   └── merge.rs                     # Merge 4 camadas
│   │
│   │   ┌─────────────────────────────────────────────┐
│   │   │  CAMADA 7: Transversais                     │
│   │   └─────────────────────────────────────────────┘
│   ├── error.rs                         # Tipos de erro (thiserror)
│   ├── control_flow.rs                  # Skip, Fail, Break, Next
│   └── events.rs                        # Event system para logging
│
├── workflows/                           # Exemplos de workflow
│   ├── fix-issue.yaml
│   ├── code-review.yaml
│   ├── security-audit.yaml
│   ├── generate-docs.yaml
│   └── weekly-report.yaml
│
└── tests/
    ├── unit/                            # Testes unitarios
    ├── integration/                     # Testes com workflows reais
    └── fixtures/                        # YAML + outputs esperados
```

---

## 6. Tipos Core em Rust

### 6.1 Step Output

```rust
/// Resultado de qualquer step executado
#[derive(Debug, Clone, Serialize)]
pub enum StepOutput {
    Cmd(CmdOutput),
    Agent(AgentOutput),
    Chat(ChatOutput),
    Gate(GateOutput),
    Scope(ScopeOutput),       // repeat, map, call
    Template(TemplateOutput),
    Empty,                     // step pulado
}

#[derive(Debug, Clone, Serialize)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub duration: Duration,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentOutput {
    pub response: String,
    pub session_id: Option<String>,
    pub stats: AgentStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentStats {
    pub duration: Duration,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub turns: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatOutput {
    pub response: String,
    pub model: String,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateOutput {
    pub passed: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScopeOutput {
    pub iterations: Vec<IterationOutput>,
    pub final_value: Option<Box<StepOutput>>,
}

/// Accessors universais (todos os steps podem responder)
impl StepOutput {
    /// Texto principal do output (stdout, response, rendered)
    pub fn text(&self) -> &str { ... }

    /// Parse como JSON
    pub fn json(&self) -> Result<Value> { ... }

    /// Divide em linhas
    pub fn lines(&self) -> Vec<&str> { ... }

    /// Exit code (so cmd, 0 para outros)
    pub fn exit_code(&self) -> i32 { ... }

    /// Sucesso? (exit_code==0 para cmd, passed para gate, etc)
    pub fn success(&self) -> bool { ... }
}
```

### 6.2 Step Executor Trait

```rust
/// Trait que cada tipo de step implementa
#[async_trait]
pub trait StepExecutor: Send + Sync {
    /// Executa o step com o input renderizado e contexto atual
    async fn execute(
        &self,
        step_def: &StepDef,       // definicao YAML do step
        config: &StepConfig,       // config mergeada (4 camadas)
        context: &Context,         // outputs dos steps anteriores
    ) -> Result<StepOutput, StepError>;
}
```

### 6.3 Control Flow

```rust
/// Excecoes de controle de fluxo (como Roast)
#[derive(Debug)]
pub enum ControlFlow {
    /// Pula o step atual sem erro
    Skip { message: String },

    /// Falha o step e potencialmente aborta
    Fail { message: String },

    /// Sai do loop repeat/map atual
    Break { message: String, value: Option<StepOutput> },

    /// Pula para proxima iteracao do repeat/map
    Next { message: String },
}
```

### 6.4 Context Store

```rust
/// Arvore de contexto que armazena outputs dos steps
pub struct Context {
    /// Steps executados neste scope
    steps: HashMap<String, StepOutput>,

    /// Variaveis do workflow (target, args, kwargs)
    variables: HashMap<String, Value>,

    /// Contexto pai (para scopes aninhados)
    parent: Option<Arc<Context>>,

    /// Valor passado pelo scope (repeat/map/call)
    scope_value: Option<Value>,
    scope_index: usize,
}

impl Context {
    /// Busca output de um step (olha parent se nao achar)
    pub fn get_step(&self, name: &str) -> Option<&StepOutput> { ... }

    /// Converte para HashMap compativel com Tera
    pub fn to_tera_context(&self) -> tera::Context { ... }

    /// Cria contexto filho (para scopes)
    pub fn child(&self, scope_value: Value, index: usize) -> Context { ... }
}
```

---

## 7. Fluxo de Execucao Detalhado

```
minion execute fix-issue.yaml -- 247
│
▼
┌─────────────────────────────────────────────────────────────┐
│ 1. CLI PARSE (clap)                                        │
│    workflow_path = "fix-issue.yaml"                         │
│    target = "247"                                           │
│    args = {}                                                │
└──────────────────────┬──────────────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────────────┐
│ 2. PARSE YAML (serde_yaml)                                 │
│    workflow.yaml → WorkflowDef {                            │
│      name, config, scopes, steps, prompts_dir              │
│    }                                                        │
└──────────────────────┬──────────────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────────────┐
│ 3. VALIDATE                                                 │
│    ✓ Todos os scopes referenciados existem                  │
│    ✓ Templates sao validos (Tera syntax)                    │
│    ✓ Config types sao validos                               │
│    ✓ Sem ciclos em call → scope → call                      │
└──────────────────────┬──────────────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────────────┐
│ 4. CREATE ENGINE                                            │
│    engine = Engine {                                        │
│      workflow: parsed,                                      │
│      config_manager: ConfigManager::new(config),            │
│      context: Context::new(target="247", args={}),          │
│    }                                                        │
└──────────────────────┬──────────────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────────────┐
│ 5. EXECUTE LOOP                                             │
│                                                             │
│    for step in workflow.steps:                               │
│    ┌─────────────────────────────────────────────────────┐  │
│    │ a. Resolve config (merge 4 camadas)                 │  │
│    │    merged = global + type + pattern + step.config    │  │
│    │                                                     │  │
│    │ b. Render templates no step                         │  │
│    │    "gh issue view {{ target }}"                     │  │
│    │    → "gh issue view 247"                            │  │
│    │                                                     │  │
│    │ c. Dispatch para executor                           │  │
│    │    match step.type:                                  │  │
│    │      "cmd"     → CmdExecutor::execute()             │  │
│    │      "agent"   → AgentExecutor::execute()           │  │
│    │      "chat"    → ChatExecutor::execute()            │  │
│    │      "gate"    → GateExecutor::execute()            │  │
│    │      "repeat"  → RepeatExecutor::execute()          │  │
│    │      "map"     → MapExecutor::execute()             │  │
│    │      "call"    → CallExecutor::execute()            │  │
│    │                                                     │  │
│    │ d. Handle resultado                                 │  │
│    │    Ok(output) → context.store(step.name, output)    │  │
│    │    Err(Skip)  → context.store(step.name, Empty)     │  │
│    │    Err(Fail)  → abort se fail_on_error              │  │
│    │    Err(Break) → propaga para repeat/map pai         │  │
│    └─────────────────────────────────────────────────────┘  │
│                                                             │
└──────────────────────┬──────────────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────────────┐
│ 6. OUTPUT FINAL                                             │
│    Ultimo step ou campo `outputs:` do workflow              │
│    Imprime resultado + estatisticas                         │
└─────────────────────────────────────────────────────────────┘
```

---

## 8. Como Cada Step Type Funciona

### 8.1 cmd (Deterministico)

```
Engine                               Shell
  │                                    │
  ├── render template do `run`         │
  ├── spawn processo ─────────────────►│ bash -c "npm run lint 2>&1"
  │                                    │
  │   ◄─── stdout (stream) ───────────┤
  │   ◄─── stderr (stream) ───────────┤
  │   ◄─── exit code ─────────────────┤
  │                                    │
  ├── CmdOutput { stdout, stderr,      │
  │     exit_code, duration }          │
  │                                    │
  ├── if fail_on_error && exit != 0:   │
  │     return Err(Fail)               │
  │                                    │
  └── context.store("lint", output)    │
```

**Implementacao Rust:**
```rust
pub struct CmdExecutor;

#[async_trait]
impl StepExecutor for CmdExecutor {
    async fn execute(
        &self, step: &StepDef, config: &StepConfig, ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let command = ctx.render_template(&step.run.as_ref().unwrap())?;
        let shell = config.get_str("shell").unwrap_or("/bin/bash");
        let timeout = config.get_duration("timeout").unwrap_or(Duration::from_secs(60));
        let working_dir = config.get_str("working_directory");

        let mut cmd = tokio::process::Command::new(shell);
        cmd.arg("-c").arg(&command);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        let start = Instant::now();
        let output = tokio::time::timeout(timeout, cmd.output()).await??;

        let result = CmdOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
            duration: start.elapsed(),
        };

        if config.get_bool("fail_on_error") && result.exit_code != 0 {
            return Err(StepError::Fail(format!(
                "Command failed (exit {}): {}", result.exit_code, result.stderr
            )));
        }

        Ok(StepOutput::Cmd(result))
    }
}
```

### 8.2 agent (Agentico — Claude Code CLI)

```
Engine                            Claude Code CLI
  │                                     │
  ├── render template do prompt         │
  ├── build command line                │
  │   claude -p                         │
  │   --output-format stream-json       │
  │   --model claude-sonnet-4-20250514      │
  │   --append-system-prompt "..."      │
  │   --dangerously-skip-permissions    │
  │   --fork-session --resume <id>      │
  │                                     │
  ├── spawn processo ──────────────────►│
  │   stdin: prompt ───────────────────►│
  │                                     │
  │   ◄─── stream JSON line by line ───┤
  │        {"type":"assistant",...}      │
  │        {"type":"tool_use",...}       │  Claude edita arquivos,
  │        {"type":"text",...}           │  roda comandos, etc.
  │        {"type":"result",...}         │
  │                                     │
  ├── parse cada linha JSON             │
  │   ├─ TextMessage → exibe progresso  │
  │   ├─ ToolUseMessage → log           │
  │   └─ ResultMessage → captura:       │
  │       response, session_id, stats   │
  │                                     │
  ├── AgentOutput { response,           │
  │     session_id, stats }             │
  │                                     │
  └── context.store("implement", out)   │
```

**Implementacao Rust (simplificada):**
```rust
pub struct AgentExecutor;

#[async_trait]
impl StepExecutor for AgentExecutor {
    async fn execute(
        &self, step: &StepDef, config: &StepConfig, ctx: &Context,
    ) -> Result<StepOutput, StepError> {
        let prompt = ctx.render_template(&step.prompt.as_ref().unwrap())?;

        let mut args = vec![
            "-p".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(), "stream-json".to_string(),
        ];

        if let Some(model) = config.get_str("model") {
            args.extend(["--model".into(), model.into()]);
        }
        if let Some(sp) = config.get_str("system_prompt_append") {
            args.extend(["--append-system-prompt".into(), sp.into()]);
        }
        if config.get_str("permissions") == Some("skip") {
            args.push("--dangerously-skip-permissions".into());
        }
        if let Some(session) = ctx.get_session() {
            args.extend(["--fork-session".into(), "--resume".into(), session.into()]);
        }

        let command = config.get_str("command").unwrap_or("claude");
        let mut child = tokio::process::Command::new(command)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Envia prompt via stdin
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(prompt.as_bytes()).await?;
        drop(stdin);

        // Parse streaming JSON
        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        let mut response = String::new();
        let mut session_id = None;
        let mut stats = AgentStats::default();

        while let Some(line) = lines.next_line().await? {
            match serde_json::from_str::<ClaudeMessage>(&line) {
                Ok(ClaudeMessage::Result(msg)) => {
                    response = msg.result;
                    session_id = msg.session_id;
                    stats = msg.stats.into();
                }
                Ok(ClaudeMessage::Text(msg)) => {
                    // Exibe progresso no terminal
                    display::agent_progress(&msg.content);
                }
                Ok(ClaudeMessage::ToolUse(msg)) => {
                    display::tool_use(&msg.tool, &msg.input);
                }
                _ => {}
            }
        }

        child.wait().await?;

        Ok(StepOutput::Agent(AgentOutput { response, session_id, stats }))
    }
}
```

### 8.3 chat (Agentico leve — API direta)

```
Engine                              LLM API
  │                                    │
  ├── render template do prompt        │
  ├── HTTP POST /v1/messages ─────────►│ Anthropic / OpenAI
  │   { model, messages, temp }        │
  │                                    │
  │   ◄─── response ──────────────────┤
  │   { content, usage }               │
  │                                    │
  ├── ChatOutput { response,           │
  │     model, usage }                 │
  │                                    │
  └── context.store("plan", output)    │

Diferenca do agent:
  chat = LLM pensa e responde texto (sem acesso a arquivos)
  agent = Claude Code edita arquivos, roda comandos, usa tools
```

### 8.4 gate (Deterministico — controle de fluxo)

```yaml
- name: check_lint
  type: gate
  condition: "{{ steps.lint.exit_code == 0 }}"
  on_pass: break       # break | continue | skip_next
  on_fail: continue    # continue | fail | skip_next
  message: "Lint passed"
```

```
Engine
  │
  ├── render e avalia condition
  │   "{{ steps.lint.exit_code == 0 }}" → "0 == 0" → true
  │
  ├── if true (passed):
  │   match on_pass:
  │     break → return Err(ControlFlow::Break)
  │     continue → nada (proximo step)
  │     skip_next → pula proximo step
  │
  ├── if false (failed):
  │   match on_fail:
  │     continue → nada
  │     fail → return Err(ControlFlow::Fail)
  │     skip_next → pula proximo step
  │
  └── GateOutput { passed: true, message }
```

### 8.5 repeat (Loop estruturado)

```yaml
- name: lint_gate
  type: repeat
  scope: lint_fix       # referencia ao scope definido acima
  max_iterations: 3
  initial_value: null    # opcional: valor inicial do scope
```

```
Engine
  │
  ├── for i in 0..max_iterations:
  │   │
  │   ├── child_ctx = context.child(scope_value, i)
  │   ├── execute_scope("lint_fix", child_ctx)
  │   │   │
  │   │   ├── cmd(:lint)       → roda
  │   │   ├── gate(:check)     → avalia
  │   │   │   └─ se passed → Break! ──────────► sai do loop ✓
  │   │   ├── agent(:fix_lint) → corrige
  │   │   │
  │   │   └── scope_output = ultimo step ou `outputs:`
  │   │
  │   ├── scope_value = scope_output (para proxima iteracao)
  │   └── continue loop
  │
  ├── se max_iterations atingido sem break:
  │   log warning "Max iterations reached"
  │
  └── ScopeOutput { iterations, final_value }
```

### 8.6 map (Colecao serial/paralela)

```yaml
- name: review_files
  type: map
  scope: analyze_file
  items: "{{ steps.changed_files.lines }}"
  parallel: 4           # 0 = serial, N = N concurrent, null = unlimited
```

```
Engine                              Tokio Runtime
  │                                      │
  ├── items = ["a.ts", "b.ts", "c.ts"]  │
  │                                      │
  ├── if parallel > 1:                   │
  │   semaphore = Semaphore(4)           │
  │   for item in items:                 │
  │     semaphore.acquire() ────────────►│ tokio::spawn
  │     spawn(execute_scope(item)) ─────►│   scope com item
  │                                      │   retorna output
  │   join_all() ◄──────────────────────┤
  │                                      │
  ├── if parallel == 0 (serial):         │
  │   for item in items:                 │
  │     execute_scope(item)              │
  │                                      │
  ├── resultados sempre na ordem original│
  │                                      │
  └── ScopeOutput { iterations }         │
```

### 8.7 parallel (Steps independentes)

```yaml
- name: parallel_analysis
  type: parallel
  steps:
    - name: security
      type: agent
      prompt: "Analyze for security issues..."
    - name: performance
      type: agent
      prompt: "Analyze for performance issues..."
    - name: style
      type: chat
      prompt: "Review code style..."
```

```
Engine                              Tokio Runtime
  │                                      │
  ├── for step in parallel.steps:        │
  │   tokio::spawn(execute(step)) ──────►│ step 1 (security)
  │   tokio::spawn(execute(step)) ──────►│ step 2 (performance)
  │   tokio::spawn(execute(step)) ──────►│ step 3 (style)
  │                                      │
  │   join_all() ◄──────────────────────┤
  │                                      │
  ├── store cada resultado:              │
  │   context["security"] = output1      │
  │   context["performance"] = output2   │
  │   context["style"] = output3         │
  │                                      │
  └── proximo step pode usar todos       │
```

---

## 9. Sistema de Config (4 Camadas)

```
                     Prioridade (mais especifico vence)
                     ─────────────────────────────────►

  ┌──────────┐   ┌──────────┐   ┌──────────┐   ┌──────────┐
  │ GLOBAL   │ + │  TIPO    │ + │ PATTERN  │ + │  STEP    │ = Config Final
  │          │   │          │   │          │   │ (inline) │
  │ timeout: │   │ agent:   │   │ "lint*": │   │ config:  │
  │   300s   │   │  model:  │   │  timeout:│   │  timeout:│
  │          │   │  sonnet  │   │  30s     │   │  10s     │
  └──────────┘   └──────────┘   └──────────┘   └──────────┘
       │              │              │              │
       └──────────────┴──────────────┴──────────────┘
                              │
                         timeout: 10s
                         model: sonnet
                    (step inline tem prioridade maxima)
```

---

## 10. Integracao com Docker Sandbox

### Modo 1: Workflow inteiro no sandbox

```bash
# O engine roda FORA, mas executa tudo DENTRO do sandbox
minion execute fix-issue.yaml -- 247 --sandbox

# Internamente:
# 1. docker sandbox create --name minion-247
# 2. docker sandbox cp workflow.yaml minion-247:/workspace/
# 3. docker sandbox exec minion-247 minion execute workflow.yaml -- 247
# 4. docker sandbox cp minion-247:/workspace/output .
# 5. docker sandbox rm minion-247
```

### Modo 2: So agent steps no sandbox

```yaml
config:
  agent:
    sandbox: true    # ← so steps agent rodam no sandbox
    # Engine roda local, cmd roda local, agent roda no sandbox
```

```
Engine (local)
  │
  ├── cmd(:fetch) → roda local
  ├── chat(:plan) → API call local
  ├── agent(:implement) → docker sandbox run claude -p "..."
  │                        └── sandbox isolado
  ├── cmd(:lint) → roda local (nos arquivos que o agent editou)
  └── cmd(:pr) → roda local
```

### Modo 3: Docker Sandbox como devbox (estilo Stripe)

```yaml
config:
  global:
    sandbox:
      enabled: true
      image: "minion-devbox:latest"    # imagem com deps pre-instaladas
      workspace: "/workspace"
      network:
        allow: ["github.com", "registry.npmjs.org"]
        deny: ["*"]                     # bloqueia todo o resto
      resources:
        cpus: 4
        memory: 8G
```

---

## 11. CLI Completa

```
minion — AI Workflow Engine

USAGE:
    minion <COMMAND>

COMMANDS:
    execute     Run a workflow
    validate    Validate a workflow YAML without running
    list        List available workflows
    init        Create a new workflow from template
    inspect     Show resolved config for a workflow
    version     Show version

EXECUTE:
    minion execute <workflow.yaml> [-- <target> [args...]]

    OPTIONS:
        --sandbox          Run in Docker Sandbox
        --dry-run          Show steps without executing
        --verbose          Show all step outputs
        --quiet            Only show errors
        --json             Output results as JSON
        --resume <step>    Resume from a specific step
        --timeout <secs>   Override global timeout
        --var KEY=VALUE    Set workflow variable

EXAMPLES:
    minion execute fix-issue.yaml -- 247
    minion execute code-review.yaml -- main..feature-branch
    minion execute security-audit.yaml -- src/ --verbose
    minion execute weekly-report.yaml --var team=backend
    minion execute deploy-check.yaml --sandbox --json
```

---

## 12. Dependencias Rust (Cargo.toml)

```toml
[package]
name = "minion-engine"
version = "0.1.0"
edition = "2021"

[dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }

# CLI
clap = { version = "4", features = ["derive"] }

# YAML parsing
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"

# Template engine
tera = "1"

# HTTP client (para chat step — LLM APIs)
reqwest = { version = "0.12", features = ["json", "stream"] }

# Error handling
anyhow = "1"
thiserror = "2"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Terminal display
colored = "2"
indicatif = "0.17"    # progress bars/spinners

# Utilities
regex = "1"
chrono = "0.4"
dirs = "5"            # home directory
```

**Zero dependencias de AI/LLM** — o engine e um task runner puro.
Claude Code e chamado via processo. LLM APIs via HTTP.

---

## 13. Workflows de Exemplo

### fix-issue.yaml (estilo Stripe Minions)
→ Ja detalhado na secao 4

### code-review.yaml
```yaml
name: code-review
steps:
  - name: diff
    type: cmd
    run: "git diff {{ target }} --stat && git diff {{ target }}"
  - name: review
    type: agent
    prompt: "Review this diff for bugs, security issues, and style:\n{{ steps.diff.stdout }}"
  - name: summary
    type: chat
    prompt: "Create a concise code review summary:\n{{ steps.review.response }}"
```

### security-audit.yaml
```yaml
name: security-audit
scopes:
  audit_file:
    steps:
      - name: content
        type: cmd
        run: "cat {{ scope.value }}"
      - name: review
        type: agent
        prompt: "Audit for OWASP Top 10:\n{{ steps.content.stdout }}"
    outputs: "{{ steps.review.response }}"

steps:
  - name: files
    type: cmd
    run: "find {{ target }} -name '*.ts' -o -name '*.js' | head -50"
  - name: audit
    type: map
    scope: audit_file
    items: "{{ steps.files.lines }}"
    parallel: 4
  - name: report
    type: chat
    prompt: "Create executive security report from:\n{{ steps.audit.results }}"
  - name: save
    type: cmd
    run: "echo '{{ steps.report.response }}' > security-report.md"
```

### incident-scan.yaml
```yaml
name: incident-scan
steps:
  - name: messages
    type: cmd
    run: "slack-export --channel {{ target }} --since yesterday"
  - name: analyze
    type: chat
    prompt: "Identify potential incidents:\n{{ steps.messages.stdout }}"
  - name: alert
    type: gate
    condition: "{{ 'HIGH' in steps.analyze.response }}"
    on_pass: continue
    on_fail: skip_next
  - name: notify
    type: cmd
    run: "curl -X POST {{ args.webhook }} -d '{{ steps.analyze.response }}'"
```

---

## 14. Estimativa de Tamanho

```
Modulo                          Linhas estimadas
──────────────────────────────────────────────
CLI (clap + commands)                    ~120
Workflow parser (YAML → structs)         ~200
Workflow validator                        ~150
Engine (dispatch loop)                   ~250
Context store                            ~150
Template engine (Tera wrapper)           ~100
Step: cmd                                ~100
Step: agent                              ~200
Step: chat                               ~150
Step: gate                                ~80
Step: repeat                             ~120
Step: map                                ~180
Step: parallel                           ~100
Step: call                                ~80
Claude integration (stream parse)        ~250
Config manager (4-layer merge)           ~180
Control flow                              ~50
Error types                               ~60
Display (terminal output)                ~150
──────────────────────────────────────────────
TOTAL                                  ~2,670 linhas

~2,700 linhas de Rust para um engine completo.
Roast tem ~5,000+ linhas de Ruby para fazer o mesmo.
```

---

## 15. Roadmap de Implementacao

```
Fase 1: MVP (1 semana)
  ✓ CLI basica (execute, validate)
  ✓ YAML parser
  ✓ Steps: cmd, agent, gate
  ✓ Context store + templates
  ✓ Repeat (loop basico)
  → Ja consegue rodar fix-issue.yaml

Fase 2: Completo (2 semanas)
  ✓ Steps: chat, map, parallel, call
  ✓ Config 4 camadas
  ✓ Claude stream JSON parsing
  ✓ Session management
  ✓ Display bonito (spinners, cores)
  → Engine completo, equivalente ao Roast

Fase 3: Polish (1 semana)
  ✓ Docker Sandbox integration
  ✓ --dry-run, --resume, --json
  ✓ Workflow init templates
  ✓ Testes de integracao
  ✓ Documentacao
  → Pronto para uso real

Fase 4: Distribuicao
  ✓ cargo install minion-engine
  ✓ Binarios pre-compilados (GitHub Releases)
  ✓ Homebrew formula
  ✓ Colecao de workflows prontos
```
