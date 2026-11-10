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
| 3 | Tera sem `get_env` | SEG-RUST-04 | ✅ feito | `2d5ff9f` |
| 4 | API: `dispatch_log_path` e segredos no log SSH | SEG-TS-01, SEG-TS-02 | ✅ feito | `9adbd5d` |
| 5 | API resiliente a falhas do Postgres | BUG-CLI-05, SEG-TS-03 | ✅ feito | (este) |
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

## 4. API: `dispatch_log_path` e segredos no despacho SSH — ✅

**Defeitos.**
- **SEG-TS-01:** `POST /api/events` aceitava `dispatch_log_path` livre. Depois, `GET /api/workflows/:id/logs/stream` abria esse caminho e devolvia o conteúdo. Qualquer portador do segredo de ingestão lia arquivos do contêiner da API, inclusive a chave SSH do root montada pelo compose.
- **SEG-TS-02:** no modo SSH, `GH_TOKEN`, `ANTHROPIC_API_KEY` e `DATABASE_URL` iam embutidos no comando remoto. Ficavam visíveis no argv do `ssh` (no `ps` do host da API), no argv do `bash -lc` remoto e no `logger.info` de cada despacho.

**Correção.**
- `packages/api/src/dispatch.ts` (novo): concentra as funções puras do despacho, que antes estavam soltas em `routes/workflows.ts`.
- **Caminho do log:** `allowedDispatchLogPath` só aceita um arquivo `stepyard-dispatch-*.log` **diretamente** dentro de `STEPYARD_DISPATCH_LOG_DIR` (padrão `/tmp`). A checagem roda em toda leitura, porque linhas antigas do banco podem conter caminhos plantados. `readDispatchLogChunk` abre com `O_NOFOLLOW` e usa `fstat` no mesmo descritor, recusando symlink com nome permitido.
- `routes/events.ts`: o schema não aceita mais `dispatch_log_path`. O Zod descarta a chave, então CLIs antigos que a enviam continuam funcionando. O upsert passa `null`, e o `COALESCE` preserva o valor gravado pelo handler de despacho.
- **Segredos no SSH:** `buildSshCommand` devolve `{ binary, args, stdin }`. As variáveis encaminhadas viram linhas `export NOME='valor'` escritas no **stdin** do `ssh`. O lado remoto faz `eval "$(cat)"` antes do `exec stepyard`. Nomes inválidos em `STEPYARD_SSH_ENV_FORWARD` são ignorados com aviso. O handler usa `stdio: ['pipe', …]` só no modo SSH, fecha o stdin logo após escrever e trata `EPIPE` sem derrubar o processo. Como o argv não carrega mais segredo, o `logger.info` também não.
- Nada muda no host remoto: não precisa de `AcceptEnv` nem de configuração nova.

**Testes** (`packages/api/src/dispatch.test.ts`, 7 testes novos; a API tinha 5):
- O caminho aceita só o formato certo e recusa `/root/.ssh/id_ed25519`, `..`, subdiretório, outro nome, sufixo extra, byte nulo, vazio e não-string. Um log real é lido. Um arquivo existente fora do diretório é recusado. Um **symlink** com nome permitido é recusado. O `EventSchema` descarta `dispatch_log_path`.
- O argv não contém o segredo nem a senha do banco; o stdin contém os `export`.
- **Protocolo de verdade:** o último argumento do `ssh` (o que o sshd entrega ao shell remoto) roda em `sh -c` com o stdin que a API envia, usando `/bin/sh` como "engine". O processo final recebe **exatamente** o segredo, que contém `'`, `$(touch PWNED)`, `"` e crases, e nada é executado.

**Medido:**
- **Mutante M1** (sem checagem de diretório e nome): 2 testes de caminho reprovam. O do symlink continua passando, porque o `O_NOFOLLOW` é uma barreira independente.
- **Mutante M2** (exports de volta na linha de comando remota): o teste do argv reprova.
- Job TypeScript do CI: build da API, build do web e testes, todos com status 0 (**12/12**).

**Não coberto.**
- Não houve um `ssh` real até um host remoto. O teste executa o comando remoto localmente, com o mesmo stdin.
- O CLI Rust ainda envia `dispatch_log_path` (`src/events/subscribers.rs:206`). Agora é ignorado; a remoção do envio fica para P2.

**Pendência do Allan:** a correção impede novos vazamentos, mas os logs já gravados pelo contêiner da API (`docker logs`) continuam com os valores. **Rotacionar `GH_TOKEN`, `ANTHROPIC_API_KEY` e a senha do banco** de qualquer ambiente onde o despacho SSH rodou.

## 5. API resiliente a falhas do Postgres — ✅

**Defeito.**
- O `Pool` do `pg` não tinha listener de `'error'`. Quando um cliente ocioso perdia a conexão (restart do Postgres, falha de rede), o `EventEmitter` lançava exceção e o processo da API terminava.
- Handlers `async` no Express 4 não encaminham rejeições: `pool.connect()` falhando, `ROLLBACK` numa conexão morta ou `setHeader` com valor inválido no export de CSV viravam *unhandled rejection*, que encerra o Node ≥ 15. Sob `tsx watch`, o processo não reinicia.

**Correção** (`packages/api/src`):
- `errors.ts` (novo):
  - `forwardAsyncErrors(Router())` envolve todo handler registrado; rejeição ou exceção síncrona vira `next(err)`. Middleware de erro, de aridade 4, fica intocado.
  - `errorHandler` responde JSON **sem** mensagem nem stack (podem conter SQL, caminhos ou string de conexão): 503 `DATABASE_UNAVAILABLE` para falha de conexão (códigos de rede e SQLSTATE 08xxx/57P0x), 500 para o resto, e o status 4xx do body-parser (JSON malformado, corpo grande demais). Em resposta de streaming já iniciada (CSV, SSE), só encerra.
- Os quatro routers assíncronos (events, workflows, analytics, overview) passaram a usar o wrapper. `index.ts` registra o `errorHandler` depois de todas as rotas e um `unhandledRejection` que só registra no log, como última defesa.
- `db.ts`: `pool.on('error', …)` registra no log e descarta o cliente; `connectionTimeoutMillis` (padrão 5 s, configurável por `POSTGRES_CONNECT_TIMEOUT_MS`) faz a requisição falhar rápido em vez de ficar pendurada.
- `events.ts` e export de CSV: o `ROLLBACK` não pode mais escapar do `catch`. Se ele falhar, o cliente é liberado com `release(err)`, e o `pg` o **destrói** em vez de devolvê-lo ao pool.

**Testes** (`packages/api/src/errors.test.ts`, 7 novos, com servidor HTTP real em porta efêmera):
- Rejeição com código de conexão → 503 sem vazar a string de conexão; rejeição comum e exceção síncrona → 500 sem vazar o SQL; JSON malformado → 400; corpo grande → 413; nenhum `unhandledRejection` durante os testes.
- `POST /api/events` com `pool.connect()` rejeitando → 503. Com `BEGIN` e `ROLLBACK` falhando → 500, e `release` recebe um `Error`.
- O pool real tem listener de `'error'`, e emitir o evento não lança exceção.

**Medido:**
- **Ponta a ponta com Postgres real** (API rodando com `tsx` contra o Postgres efêmero no M1, depois `docker restart` no Postgres):

  | Código | Antes do restart | Durante | Depois de o banco voltar | Processo da API |
  |---|---|---|---|---|
  | Novo | 201 | 503, depois 201 | 201 | **vivo**; o log registra "Idle Postgres client error; client discarded" |
  | `HEAD` (controle) | 201 | sem resposta | sem resposta | **morreu** com `throw er; // Unhandled 'error' event` |

  A primeira execução usava porta publicada aleatória, que mudou no restart e produziu um falso "503 depois de voltar". Refiz com porta fixa. Um primeiro "controle" também não valeu: um erro de shell meu (zsh sem divisão de palavras) fez rodar o código novo. Refeito em bash, com restauração por `trap`.
- **Mutantes:** M1 (wrapper sem efeito) → 4 testes reprovam, com *unhandled rejection* registrada pelo vitest. M2 (sem `pool.on('error')`) → reprova o teste do pool. M3 (`release()` sem descartar) → reprova o teste do ROLLBACK. Restaurados.
- Job TypeScript do CI: build da API, build do web e testes, todos com status 0 (**19/19**).

**Não coberto.**
- O segundo pool, morto, em `src/db/index.ts` (nada o importa; MANUT-16) não foi alterado.
- O SSE de logs já tinha `.catch` no loop e não mudou.
