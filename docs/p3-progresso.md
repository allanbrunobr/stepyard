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
| 1 | O binário deixa de redeclarar a árvore de módulos da lib | ARQ-11, MANUT-10, TEST-05 | ✅ feito | (este) |

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
