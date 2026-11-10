# Progresso das correções P2 — análise de 09/11/2026

Referência: [`docs/analise-20261109.md`](analise-20261109.md), seção 3 (P2). Branch local `p2-correcoes`, criada a partir de `9350b4b` (a ponta da `analise-stepyard`, PR #1). Um commit local por item. **Sem push.**

A verificação de cada item segue o padrão da P1 (ver `docs/p1-progresso.md`):
- testes com Postgres real (efêmero no M1, via Tailscale);
- os portões do CI (fmt, clippy, audit-patterns, xtask G4, perfil SQLite) e o job TypeScript quando a API muda;
- a suíte completa;
- um mutante por correção, para provar que o teste novo distingue.

**Fora do escopo desta branch:** arquivos em `.github/`. Nenhuma ordem desta sessão liberou mexer neles. Os itens P2 que dependem do CI (fixar actions por SHA, `permissions:`, `cargo audit`/`npm audit` no CI, job com Docker, compilar `--features slack` no CI) ficam **registrados como pendentes**, com a mudança proposta, sem alteração.

## Itens

| # | Item | Achados | Status | Commit |
|---|---|---|---|---|
| 0 | Contradições do pré-PR do #1 e o dry-run do v1 | checagem pré-PR; regressão do P1-10 | ✅ feito | (este) |

---

## 0. Contradições encontradas pela checagem pré-PR do #1 — ✅

A checagem de contradições rodou antes do PR #1 e deixou três defeitos reais, todos introduzidos pela P1. Eles foram registrados no corpo do PR. Durante a correção apareceu um quarto, que a checagem tinha refutado por engano.

1. **`{{ vars.BRANCH }}` não existia no v1.** A `docs/REMOTE.md` mandava ler `--branch` como `{{ vars.BRANCH }}`, mas o despacho roda `--engine v1`, que só expunha `args`. **Medido** com o argv exato do despacho: falhava com ``Variable `vars.BRANCH` not found``, e `{{ args.BRANCH }}` funcionava.
   - **Correção:** o v1 expõe o mesmo objeto como `vars` e como `args`, como o v2 já fazia. A documentação passa a usar `{{ args.BRANCH }}`, que vale nos dois motores, e deixa claro que não há checkout de branch. A ajuda de `stepyard remote exec --branch` não promete mais checkout.
2. **`dispatch_failed` não era estado terminal do stream de logs.** Para um run nesse estado, `GET /workflows/:id/logs/stream` nunca enviava `done`, e `stepyard remote logs` ficava pendurado.
   - **Correção:** o status entrou em `TERMINAL_STATUSES`.
3. **A documentação mostrava comandos que o v2 passou a recusar** (`docs/DOCKER-SANDBOX.md`, `docs/EXAMPLES.md`).
   - **Correção:** os exemplos agora trazem `--engine v1` e, quando há sandbox, `--repo` ou `--no-sandbox`.
4. **O `--dry-run` do v1 estava bloqueado pelo P1-10.** O `--dry-run` só imprime o plano e só existe no v1. O bloqueio do sandbox v1 o recusava, e sem `--no-sandbox` não restava dry-run nenhum. A checagem pré-PR tinha "refutado" esse achado; **medi**, e a recusa acontecia.
   - **Correção:** as duas políticas do item 10 não se aplicam a `--dry-run`, que não cria contêiner.

**Testes:**
- `tests/cli_v1_vars.rs`: binário v1 com Postgres real. `{{ vars.BRANCH }}` e `{{ args.BRANCH }}` imprimem `feat/x`.
- `packages/api/src/routes/stream.test.ts`: o stream real, com o banco simulado devolvendo um run `dispatch_failed`, termina com `event: done`.
- `tests/cli_sandbox_policy.rs`: `--engine v1 --dry-run` com sandbox não recebe mais a recusa.

**Medido:**
- **Mutantes:** cada correção desfeita reprova o seu teste. No caso do stream, o teste ficou pendurado até o timeout de 8 s, que é o próprio defeito. Tudo restaurado.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.184 passaram, 0 falharam, 16 ignorados**. Job SQLite: verde. Job TypeScript: verde (**22 testes**).
