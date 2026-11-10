# Progresso das correções P3 — análise de 09/11/2026

Referência: [`docs/analise-20261109.md`](analise-20261109.md), seção 3 (P3). Branch local `p3-correcoes`, criada a partir de `437dcf3` (`p2-correcoes`). Um commit local por item. **Sem push e sem PR.**

**Regras desta rodada:**
- não mexer em `.github/`, em credenciais nem em worktrees;
- não apagar nada fora do repositório;
- pular os itens que dependem de decisão do Allan (SEG-TS-05 e o subscriber do dashboard) e os majors do `vite`, do `vitest` e do `node-cron`.

**Verificação de cada item:**
- os portões do CI (fmt, clippy `-D warnings`, `audit-patterns.sh`, xtask G4, perfil SQLite) e o job TypeScript quando a API muda;
- a **suíte completa** com Postgres real (efêmero no M1) e o job SQLite, depois de cada item;
- a contagem de worktrees registradas pela suíte, que tem de ser +0;
- um mutante quando a correção tem um teste que pode distinguir.

## Itens

| # | Item | Achados | Status | Commit |
|---|---|---|---|---|
| 1 | O binário deixa de redeclarar a árvore de módulos da lib | ARQ-11, MANUT-10, TEST-05 | ✅ feito | `7b26a58` |
| 2 | Portões de auditoria que falhavam abertos (G4 do xtask; G1/G2 sem prefixo; autoteste dos portões) | BUG-CLI-15, TEST-08, TEST-06 (TEST-07/SEG-RUST-13 parcial) | ✅ feito | `eb95d08` |
| 3 | Saída de passo limitada no log de sessão (1 MiB por stream, configurável) | BUG-HARNESS-08 (BUG-HARNESS-03 pendente) | ✅ feito | `a913232` |
| 4 | `SessionId` no crate de contrato; o orquestrador deixa de depender do crate de sessão | ARQ-10 (ARQ-09 pendente) | ✅ feito | `78dc9e5` |
| 5 | Higiene: pool de banco morto da API; `allow(dead_code)` que não suprimiam nada | MANUT-16, MANUT-09 (parcial) | ✅ feito | `1e23944` |
| 6 | MSRV medida (1.88) e lint instável que não verificava nada | MANUT-08 (job de MSRV no CI pendente) | ✅ feito | `2f67af8` |
| 7 | Guard do diretório de build calibrado e documentado | MANUT-14 (parcial) | ✅ feito | `c731327` |
| 8 | Referências a invariantes inexistentes, comentário errado sobre `parallel`, `exclude` órfãos | MANUT-12 (parcial) | ✅ feito | `1b91937`, `8c1cb61` |
| 9 | Logs de diagnóstico com `session_id` e formato JSON opcional | MANUT-15 (parcial) | ✅ feito | `515eccc` |

---

## 1. Árvore de módulos declarada uma vez — ✅

**Defeito (ARQ-11, MANUT-10, TEST-05).** O `src/main.rs` redeclarava os 18 módulos (`mod cli; mod engine; …`) que o `src/lib.rs` já declara. Cada módulo compilava duas vezes, cada teste unitário rodava duas vezes (inflando a contagem da suíte), e cada tipo existia em duas cópias distintas, uma da lib e outra do binário. As árvores já tinham **divergido**: `signal` só existia no binário.

**Correção.** O `main.rs` virou um ponto de entrada fino sobre a lib (`use stepyard_engine::cli::Cli; use stepyard_engine::{display, signal};`), e o `lib.rs` passou a declarar `pub mod signal`.

**Medido:**
- Os três perfis compilam (padrão, `--features slack` e `--no-default-features --features sqlite`).
- A suíte completa foi de **1.199 para 835** testes, ou seja, −364. É exatamente o número de testes unitários da lib que rodavam uma segunda vez pelo binário.
- A lista de **nomes** dos testes da lib é idêntica antes e depois (364 = 364, `diff` vazio): nenhum teste distinto se perdeu. 0 falhas, 18 ignorados.
- Portões do CI: todos com status 0. Job SQLite: verde. Worktrees registradas pela suíte: +0.

Isto **não** tem mutante: é uma reorganização, e a prova é a igualdade das listas de testes junto com a suíte verde.

## 2. Portões de auditoria que falhavam abertos — ✅ (parcial)

**Defeitos.**
- **BUG-CLI-15 / TEST-08, portão G4 (xtask):**
  - o caminho do crate-alvo era relativo ao diretório atual, então rodado de um subdiretório o G4 "pulava o caminho ausente" e passava com 0 achados;
  - arquivo ilegível ou com erro de parse era pulado com saída 0;
  - a forma `self.lifecycle().<método>()`, usada no `scope.rs`, não era vista;
  - o xtask não tinha nenhum teste.
- **TEST-06, portões por regex:** G1 e G2 só pegavam `tracing::info!(…)` com o prefixo, e um `use tracing::info; info!(…)` passava. Nenhum portão tinha teste.

**Correção.**
- `xtask/src/main.rs`:
  - o caminho é resolvido pela raiz do workspace (`CARGO_MANIFEST_DIR`);
  - alvo ausente, arquivo ilegível ou erro de parse viram **erro com saída 2**, falhando fechado;
  - a forma `self.lifecycle()` é reconhecida;
  - 6 testes: violação detectada; `emit`/`session.append` antes da chamada aceitos; forma `self.lifecycle()` detectada; exceção auditada respeitada; arquivo que não faz parse é **contado como erro**; alvo encontrado de qualquer diretório.
- `scripts/audit-patterns.sh`:
  - os padrões viraram variáveis, e G1/G2 aceitam as macros com ou sem `tracing::`;
  - **autoteste**: antes da varredura, cada portão (G1, G2, G3, G5, Rule7a) tem de casar com um exemplo "ruim" conhecido em `scripts/audit-fixtures/*.rs.fixture`. A extensão os mantém fora das varreduras `--type rust`, e o G5 confirma isso com "0 matches" apesar de o fixture conter `SandboxError::Other`.

**Medido:**
- Com a forma nova reconhecida, o G4 continua com **0 achados** no repositório, inclusive rodando a partir de `crates/stepyard-core`: as chamadas por `self.lifecycle()` do `scope.rs` têm de fato `emit` antes.
- Os G1/G2 ampliados pegam **0** sites no código atual, então fecham o falso negativo sem mexer no código.
- **Mutantes:**
  - xtask sem reconhecer `self.lifecycle()` → reprova o teste da forma nova;
  - erro de parse ignorado → **o primeiro teste deixou esse mutante passar** (ele verificava o `check_source`, e o mutante muda o tratamento no `walk_file`). Extraí `audit_source_into` e testei o efeito real, a contagem do erro; agora reprova;
  - G1 de volta ao padrão antigo → o autoteste reprova em `g1_unqualified.rs.fixture`.
  - Tudo restaurado.
- Portões do CI: todos com status 0. Suíte completa: **841 passaram** (+6 do xtask), 0 falharam, 18 ignorados. Job SQLite: verde. Worktrees: +0.

**Não coberto (TEST-07 / SEG-RUST-13).** Ampliar o G3 para `.args(["-c", …])` e `/bin/sh` pegaria **4** sites que usam `sh -c` de propósito:
- `orchestrator/src/local.rs:144` executa o `run:` do workflow, que é a semântica do passo `cmd`;
- `src/sandbox/docker.rs:333/523`, as cópias do v1;
- `harness/src/process_group.rs:132`.

Fazer isso direito exige um mecanismo de exceção por linha, como o comentário `audit: … exempt - reason:` do G4, que o script de regex não tem. Excluir arquivos inteiros enfraqueceria o portão. Também não estendi a Rule7a para `std::thread::sleep`, porque os testes de polling limitado (por exemplo `cli_concurrent_runs`) usam esse padrão legitimamente.

## 3. Saída de passo sem limite no log de sessão — ✅ (parcial)

**Defeito (BUG-HARNESS-08).** O stdout/stderr **inteiro** de cada passo ia para uma linha JSONB do log de sessão. O log é relido por completo várias vezes por passo (`progress_from_log` e os `replay` do `scope.rs`), então o custo crescia a cada passo. Um log de build de centenas de MB acabava numa única linha.

**Correção** (`crates/stepyard-harness/src/engine.rs`): no `emit`, ponto único por onde todo evento passa antes do log, `cap_step_output` limita o `stdout` e o `stderr` do snapshot de `StepCompleted` a `STEPYARD_MAX_STEP_OUTPUT_BYTES` cada (padrão 1 MiB). O corte é feito numa fronteira de caractere e termina com o marcador `…[stepyard: output truncated, N more bytes …]`. Como os passos seguintes leem a saída do log, todos veem o mesmo valor limitado. `docs/CONFIG.md` documenta a variável.

**Testes.**
- Unitário: corte numa fronteira de caractere (limite de 1 byte cai dentro do `é` e recua), contagem de bytes descartados no marcador, stream abaixo do limite intacto, outros eventos inalterados.
- **Ponta a ponta pelo binário v2 com Postgres real** (`tests/cli_v2_output_cap.rs`): um passo que imprime 3.000.000 bytes, seguido de outro passo. A maior linha de `session_events` da sessão fica entre 1 MiB e 1,2 MB.

**Medido:**
- **Mutante** (sem o limite no `emit`): o teste reprova com a maior linha em **3.000.192 bytes**, a saída inteira. Com o limite, fica em torno de 1 MiB. Restaurado.
- Os dois testes caíram em erros **meus** antes de passar: comparar `Event` com `==` (o tipo não implementa `PartialEq`) e uma expectativa errada sobre um stream acima do limite. Ambos corrigidos no teste; a função não mudou.
- Portões do CI: todos com status 0. Suíte completa: **843 passaram**, 0 falharam, 18 ignorados. Job SQLite: verde. Worktrees: +0.

**Não coberto.**
- O log continua sendo relido por completo várias vezes por passo. Agora o custo é limitado por passo, mas segue linear no número de passos por releitura. Cache ou leitura incremental é uma mudança maior no harness.
- O `parallel` ainda só grava `StepStarted`/`StepCompleted` dos sub-passos no fim (BUG-HARNESS-03).

## 4. O orquestrador de sandbox deixa de depender do crate de sessão — ✅ (parcial)

**Defeito (ARQ-10).** O `stepyard-sandbox-orchestrator` dependia do `stepyard-session` inteiro, com sqlx, drivers, migrações e as features `postgres`/`sqlite`, só para usar o tipo `SessionId` num único arquivo (`workspace.rs`). A camada de sandbox carregava a camada de persistência.

**Correção.**
- `SessionId` (o newtype de `Uuid`, com `Display`, `FromStr` e as conversões) mudou para `stepyard-core` (`session_id.rs`), o crate de contrato sem IO. O `stepyard-session` o **reexporta** (`pub use stepyard_core::SessionId`), então nenhum usuário de `stepyard_session::SessionId` precisou mudar.
- O orquestrador depende de `stepyard-core` em vez de `stepyard-session`. As features `postgres`/`sqlite` dele, que só repassavam ao crate de sessão, saíram, junto com as duas referências no `Cargo.toml` da raiz.

**Medido:**
- `cargo tree -p stepyard-sandbox-orchestrator -e normal`: as dependências diretas do workspace se resumem a `stepyard-core`, e há **0** ocorrências de `sqlx` no grafo.
- Os três perfis compilam (portões, perfil SQLite incluso). Suíte completa: **843 passaram**, 0 falharam, 18 ignorados. Job SQLite: verde. Worktrees: +0.

Não há mutante: é uma mudança de estrutura. A prova é o grafo de dependências junto com a suíte verde.

**Não coberto: ARQ-09.** A reconciliação ainda usa SQL direto e `docker ps`, em vez dos traits `EventStore`/`SandboxLifecycle`. A correção da P1 (lease por *advisory lock* do PostgreSQL) depende de recursos específicos do Postgres que esses traits não modelam. Levar isso para os traits exige primeiro desenhar um "lease" abstrato no `EventStore`, com uma implementação para o SQLite, e fica para depois.

## 5. Higiene: código morto da API e `allow(dead_code)` inócuos — ✅ (parcial)

**MANUT-16.** `packages/api/src/db/index.ts` e `src/db/pool.ts` formavam um segundo pool de PostgreSQL que nenhuma rota usava, com um valor padrão embutido (`postgresql://postgres:postgres@localhost:5432/stepyard`). Os dois saíram. A única rota que importava dali (`routes/overview.ts`) passou a usar o módulo de banco real, `src/db.ts`.

**MANUT-09.** O código tinha 50 `#[allow(dead_code)]`/`#![allow(dead_code)]` em 24 arquivos (52 com `tests/support`, fora da varredura), muitos com comentários como "wired into adapt_step in PR 5c" que não valiam mais. Fiz uma varredura mecânica, com script:
1. remover todos os allows de linha inteira dos 24 arquivos;
2. rodar `cargo check` em três perfis (padrão `--workspace --all-targets`, `--features slack`, `--no-default-features --features sqlite`), com timeout em cada um;
3. restaurar o arquivo inteiro sempre que aparecia um aviso `never used/read/constructed`;
4. repetir até não haver aviso.

Convergiu em 2 rodadas: 7 arquivos voltaram ao original, porque os avisos eram reais (`harness/engine.rs`, `executor.rs`, `startup.rs`, `orchestrator/lib.rs`, `cli/remote.rs`, `engine/mod.rs`, `events/subscribers.rs`).

Ajustes feitos à mão depois da varredura:
- **voltaram** os dois allows de `process_group.rs` e `orchestrator/local.rs`. Eles existem para builds **não-Unix**, e a varredura só compila no macOS, então a falta de aviso aqui não prova nada sobre aqueles alvos;
- saíram três comentários de cabeçalho que só justificavam o `#![allow]` removido.

Resultado: **24 linhas removidas em 15 arquivos Rust**.

**O que o instrumento não distingue.** Num crate lib, um item `pub` nunca gera aviso de código morto. Logo, a ausência de aviso nesses itens só mostra que o allow não suprimia nada, e **não** que o código é usado. Os plugins (`src/plugins/*`, `validate_plugin_configs`), por exemplo, continuam sem uso real (ARQ-12, ARQ-07). Sobram 24 allows nos 7 arquivos restaurados, mais 2 nos alvos não-Unix e 2 em `tests/support`. Os dos 7 arquivos marcam código de fato morto e saem junto com a desativação do v1 ou com uma revisão item a item.

**Medido:**
- Portões do CI: todos com status 0. Clippy `--features slack -D warnings`: 0.
- Job TypeScript: build da API e do web, e testes da API, todos com status 0.
- Suíte completa: **843 passaram**, 0 falharam, 18 ignorados. Job SQLite: verde. Worktrees: +0.

- **Linux** (verificado depois, junto com o item 8): `cargo check --locked` da árvore de trabalho num contêiner `rust:1.88`, com `RUSTFLAGS=-Dwarnings`, aprova (status 0) os três perfis: padrão `--workspace --all-targets`, `--features slack` e `--no-default-features --features sqlite`. A varredura original só tinha compilado no macOS.

Não há mutante: a mudança não altera comportamento. A prova é a compilação sem aviso nos três perfis, em macOS e em Linux, junto com a suíte verde.

## 6. MSRV declarada e lint que não verificava nada — ✅ (parcial)

**Defeito (MANUT-08).**
- O `Cargo.toml` declarava `rust-version = "1.82"`, mas o grafo travado no `Cargo.lock` exige mais.
- O `deny` de `non_exhaustive_omitted_patterns` era um lint instável. No stable, o compilador o ignora com um aviso "unknown lint" repetido em cada crate e alvo. No nightly, ele também precisaria de `#![feature(non_exhaustive_omitted_patterns_lint)]` no crate, e nenhum crate declara isso. Então não verificava nada em toolchain nenhuma.

**Medido antes de mudar.**
- Varredura estática do `rust-version` de todo pacote do registry no lock: o maior é o do **`home` 0.5.12, que pede 1.88** e entra no grafo normal pelo `sqlx-postgres`. O relatório falava em 1.85; não tinha visto esse crate.
- A prova veio de compilação real em Linux, no M1, em contêineres descartáveis (`docker run --rm rust:<versão>`, com o fonte copiado para dentro), rodando `cargo check --locked --workspace --all-targets`:
  - **`rust:1.87`: recusado**, com `home@0.5.12 requires rustc 1.88` como único motivo;
  - **`rust:1.88`: compila**.
- Um `cargo check` completo local (rustc 1.98) imprimia **65 linhas de aviso**, todas desse lint e nenhuma de outra origem.

**Correção.**
- `rust-version = "1.88"`, com um comentário que diz de onde vem o número e como reverificar.
- O lint saiu da tabela `[workspace.lints.rust]`. **Não** usei `unknown_lints = "allow"`, que esconderia erros de digitação em nomes de lint.
- Três comentários em `src/events/subscribers.rs` diziam que o lint "dispararia" se um variante novo faltasse. Agora descrevem só o comportamento do braço.
- A CONTRIBUTING passou a citar a MSRV e o comando para reverificá-la depois de um `cargo update`.

**Medido depois:**
- `cargo check` completo local: **0** avisos.
- Árvore de trabalho no contêiner `rust:1.88`: compila, com 0 avisos. A primeira tentativa falhou por um artefato **meu**: o `tar` do macOS levou arquivos AppleDouble `._*.sql` e o `sqlx::migrate!` tentou lê-los. Com `COPYFILE_DISABLE=1`, passou.
- Portões do CI e clippy `--features slack`: todos com status 0.
- Suíte completa: **843 passaram**, 0 falharam, 18 ignorados. Job SQLite: verde. Worktrees: +0.

**Não coberto.** Não existe job de MSRV no CI, porque criá-lo exige mexer em `.github/`. A MSRV volta a ficar falsa no primeiro `cargo update` que puxar um crate mais novo. O passo do CI seria `dtolnay/rust-toolchain@1.88` mais `cargo check --locked --workspace --all-targets`. Também fica para depois a medição de cobertura com piso (TEST-15), pelo mesmo motivo.

## 7. Guard do diretório de build — ✅ (parcial)

**Defeito (MANUT-14).**
- `scripts/guard-cargo-target-size.sh` tinha limite padrão de 1 GiB. Uma única linha de base completa deixa 8,4 GiB no target, então o `scripts/cargo-guarded.sh` (que é `guard; cargo; guard`) falhava depois de **todo** build completo bem-sucedido.
- Nenhum documento citava os dois scripts nem o `target-dir` compartilhado em `/tmp/stepyard-target`.

**Correção.**
- O limite padrão do guard foi para 16 GiB, cerca de duas passadas completas. O comentário no script explica a conta, e `STEPYARD_CARGO_TARGET_MAX_MIB` continua sobrescrevendo.
- A CONTRIBUTING ganhou a seção "Build directory": por que o target fica fora do checkout, que ele é compartilhado entre checkouts e worktrees (disputa o mesmo lock do Cargo), como isolar uma worktree com `CARGO_TARGET_DIR`, e o que os dois scripts fazem.

**Medido:**
- Guard no diretório real, hoje com **42,94 GiB** (inflado pelas builds desta análise): reprova com status 1, como deve.
- Diretório vazio: aprova com status 0. Arquivo de 2 MiB: limite de 1 MiB reprova (status 1) e limite de 3 MiB aprova (status 0).
- O `CARGO_TARGET_DIR` de fato tem precedência sobre o `target-dir` da config: `cargo metadata` passa de `/tmp/stepyard-target` para o caminho da variável. Assim, a instrução da CONTRIBUTING funciona.
- Portões do CI: todos com status 0. Suíte completa: **843 passaram**, 0 falharam, 18 ignorados. Job SQLite: verde. Worktrees: +0.

**Não coberto (decisão do Allan).** O `target-dir = "/tmp/stepyard-target"` versionado continua.
- Um caminho por usuário não cabe na config, porque o Cargo não expande `~` nem variáveis em `target-dir`.
- Remover a linha devolveria o `target/` local de mais de 100 GiB para dentro de cada checkout, que é o problema que ela resolveu.
- Mudar o padrão afeta o fluxo de todas as worktrees, então a escolha fica para quem mantém o repositório.
- O diretório `/tmp/stepyard-target` está em 42,94 GiB e **não foi limpo**: fica fora do repositório.

## 8. Comentários que apontam para o que não existe — ✅ (parcial)

**Defeito (MANUT-12).** O código cita um planejamento privado que o leitor não consegue consultar. Corrigi a parte com dano concreto:
- **21 citações de invariantes numeradas** que a `ARCHITECTURE.md` não tinha: "Invariante 11" (16×), "Invariante 9" (3×) e "Invariante 2/3" (uma vez cada). A ARCHITECTURE lista só 6.
- O cabeçalho de `src/cli/harness_adapter.rs` dizia que `parallel` "is the only v1 kind still rejected outright", mas o `adapt` o converte num escopo sintetizado `__parallel_<índice>`.
- O `stepyard-session/src/lib.rs` mandava ler uma seção "Data Model" que não existe.
- O `Cargo.toml` tinha `exclude` para `PROMPT.md`, `EPICS-AND-STORIES.md` e `territory_map.json`, que não existem no repositório nem no histórico.

**Correção.**
- A `ARCHITECTURE.md` ganhou:
  - a **invariante 7**: sessões concorrentes não interferem umas nas outras. Ela tem evidência: `SandboxLifecycle` é `Send + Sync`, `tests/concurrent_sessions.rs` e `tests/cli_concurrent_runs.rs`, e o lease da P1 (`557b852`) faz a reconciliação só falhar sessões cujo dono sumiu;
  - uma subseção **"Design rule with known gaps"** com a **regra 8**: o motor v2 *deveria* não guardar estado entre passos. Ela **não** fica na lista "These hold today", porque hoje não vale. A subseção nomeia as exceções abertas: BUG-HARNESS-03 (`parallel` só grava os eventos dos sub-passos no fim), BUG-HARNESS-04 (turnos de chat dentro de escopo fora do histórico), BUG-HARNESS-07 (o replay de escopo só recupera `cmd`) e a CLI sem comando de retomada. O commit `1b91937` a listava, errado, como invariante válida (número 7), contradizendo esses achados e o `437dcf3`. A revisão apontou o erro, e o commit seguinte corrigiu e renumerou.
- As citações agora apontam para a ARCHITECTURE:
  - as 16 de "Invariante 11" e a "Invariante 2, 11" do crate de sessão → "ARCHITECTURE invariant 8" / "invariants 1 and 8";
  - as 3 de "Invariante 9" → "ARCHITECTURE invariant 7";
  - a "Invariante 3" → invariante 5 (contêineres descartáveis).
- O cabeçalho do adaptador descreve o que acontece com `parallel`: adaptado no nível superior e rejeitado dentro de outro escopo (`NestedScopesNotSupported`, porque é contêiner).
- Os três `exclude` órfãos saíram.

**Medido:**
- `rg 'Invariante [0-9]'` em .rs/.toml/.yml/.ts/.sh: de 21 para **0**.
- Portões do CI: todos com status 0. Suíte completa: **843 passaram**, 0 falharam, 18 ignorados. Job SQLite: verde. Worktrees: +0. A correção da numeração foi verificada outra vez do mesmo jeito, com o mesmo resultado.

Só comentários e documentação mudaram, então não há mutante.

**Não coberto.** Continuam as referências de **história** de planejamento. Contagem atual nos mesmos tipos de arquivo:

| Referência | Ocorrências | Arquivos |
|---|---|---|
| `Story N` | 164 | 65 |
| `PR N` | 191 | 26 |
| `Task #N` | 105 | 26 |
| `Round N` | 29 | 20 |
| `Epic N` | 15 | 10 |

Trocar cada uma por uma descrição de comportamento é trabalho manual: cerca de 500 comentários em 60+ arquivos, cada um exigindo ler o código ao redor. Não vale fazer isso antes da desativação do v1, que apaga boa parte deles. Também não pus um portão no `audit-patterns.sh` contra ocorrências novas. Com centenas de casos existentes, ele precisaria de uma linha de base, e esse mecanismo o script não tem.

## 9. Logs de diagnóstico sem horário e sem sessão — ✅ (parcial)

**Defeito (MANUT-15).**
- Os logs de `tracing` da CLI saíam sem horário e sem `session_id`, e não havia nenhum span no código. Uma linha de log não podia ser ligada ao log da sessão (`.stepyard/logs/<sessão>.jsonl`).
- O `RUST_LOG` não aparecia em nenhum documento.

**Correção.**
- `src/main.rs`: com `STEPYARD_LOG_FORMAT=json` (opt-in), cada linha vira um objeto JSON com `timestamp` e os campos dos spans ativos. O padrão segue o texto compacto sem horário, porque a CLI, o despacho remoto e os testes leem essa saída. Feature `json` do `tracing-subscriber`; o lock ganhou só o `tracing-serde` 0.2.0.
- `stepyard_harness::Engine::step` roda dentro de um span `session{session_id}`, o que vale também para quem usa a biblioteca. O span é de nível **ERROR**. Na primeira versão ele era INFO, e **o teste reprovou**: com `RUST_LOG=warn`, o `EnvFilter` descartava o span, e justamente os avisos perdiam o `session_id`.
- `docs/CONFIG.md` ganhou a seção "Diagnostic logs": a diferença entre o log de sessão e os logs de diagnóstico, mais `RUST_LOG` e `STEPYARD_LOG_FORMAT`.

**Teste** (`tests/cli_v2_log_json.rs`, binário v2 com Postgres real): um `repeat` sem gate de saída atinge `max_iterations`, e o harness emite um `warn` dentro do passo. Com `STEPYARD_LOG_FORMAT=json` e `RUST_LOG=warn`, esse aviso tem de sair como JSON com `timestamp` e com `span.session_id` igual ao id da sessão gravada no banco para aquele tenant.

**Medido:**
- Antes do teste, rodei o binário à mão num workflow `cmd` simples. Nenhum evento de log acontece dentro do passo, nem em `debug`. O span só aparece quando há eventos no caminho do passo: o `max_iterations` do `repeat`, os avisos de workspace e tar do orquestrador e a sincronização do contêiner.
- **Mutantes**, todos reprovando o teste:
  - span INFO: o aviso sai sem `span`;
  - sem `#[instrument]`: `left: None`, `right: Some(<uuid>)`;
  - sem o ramo `json`: "no JSON max_iterations warning".
  - Restaurados.
- Portões do CI e clippy `--features slack`: todos com status 0. Linux (`rust:1.88`, `-Dwarnings`): os três perfis com status 0.
- Suíte completa: **844 passaram** (+1), 0 falharam, 18 ignorados. Job SQLite: verde. Worktrees: +0.

**Não coberto.**
- A reconciliação de inicialização roda antes de existir sessão.
- As tarefas criadas com `spawn` dentro do `parallel` não herdam o span.
- Os cerca de 270 `println!`/`eprintln!` são a interface da CLI, não logs.
- Não há métricas.

## Fora do escopo dos itens: limpeza de worktrees (pedida à parte)

Das 235 worktrees registradas, **230** eram `stepyard-session-*` criadas por suítes de teste:
- 226 em `.stepyard/workspaces` deste checkout;
- 4 em `crates/stepyard-harness/.stepyard/workspaces`.

Antes de remover, conferi que nenhuma estava travada (`locked`), nenhuma tinha processo aberto e nenhuma tinha alteração não commitada. Todas saíram com `git worktree remove --force`, sem nenhuma falha, e depois rodei `git worktree prune`. O checkout principal, as worktrees das sessões Orca e as 2 `stepyard-session-*` que ficam dentro do checkout principal **não** foram tocadas.

## O que ficou de fora da P3

### Pulado por ordem (decisão do Allan ou major de dependência)
- **SEG-TS-05** e o **segredo do subscriber do dashboard**: dependem de decisão do Allan.
- **Majors do `vite`, do `vitest` e do `node-cron`.**

### Exige `.github/`, que não posso tocar
- **Job de MSRV** (MANUT-08): `dtolnay/rust-toolchain@1.88` mais `cargo check --locked --workspace --all-targets`. Sem ele, a MSRV volta a ficar falsa no primeiro `cargo update` que puxar um crate mais novo.
- **Cobertura com piso** (TEST-15, `cargo llvm-cov`).
- **SEG-TS-10/11, TEST-02/03/13, MANUT-05**: os achados de CI e workflows do relatório.

### Grandes demais para uma rodada de commits locais (G no relatório)
- **Desativação do v1** (ARQ-07, MANUT-13, SEG-RUST-10, ARQ-12, TEST-14), estimada em 2–4 semanas. Antes de apagar uma linha, é preciso portar para o v2 o `--repo`, um subscriber de eventos que leia o log de sessão, `stack`/`prompts` e o `--dry-run`. Só depois saem `src/engine`, `src/steps`, `src/sandbox/docker.rs`, `src/events` e `src/plugins` (cerca de 8,9 mil linhas). Não comecei uma remoção parcial: com dois motores convivendo, apagar metade de um deixaria o padrão (v2) e o v1 divergindo ainda mais. Enquanto isso, os plugins seguem com `dlopen` no construtor do v1, nunca despachados e com `unsafe` sem teste.
- **Quebra das funções gigantes** (ARQ-08, MANUT-11): `run_parallel` (~600 linhas), `run_chat_step`, `Engine::step`, `execute_v2`, e a unificação dos executores de nível superior e de escopo (51 montagens manuais de `StepFailed`). É refatoração sem mudança de comportamento, mas no caminho quente do harness. Pede uma rodada própria, com caracterização dos eventos emitidos antes e depois.

### Ficou parcial, com o motivo registrado no item
- **ARQ-09** (item 4): a reconciliação continua com SQL direto e `docker ps`. Passá-la para os traits exige primeiro um "lease" abstrato no `EventStore`, com implementação para o SQLite.
- **BUG-HARNESS-03** (item 3): o `parallel` só grava `StepStarted`/`StepCompleted` dos sub-passos no fim. A correção muda a ordem dos eventos no log. Precisa ser conferida contra a invariante 2 (pareamento por passo de nível superior), a contagem de progresso e retomada, o contrato do dashboard (`tests/fixtures/dashboard-event.json`) e o limite de saída do item 3. Não a comecei para não deixá-la pela metade.
- **Releitura completa do log** várias vezes por passo (item 3): agora limitada por passo, mas ainda linear no número de passos.
- **G3 e Rule7a mais amplos** (item 2): exigem um mecanismo de exceção por linha no `audit-patterns.sh`.
- **MANUT-09** (item 5): restam 24 `allow(dead_code)` em 7 arquivos sobre código de fato morto, mais 2 para alvos não-Unix e 2 em `tests/support`.
- **MANUT-12** (item 8): continuam cerca de 500 referências de história de planejamento (`Story N`, `PR N`, `Task #N`, `Round N`, `Epic N`). Melhor tratá-las depois da desativação do v1.
- **MANUT-14** (item 7): o `target-dir` versionado em `/tmp` continua. Mudar o padrão é decisão do Allan.
- **MANUT-15** (item 9): o span cobre `Engine::step`. Ficam fora a reconciliação de inicialização (antes de existir sessão), as tarefas criadas com `spawn` dentro do `parallel` e os ~270 `println!`/`eprintln!` da interface.
- **A regra 8 da ARCHITECTURE** (sem estado entre passos) não vale hoje. As exceções nomeadas lá são BUG-HARNESS-03, -04, -07 e a falta de retomada na CLI.

### Ainda abertos da P2 (fora do escopo desta rodada)
- **BUG-CLI-08**: evento `WorkflowFailed` no core.
- **SEG-TS-04**: compose de produção.
- **BUG-MOTOR-02/15**: proxy no Linux.
- **SEG-TS-08**: cookie.
- **TEST-10/11**.

### Efeitos colaterais fora do repositório (nada apagado)
- `/tmp/stepyard-target` está com **42,94 GiB**, em boa parte das builds desta análise. Não foi limpo.
- No daemon Docker do M1 ficaram as imagens `rust:1.87` e `rust:1.88`, baixadas para medir a MSRV. Os contêineres foram todos `--rm`. Antes, na P2, eu já tinha reconstruído a imagem `stepyard-sandbox:latest` lá.
