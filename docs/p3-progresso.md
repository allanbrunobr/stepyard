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
| 2 | Portões de auditoria que falhavam abertos (G4 do xtask; G1/G2 sem prefixo; autoteste dos portões) | BUG-CLI-15, TEST-08, TEST-06 (TEST-07/SEG-RUST-13 parcial) | ✅ feito | (este) |

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
