# Progresso das correções P1 — análise de 09/11/2026

Referência: [`docs/analise-20261109.md`](analise-20261109.md), seção 3 (P1). Um commit local por item, na ordem da tabela P1 (gravidade). Nada foi empurrado.

## Como cada item é verificado

- **Testes com Postgres real.** Os testes de banco rodam contra um Postgres 16 efêmero no daemon Docker remoto (`m1-docker`), alcançado pelo IP Tailscale do M1. O contêiner tem `--rm`, uma senha descartável e é removido ao fim de cada execução. Isso corrige a seção 1 do relatório, que dava esses testes como "não medidos": a porta não responde em `127.0.0.1`, mas responde pelo IP Tailscale.
- **Linha de base com banco, antes de qualquer correção** (commit `ea26129`): `cargo test --workspace --no-fail-fast` → **1.125 passaram, 0 falharam, 13 ignorados** (52 binários). Os 94 testes dependentes de banco passam contra Postgres real.
- **Portões por item, iguais aos do CI:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `scripts/audit-patterns.sh`, `cargo xtask audit-emit-before-io`, perfil SQLite (`cargo check` do engine e `clippy` do session). Por fim, a suíte inteira com banco.
- **O teste novo precisa distinguir.** Para cada correção, injeto o defeito antigo (mutante) e confirmo que o teste novo reprova na asserção certa. Só então restauro o código.

## Itens

| # | Item P1 | Achados | Status | Commit |
|---|---|---|---|---|
| 1 | Reconciliação de partida só para sessões órfãs | ARQ-01, BUG-CLI-01, BUG-HARNESS-01 | ✅ feito | `529b532` |
| 2 | Slack `target` + filtro `shell_quote` nos workflows | SEG-RUST-02, SEG-RUST-07 | ✅ feito | `fb6c43f` |
| 3 | Tera sem `get_env` | SEG-RUST-04 | ✅ feito | (este) |
| 4 | API: `dispatch_log_path` e segredos no log SSH | SEG-TS-01, SEG-TS-02 | pendente | |
| 5 | API resiliente a falhas do Postgres | BUG-CLI-05, SEG-TS-03 | pendente | |
| 6 | Contrato de despacho dashboard/remote | ARQ-02, BUG-CLI-03/04/07, MANUT-04/07, SEG-TS-09 | pendente | |
| 7 | Adaptador v1→v2: `timeout` e `fail_on_error` | BUG-CLI-02, MANUT-01/02, ARQ-06 | pendente | |
| 8 | v1 `parallel` honra `fail_on_error` resolvido | BUG-MOTOR-08 | pendente | |
| 9 | Workspace v2: timeout e caminhos excluídos | BUG-HARNESS-02, SEG-RUST-08 | pendente | |
| 10 | Sandbox: volumes do YAML e bloqueio do v1 `--sandbox` | SEG-RUST-03, SEG-RUST-01, BUG-MOTOR-04, ARQ-04, SEG-RUST-11 | pendente | |

## Pendências que são do Allan (exigem conta, login, gasto ou segredo)

- **Rotacionar `GH_TOKEN`, `ANTHROPIC_API_KEY` e a senha do banco** em qualquer ambiente onde a stack compose rodou com despacho por SSH. Os valores estão em claro nos logs do contêiner da API (SEG-TS-02). A correção de código do item 4 impede novos vazamentos, mas não apaga os que já aconteceram.

---

## 1. Reconciliação de partida só para sessões órfãs — ✅

**Defeito.** Cada `stepyard execute` (v2 + Postgres, o padrão) marcava como `failed` **toda** sessão `running` do banco e fazia `docker rm -f` no contêiner dela (`crates/stepyard-harness/src/startup.rs:88-101`, chamada incondicional em `src/cli/commands.rs:466-478`). Um segundo processo contra o mesmo banco destruía o trabalho do primeiro.

**Correção.**
- `SessionLease` (`crates/stepyard-harness/src/startup.rs`): o dono da sessão segura um *advisory lock* de sessão do PostgreSQL (`pg_try_advisory_lock`, chave = 8 primeiros bytes do UUID) numa conexão dedicada, desanexada do pool. O PostgreSQL libera o lock quando a conexão fecha, inclusive se o processo morrer, então "consegui o lock" é prova direta de que o dono se foi.
- `execute_v2` (`src/cli/commands.rs`) toma o lease logo depois de criar a sessão e o mantém até retornar.
- A reconciliação só considera sessões com `started_at` mais antigo que `LEASE_GRACE` (60 s), o que cobre a janela entre o INSERT e o lease. Para cada uma, tenta o lock: se outro processo o segura, a sessão é poupada (`ReconcileReport::sessions_spared`) e, por continuar `running`, o contêiner dela também. Uma colisão de chave só pode poupar uma sessão morta; nunca afeta uma viva.
- Não há migração de esquema. Sessões criadas por binários antigos (sem lease) continuam sendo tratadas como órfãs depois da carência, como antes.

**Testes** (`crates/stepyard-harness/tests/startup_reconcile_liveness.rs`, novo, e `startup.rs` unitário):
- `reconcile_spares_a_session_whose_owner_holds_the_lease_and_fails_it_once_released`: sessão com lease fica `running`, sem `crash_recovery`; um segundo lease é recusado; depois do `release`, a reconciliação a marca `failed` com exatamente um `crash_recovery`.
- `reconcile_spares_a_fresh_session_inside_the_grace_window`.
- `dropping_the_lease_releases_it_so_a_crashed_owner_is_detected`: simula o crash com `drop` sem `release` e espera o lock ficar livre no servidor (`pg_advisory_lock` com timeout, sem `sleep`; o portão Rule7a barrou a primeira versão, que usava `sleep`).
- `session_lease_key_is_the_big_endian_head_of_the_uuid` (unitário).
- O teste antigo `startup_reconcile.rs` passou a envelhecer a sessão semeada além da carência. Rodou contra o Docker do M1 e o Postgres real.

**Medido:**
- Testes de reconciliação com Postgres: 4/4 verdes. Unitários do `startup`: 7/7.
- **Mutante** (ignorar o lock, `if !owner_gone && false`): o teste do lease **reprova** com "a session whose owner holds the lease must stay running". Restaurado em seguida.
- Portões do CI: todos com status 0.
- Suíte completa com banco: **1.129 passaram, 0 falharam, 13 ignorados** (53 binários; +3 testes de integração, +1 unitário).

**Não coberto:** não há teste de ponta a ponta com dois processos `stepyard execute` reais e Docker concorrendo. O lease e a reconciliação foram testados na API da biblioteca.

## 2. Slack `target` + filtro `shell_quote` nos workflows — ✅

**Defeito.**
- O `target` digitado no Slack (`fix issue 1;id`) passava sem validação (`src/slack/mod.rs:108-112`) e chegava a `gh issue view {{ target }}`, executado via `sh -c`.
- Não existia filtro de shell-quote no Tera. Os workflows embutidos interpolavam em `sh -c` o `target`, nomes de arquivo do repositório auditado (`cat {{ scope.value }}`), stdout de passos anteriores e respostas de LLM, essas últimas inclusive dentro de heredocs, onde uma linha igual ao terminador encerraria o heredoc e executaria o resto.

**Correção.**
- `stepyard_harness::template_engine()` (`crates/stepyard-harness/src/render.rs`): instância Tera única, usada pelo v2 (`render`) e pelos dois pontos do v1 (`src/engine/context.rs`, `src/engine/template.rs`). Registra o filtro `shell_quote`, que transforma string, número ou booleano numa única palavra entre aspas simples POSIX e recusa listas e mapas. O item 3 vai mexer nesta mesma fábrica.
- Slack: `fix issue`, `review pr` e `fix ci` só aceitam `^[0-9]{1,10}$`. `security audit` e `generate docs` só aceitam URL de repositório (target `.`) ou caminho relativo `[a-z0-9_./-]` sem `-` inicial e sem `..`.
- Os 12 workflows embutidos foram revisados:
  - Toda interpolação em `run:` passou a usar `| shell_quote`.
  - `echo "{{ … }}"` virou `printf '%s\n' {{ … | shell_quote }}`.
  - Heredocs alimentados por template viraram `printf`.
  - `cat`/`mkdir` ganharam `--`.
- **Ficaram sem filtro, de propósito:**
  - `stack.*`: comandos que o próprio usuário configurou para serem executados.
  - `scope.index`: inteiro.
  - `steps.extract_issue_id.*`: saída de `grep -oE '[0-9]+$'`, só dígitos.

**Testes.**
- `tests/workflow_shell_quote.rs` (portão determinístico, regra 3): percorre o YAML de todos os workflows embutidos. Toda expressão `{{ … }}` dentro de `run:` precisa terminar em `| shell_quote` ou estar na lista de exceções, e não pode haver template dentro de heredoc. Um meta-teste prova que o portão pega `{{ target }}` sem filtro e template em heredoc.
- `crates/stepyard-harness/tests/injection_negative.rs`: execução real em `sh -c` com um arquivo chamado `$(touch pwned).py; touch pwned2`. Com o filtro, o arquivo é lido como nome; o **controle** sem filtro executa os dois `touch`. O nome do arquivo segue a convenção que o portão G3 reserva para testes que montam `sh -c` de propósito. A primeira versão, dentro de `render.rs`, foi barrada pelo G3.
- Unitários do filtro (`render.rs`): aspas, `;`, vazio, recusa de lista.
- Slack (`src/slack/mod.rs`, exigem `--features slack`): recusa `1;id`, `#1$(id)`, `` `id` ``, `7|id`, `main`, `3&&id` e número com 11 dígitos; recusa caminhos com `;`, `$(…)`, `..`, `-rf` e `|`; aceita número, URL de issue, caminho relativo e URL de repositório.

**Medido:**
- **Ponta a ponta pela CLI** (`stepyard execute workflows/simple-test.yaml --no-sandbox -- "x'; touch M; echo '"`, com Postgres real):

  | Workflow | v1 | v2 |
  |---|---|---|
  | Novo | não executou o `touch` | não executou o `touch` |
  | Antigo (`HEAD`, controle) | **executou** | **executou** |

  Um primeiro payload (`x; touch M`) não disparava nem no workflow antigo, porque o valor estava entre aspas simples. O controle mostrou que aquele instrumento não distinguia nada, e por isso o payload foi trocado.
- **Mutante** no Slack (aceitar qualquer target): o teste reprova em `must refuse "fix issue 1;id"`. Restaurado.
- Testes do Slack: 5/5. `cargo clippy -p stepyard-engine --features slack --all-targets -- -D warnings`: status 0. Essa feature continua fora do CI (MANUT-05, P2).
- Portões do CI: todos com status 0. Suíte completa com banco: **1.134 passaram, 0 falharam, 13 ignorados**.

**Não coberto.**
- Workflows de terceiros continuam podendo interpolar sem filtro. O portão cobre só os workflows embutidos.
- O `target` de `git diff {{ target | shell_quote }}...HEAD` ainda pode começar com `-` e ser lido como opção do git. Não há execução de comando arbitrário, e isso fica como higiene para P2.

## 3. Tera sem `get_env` — ✅

**Defeito.** O Tera 1.20.1 registra `get_env` incondicionalmente em `Tera::default()`, e os templates são renderizados no host, mesmo com sandbox. Assim, YAML de workflow, prompts do repositório analisado e `.stepyard/defaults.yaml` podiam ler qualquer variável do host (`{{ get_env(name="GH_TOKEN") }}`) e levá-la ao contêiner ou a um prompt enviado ao LLM.

**Correção.**
- `template_engine()` (a mesma fábrica do item 2, usada pelo v1 e pelo v2) sobrescreve `get_env` com uma função que sempre falha. A mensagem orienta a usar `--var KEY=VALUE`. Nenhum workflow nem documento do projeto usava `get_env` (conferido com `rg`).
- Efeito colateral útil: no v1, os erros de template passaram a incluir a cadeia de causas do Tera (`tera_error_chain` em `src/engine/context.rs`). Antes, o usuário via só "Failed to render '__tmpl__'", sem saber o motivo. Isso valia para qualquer erro de template no v1, por exemplo variável inexistente.

**Testes.**
- v2 (`render.rs`): `get_env` com e sem `default` e via `{% set %}` falha com "get_env is disabled", e a mensagem não ecoa o valor de `PATH`.
- v1 (`src/engine/context.rs`): `get_env` recusado; `shell_quote` disponível no v1 (`gh issue view '1;id'`).

**Medido:**
- **Mutante** (sem o `register_function`): os dois testes reprovam. Restaurado.
- Os 348 testes da lib do engine passam, inclusive os que verificam texto de erro.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.139 passaram, 0 falharam, 13 ignorados**. Os testes da lib do engine contam duas vezes (lib e binário, achado TEST-05), por isso +5 para 3 testes novos.
