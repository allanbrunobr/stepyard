# Minion Engine — Autoresearch

Pesquisa autônoma para otimizar o Minion Engine.
Baseado no padrão autoresearch de Andrej Karpathy.

## Setup

Para iniciar uma nova sessão de experimentos:

1. **Combine um tag de run**: proponha um tag baseado na data (ex: `mar14`). A branch `autoresearch/<tag>` não pode existir.
2. **Crie a branch**: `git checkout -b autoresearch/<tag>` a partir da main.
3. **Leia os arquivos do projeto** para entender o código:
   - `README.md` — visão geral do projeto
   - `Cargo.toml` — dependências e configuração
   - `src/` — código-fonte Rust (11K LOC)
   - `tests/integration.rs` — testes de integração (17 testes)
   - `tests/prompt_resolver.rs` — testes do prompt resolver (11 testes)
   - `workflows/` — workflows YAML de exemplo (11 workflows)
   - `docs/` — documentação adicional
4. **Verifique que o evaluate roda**: `chmod +x evaluate.sh && ./evaluate.sh 2>/dev/null | grep "^score:"`
5. **Inicialize results.tsv**: crie com apenas o header row.
6. **Confirme e comece**.

## O que é o evaluate.sh

O `evaluate.sh` é o juiz fixo. Ele mede 6 dimensões:

| Dimensão | Peso | O que mede |
|---|---|---|
| **Compilação** | 10/100 | Compila sem erros? |
| **Testes** | 30/100 | Quantos testes passam? (cargo test) |
| **Warnings** | 15/100 | Warnings do compilador (menos = melhor) |
| **Clippy** | 15/100 | Lint warnings do clippy (menos = melhor) |
| **Binary size** | 10/100 | Tamanho do binário release (menor = melhor) |
| **Workflows** | 20/100 | Todos os YAML são válidos? (minion validate) |

Score = pontos obtidos / pontos possíveis (0.0 a 1.0).

Se não compila, score = 0.0 imediatamente.

## Experimentação

Cada experimento roda `./evaluate.sh`. O resultado leva ~45-60 segundos (compilação + testes).

**O que você PODE modificar:**

```
src/                             — TODO o código Rust
├── engine/mod.rs                ← core engine (1856 LOC) — execução de steps
├── engine/template.rs           ← template rendering (Tera)
├── engine/context.rs            ← execution context
├── engine/state.rs              ← state management
├── steps/                       ← executores de cada tipo de step
│   ├── cmd.rs                   ← shell commands
│   ├── chat.rs                  ← API calls diretas
│   ├── agent.rs                 ← Claude Code CLI
│   ├── gate.rs                  ← conditional flow
│   ├── repeat.rs                ← retry loops
│   ├── map.rs                   ← iteration
│   ├── parallel.rs              ← concurrent execution
│   └── call.rs                  ← scope invocation
├── workflow/                    ← YAML parsing e validação
│   ├── parser.rs                ← YAML → Workflow struct
│   └── validator.rs             ← validação de workflows
├── sandbox/                     ← Docker sandbox
├── config/                      ← 4-layer config resolution
├── prompts/                     ← stack detection + prompt registry
├── plugins/                     ← dynamic plugin loading
├── cli/                         ← CLI commands
├── error.rs                     ← error types
├── control_flow.rs              ← flow control
└── lib.rs                       ← library root

workflows/                       ← workflow YAML files
├── code-review.yaml
├── fix-issue.yaml
├── fix-test.yaml
├── security-audit.yaml
└── ... (11 workflows)

tests/                           ← testes
├── integration.rs               ← testes de integração
├── prompt_resolver.rs           ← testes do prompt resolver
└── fixtures/                    ← fixtures YAML e prompts
```

**O que você NÃO PODE modificar:**

```
evaluate.sh                      — avaliação fixa (o juiz)
program.md                       — este arquivo
Cargo.lock                       — gerado automaticamente
```

**O objetivo: obter o maior score possível.**

O score sobe quando você:
- Corrige warnings do compilador
- Corrige clippy warnings
- Adiciona testes que passam
- Reduz tamanho do binário
- Garante que todos os workflows validam
- Refatora código para eliminar dead code

O score desce quando você:
- Introduz erros de compilação (score = 0)
- Quebra testes existentes
- Adiciona warnings
- Aumenta o binário desnecessariamente

## Output format

```
---
score:              0.850000
compilation:        OK
tests_passed:       28
tests_total:        28
tests_failed:       0
warnings:           5
clippy_warnings:    3
binary_size_mb:     12.5
compile_time_ms:    41000
workflows_valid:    11
workflows_total:    11
total_time_s:       41.0
```

Extraia a métrica principal:
```bash
grep "^score:" run.log
```

## Logging de resultados

Logue em `results.tsv` (tab-separated):

```
commit	score	status	description
```

Exemplo:
```
commit	score	status	description
a1b2c3d	0.750000	keep	baseline
b2c3d4e	0.800000	keep	fix 5 clippy warnings in engine/mod.rs
c3d4e5f	0.650000	discard	refactor parser (broke 3 tests)
d4e5f6g	0.000000	crash	syntax error in steps/chat.rs
```

## O loop de experimentos

LOOP FOREVER:

1. Olhe o estado do git
2. Decida o que testar. Áreas para explorar:
   - **Warnings**: resolva compiler warnings (dead code, unused imports, unused variables)
   - **Clippy**: resolva clippy lints (unnecessary clones, manual implementations, etc.)
   - **Testes**: adicione testes para código não coberto (error paths, edge cases)
   - **Refactoring**: simplifique código mantendo funcionalidade (menos LOC = menos warnings)
   - **Dead code**: remova funções/structs não usadas
   - **Error handling**: melhore tratamento de erros (menos unwrap, mais Result)
   - **Performance**: otimize hot paths (template rendering, YAML parsing)
   - **Binary size**: remova dependências não usadas, use features flags
   - **Workflows**: corrija workflows inválidos ou adicione novos
   - **Tipo safety**: fortaleça tipos (menos String, mais enums/newtypes)
3. Edite os arquivos
4. `git add -A && git commit -m "descrição curta"`
5. `./evaluate.sh > run.log 2>&1`
6. `grep "^score:" run.log`
7. Se vazio → crashou. `tail -n 50 run.log` para ver o erro.
8. Registre em results.tsv (NÃO commite results.tsv)
9. Score subiu → keep (novo baseline). Score igual ou pior → `git reset --hard HEAD~1`

**Timeout**: Cada evaluate.sh deve rodar em <120 segundos. Se exceder 3 minutos, mate e trate como crash.

**NUNCA PARE**: O humano pode estar dormindo. Rode indefinidamente até ser interrompido.

## Estratégias sugeridas (em ordem)

1. **Baseline**: rode evaluate.sh sem mudar nada
2. **Quick wins**: resolva compiler warnings óbvios (unused imports, dead code)
3. **Clippy**: rode `cargo clippy --all-targets 2>&1` e resolva os lints
4. **Testes**: adicione testes para edge cases (empty workflow, invalid YAML, etc.)
5. **Refactoring**: simplifique funções longas no engine/mod.rs (1856 LOC)
6. **Dead code**: remova funções marcadas com `#[allow(dead_code)]`
7. **Error handling**: substitua `unwrap()` por `?` ou `.unwrap_or_default()`
8. **Dependências**: remova crates não usadas do Cargo.toml
9. **Binary size**: use `[profile.release] lto = true, strip = true`
10. **Novos testes**: teste cenários de erro (malformed YAML, missing steps, etc.)

## Dica: Rust é mais difícil

Diferente de Python, mudanças em Rust podem não compilar (borrow checker, lifetimes, types). Isso é normal. O padrão keep/discard lida com isso — crash = discard, seguir em frente.

Se uma mudança não compila:
1. Leia o erro do compilador
2. Tente corrigir (o rustc geralmente sugere o fix)
3. Se não conseguir em 2-3 tentativas, descarte e tente outra coisa
