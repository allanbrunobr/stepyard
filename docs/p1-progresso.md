# Progresso das correções P1 — análise de 09/11/2026

Referência: [`docs/analise-20261109.md`](analise-20261109.md), seção 3 (P1). Um commit local por item, na ordem da tabela P1 (gravidade). Nada foi empurrado.

## Como cada item é verificado

- **Testes com Postgres real.** Os testes de banco rodam contra um Postgres 16 efêmero no daemon Docker remoto (`m1-docker`), alcançado pelo IP Tailscale do M1. O contêiner tem `--rm`, uma senha descartável e é removido ao fim de cada execução. Isso corrige a seção 1 do relatório, que dava esses testes como "não medidos": a porta não responde em `127.0.0.1`, mas responde pelo IP Tailscale.
- **Linha de base com banco, antes de qualquer correção** (commit `ea26129`): `cargo test --workspace --no-fail-fast` → **1.125 passaram, 0 falharam, 13 ignorados** (52 binários). Os 94 testes dependentes de banco passam contra Postgres real.
- **Portões por item, iguais aos do CI:** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `scripts/audit-patterns.sh`, `cargo xtask audit-emit-before-io`, perfil SQLite (`cargo check` do engine e `clippy` do session). Por fim, a suíte inteira com banco.
- **Job SQLite do CI** (a partir do item 7): `cli_lite_smoke` e testes do `stepyard-session` com `--features sqlite` entram na verificação de cada item. Os portões já cobriam o `check` e o `clippy` desse perfil, mas não os testes.
- **O teste novo precisa distinguir.** Para cada correção, injeto o defeito antigo (mutante) e confirmo que o teste novo reprova na asserção certa. Só então restauro o código.

## Itens

| # | Item P1 | Achados | Status | Commit |
|---|---|---|---|---|
| 1 | Reconciliação de partida só para sessões órfãs | ARQ-01, BUG-CLI-01, BUG-HARNESS-01 | ✅ feito | `529b532` |
| 2 | Slack `target` + filtro `shell_quote` nos workflows | SEG-RUST-02, SEG-RUST-07 | ✅ feito | `fb6c43f` |
| 3 | Tera sem `get_env` | SEG-RUST-04 | ✅ feito | `2d5ff9f` |
| 4 | API: `dispatch_log_path` e segredos no log SSH | SEG-TS-01, SEG-TS-02 | ✅ feito | `9adbd5d` |
| 5 | API resiliente a falhas do Postgres | BUG-CLI-05, SEG-TS-03 | ✅ feito | `a64c4d1` |
| 6 | Contrato de despacho dashboard/remote | ARQ-02, BUG-CLI-03/04/07, MANUT-04/07, SEG-TS-09 | ✅ feito | `8f084a3` |
| 7 | Adaptador v1→v2: `timeout` e `fail_on_error` | BUG-CLI-02, MANUT-01/02, ARQ-06 (parcial) | ✅ feito | `2e0a0d7` |
| 8 | v1 `parallel` honra `fail_on_error` resolvido | BUG-MOTOR-08 | ✅ feito | `916aaa3` |
| 9 | Workspace v2: timeout e caminhos excluídos | BUG-HARNESS-02, SEG-RUST-08 | ✅ feito | `584bbee` |
| 10 | Sandbox: volumes do YAML e bloqueio do v1 `--sandbox` | SEG-RUST-03, SEG-RUST-01 (parcial), BUG-MOTOR-04, ARQ-04, SEG-RUST-11 | ✅ feito | (este) |

## Pendências que são do Allan (exigem conta, login, gasto ou segredo)

- **`AUTO_VOLUMES` do sandbox v1** (`src/sandbox/config.rs`): no modo `--repo`, usado pelo Slack e pelo despacho, o contêiner ainda recebe `~/.claude` em **leitura e escrita** e `~/.ssh` somente leitura. Um agente manipulado pelo repositório analisado pode plantar hooks ou configurações em `~/.claude` que depois rodam no host, e pode ler as chaves SSH. Não mexi nisso porque montar `~/.claude` só para leitura ou tirar `~/.ssh` pode quebrar a autenticação do Claude CLI (OAuth) e o envio de commits por SSH de dentro do contêiner, e verificar isso exige a sua conta. Sugestão: testar `~/.claude` como `:ro`, com cópia só do arquivo de credencial, e trocar o remoto SSH por HTTPS com `GH_TOKEN`.
- **Imagem no M1:** a imagem `stepyard-sandbox:latest` (1,63 GB) foi **construída por mim** no daemon remoto `m1-docker` durante o mutante M4 do item 6. Sem a recusa, o v2 seguiu adiante e o pré-voo construiu a imagem. Os itens 9 e 10 usaram essa imagem nos testes Docker. Não a removi: apagar algo fora do repositório não é permitido nesta sessão. Fica a seu critério (`docker --context m1-docker rmi stepyard-sandbox:latest`).

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

## 6. Contrato de despacho dashboard/remote — ✅

**Defeito.** O despacho do dashboard e do `stepyard remote` montava uma linha de comando que o motor padrão (v2):
- **rejeitava:** `--var branch=…`, chave minúscula, barrada pelo clap;
- **ignorava:** com `--repo`, o workflow rodava num diretório vazio, e o v2 não reporta ao dashboard.

A API respondia 202, o processo filho morria, e o run ficava em `dispatching` para sempre. O exemplo literal de `docs/REMOTE.md` (`--var foo=1`) não funcionava. Além disso, o v2 aceitava em silêncio `--repo`, `--resume` e `--json`, e `--timeout` não tinha efeito em nenhum motor sem que nada avisasse.

**Correção.**
- **CLI** (`src/cli/commands.rs`):
  - Com `--engine v2` (o padrão), `--repo`, `--resume`, `--json` e `--dry-run` são **recusados** antes de qualquer trabalho de sandbox, imagem ou banco. A mensagem lista todas as flags de uma vez e aponta `--engine v1`, e mantém o formato "does not support …" que o teste antigo do `--dry-run` espera.
  - `--timeout`, nos dois motores, e `--quiet`, no v2, geram aviso explícito de que não têm efeito.
- **API** (`routes/workflows.ts`):
  - O despacho passa `--engine v1` explícito, a mesma escolha do bot do Slack, e `--var BRANCH=…`.
  - As chaves de `vars` são validadas no Zod com a regex do CLI, então uma chave inválida dá 400 antes de qualquer spawn.
  - `child.on('exit')` marca `dispatch_failed`, com o código de saída e o caminho do log, quando o processo morre antes de reportar. O `UPDATE` só afeta runs sem nenhum evento.
- **`stepyard remote`** (`src/cli/remote.rs`): `parse_kv_vars` aplica a mesma regex no cliente.
- **Documentação:** a tabela de limitações do v2 no README ("Ignored" → "Rejected with an error" / aviso) e `docs/REMOTE.md` (exemplo com chaves maiúsculas e um parágrafo sobre `--engine v1`, `BRANCH` e `dispatch_failed`).

**Coerência com o item 10:** o despacho usa v1. Por isso, o bloqueio do sandbox v1 previsto no item 10 vai mirar o modo que monta o diretório **do host**, e não o modo `--repo`, usado pelo despacho e pelo Slack.

**Testes.**
- `tests/cli_v2_flags.rs` (binário real com `assert_cmd`, sem Docker nem banco): as 4 flags recusadas, uma a uma; a lista combinada; o aviso do `--timeout`.
- `packages/api/src/routes/dispatch.test.ts` (servidor HTTP real e um "stepyard" falso que grava o argv e sai com 2):
  - O argv começa com `execute --engine v1`, contém `BRANCH=feature/x` e não `branch=`.
  - O run é marcado `dispatch_failed` com "exit code 2".
  - Uma chave `foo` dá 400 sem tocar no banco.
- `src/cli/remote.rs`: recusa `foo`, `branch`, `1X` e `A-B`.

**Medido:**
- **Mutantes:** M1 (despacho antigo: sem engine, `branch=`), M2 (sem `on('exit')`) e M3 (sem regex nas chaves) reprovam cada um o seu teste na API. M4 (v2 volta a aceitar as flags) reprova os 2 testes do binário. Sem a recusa, o v2 tentou preparar o sandbox no Docker do M1 até o timeout do `assert_cmd`; conferi que não sobrou contêiner nem processo. Tudo restaurado.
- Portões do CI: todos com status 0. Job TypeScript: status 0 (**21/21**). Suíte Rust completa com banco: **1.144 passaram, 0 falharam, 13 ignorados**.

**Não coberto.**
- O v2 continua sem suporte a `--repo` e sem eventos para o dashboard. Essas lacunas mantêm o v1 vivo e fazem parte do plano de desativação do v1 (P3).
- `--sandbox`, que já é o padrão e não muda nada, não foi tocado.

## 7. Adaptador v1→v2: `timeout` e `fail_on_error` — ✅

**Defeito.** O adaptador v1→v2 descartava todo o `config` dos passos `cmd` e `agent` (`src/cli/harness_adapter.rs:479-484`). Com isso, no motor padrão:
- `fail_on_error: false`, usado em 10 dos 12 workflows embutidos (por exemplo o `lint` do `fix-issue`, seguido de um gate e de um agente que corrige), abortava o run no primeiro código de saída diferente de zero;
- nenhum `timeout` era aplicado: nem o `config.global.timeout`, nem o `config.cmd.timeout`, nem o próprio do passo. Os workflows embutidos declaram timeouts de propósito (por exemplo `fix-issue.yaml:124`, 600 s no agente), e `cmd` e `agent` rodavam sem limite.

**Correção.**
- **Harness** (`crates/stepyard-harness`): campo novo `Step::allow_failure` (só `cmd`). Com ele, um código de saída ≠ 0 conclui o passo, e o snapshot guarda o código real para o gate seguinte. É honrado nos três pontos de decisão:
  - `engine.rs`, em `execute_cmd_with_select`, que também cobre os corpos de repeat/map/call;
  - os dois pontos do `parallel` em `scope.rs`: o dreno que abortava os irmãos e a emissão de eventos.
- **Adaptador:**
  - `timeout` e `fail_on_error` são resolvidos pelo **mesmo** `ConfigManager` do v1 (global < tipo < padrão de nome < passo), inclusive dentro de corpos de `parallel`, e então mapeados para `Step::timeout` e `Step::allow_failure`.
  - Sem nenhuma camada definindo `timeout`, valem os fallbacks do v1: 60 s para cmd, 600 s para agent.
  - Valor malformado (`"soon"`, número sem unidade, `fail_on_error: "no"`) passa a ser erro de carga (`AdapterError::InvalidStepConfig`), e não um default silencioso.
- **Documentação:** `docs/CONFIG.md` ganhou a linha de `fail_on_error` e uma nota sobre o que os dois motores aplicam.

**Mudança de comportamento (atenção):** no v2, que é o padrão, `cmd` e `agent` agora **têm** timeout, como sempre tiveram no v1 e como a `CONFIG.md` prometia. Com os defaults embutidos, cmd para em 60 s e agent em 300 s (`config.global.timeout`). Um workflow que dependia de rodar sem limite no v2 precisa declarar `timeout`. Também: uma config de usuário com `timeout: 30` (número sem unidade), que o v1 ignorava em silêncio, agora é recusada no v2.

**Interação com o item 9:** com timeouts ativos, o defeito do item 9 (timeout destrói o contêiner **antes** da cópia de volta, BUG-HARNESS-02) fica mais fácil de disparar no modo Docker. O item 9 corrige isso. Por isso convém não usar este commit isoladamente sem os seguintes.

**Testes.**
- Adaptador (`harness_adapter.rs`): as quatro camadas (tipo vence global, padrão `slow_*` vence tipo, passo vence tudo, agent herda global, filho de `parallel` resolvido); fallbacks sem config; valores malformados recusados; o `lint` do `fix-issue.yaml` embutido sai com `allow_failure` e 60 s.
- **Ponta a ponta pelo binário v2 com Postgres real** (`tests/cli_v2_step_config.rs`):
  - `exit 3` com `fail_on_error: false` → o gate vê `exit_code == 3` e o run chega ao passo seguinte;
  - o mesmo com `true` → o run falha;
  - sub-passo de `parallel` tolerado não aborta o irmão;
  - `timeout: "1s"` corta um `sleep 30` em menos de 20 s.
- Dois testes do harness que montam `Step` literal ganharam o campo novo.

**Medido:**
- **Mutantes**, todos contra o ponta a ponta com Postgres: M1 (engine ignora `allow_failure`) reprova só o teste do gate; M2 (`parallel` ignora) reprova só o do `parallel`; M3 (adaptador não mapeia o timeout) reprova só o do timeout, que esperou os 30 s inteiros. Restaurados.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.156 passaram, 0 falharam, 13 ignorados**. Job SQLite: smoke 5/5 e session verde.

**Não coberto.**
- `idle_timeout` e os demais campos que o adaptador ainda descarta (`output_type`, `async_exec`, `events`, `plugins`; ARQ-06) não entraram aqui. Recusar esses campos com erro é uma mudança maior, que fica para P2.
- O `chat` continua com o caminho próprio de timeout, que já existia.

## 8. v1 `parallel` honra o `fail_on_error` resolvido — ✅

**Defeito.** No v1, o `parallel` calculava o config resolvido de cada sub-passo (`resolve_scope_step_config`, linha 62) e o **descartava** em `dispatch_step` (`_config`), reconstruindo o config só com o inline do passo. Com isso, o default `cmd.fail_on_error: true` se perdia, `get_bool` caía em `false`, e um `cargo test` que saía com 101 dentro de um `parallel` era registrado como sucesso. O teste antigo do módulo já denunciava o problema: para forçar uma falha, usava um tipo de passo não suportado, porque um `cmd` com `exit 1` não falhava.

**Correção** (`src/steps/parallel.rs`): `dispatch_step` usa o config resolvido nas quatro camadas, como map, repeat e call já faziam. O motor v1 passa o `ConfigManager` ao `ParallelExecutor` (`src/engine/mod.rs:995`), então a correção vale na execução real.

**Testes.**
- Unitários: com os defaults embutidos, `exit 101` em um sub-passo faz o `parallel` falhar; com `fail_on_error: false` inline, a falha é tolerada.
- **Ponta a ponta pelo binário v1 com Postgres real** (`tests/cli_v1_parallel_fail_on_error.rs`): o run falha quando um sub-passo sai com 101 e termina com sucesso quando o sub-passo declara `fail_on_error: false`.

**Medido:**
- **Mutante**, que é o código antigo (config inline só): reprova o teste de ponta a ponta e o unitário de falha; os de tolerância continuam passando. Restaurado.
- O clippy barrou a primeira versão (alias que gerava 4 empréstimos desnecessários), e o alias saiu.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.162 passaram, 0 falharam, 13 ignorados**. Job SQLite: smoke 5/5 e session 11/11.

## 9. Workspace v2: timeout e caminhos excluídos — ✅

**Defeitos.**
- **BUG-HARNESS-02:** em três pontos (timeout de cmd, idle-timeout de cmd e timeout de sub-passo de `parallel`), o harness fazia `docker rm -f` no contêiner da sessão **antes** de `finalise_fail`, que é quem copia o `/workspace` de volta. Tudo o que passos anteriores tinham mudado se perdia. O item 7 tornou isso mais provável, porque o v2 agora aplica timeouts.
- **SEG-RUST-08:** a cópia de ida exclui `target`, `node_modules`, `.stepyard`, `.venv` e `__pycache__` por componente de caminho, mas o `.git` copiado ainda lista os arquivos versionados sob eles. O `git status` dentro do contêiner os reportava como deletados, e a cópia de volta **os apagava no host** (por exemplo `.stepyard/defaults.yaml` e `src/target/mod.rs`).

**Correção** (`crates/stepyard-harness`):
- Nos três pontos, a ordem passou a ser `finalise_fail()` (copia de volta, destrói o contêiner e marca a sessão) e só então `destroy_by_session`. Este último é idempotente e continua parando o processo que estourou o tempo quando não há workspace de contêiner. O portão G4 (emit-before-IO) segue verde.
- `container_workspace::without_excluded_deletions`: antes da cópia de volta, descarta as **deleções** de caminhos que casam com um padrão de exclusão, com a mesma regra do tar: padrão sem `/` casa qualquer componente, com `*` e `?`; padrão com `/` casa o prefixo. Adições e modificações seguem como antes.

**Testes.**
- **Ponta a ponta com Docker real (M1) e Postgres real** (`tests/v2_container_workspace_docker.rs`, `#[ignore]` pela convenção do projeto; roda com `STEPYARD_TEST_DOCKER=1 … -- --ignored`), pelo binário v2 com `--sandbox-runtime docker` num repositório git temporário:
  1. o passo 1 cria um arquivo e o passo 2 estoura o timeout de 3 s → o arquivo chega ao host;
  2. o mesmo com o timeout num sub-passo de `parallel`;
  3. arquivos versionados em `.stepyard/`, `src/target/` e `web/node_modules/` sobrevivem a um run que altera o `README.md`, e a alteração real é copiada.
- Unitários: filtragem das deleções (incluindo `targets/`, que **não** é excluído, e uma adição em `target/`, que é mantida) e casamento de padrões no estilo tar.

**Medido:**
- **Antes da correção**, os testes 1 e 3 **reprovaram** no código atual. O teste 3 reprovou primeiro por erro meu de fixture (`.stepyard/defaults.yaml` com conteúdo que o CLI não aceita). Corrigido o fixture, ele reprovou pelo motivo certo: "`.stepyard/defaults.yaml` was deleted from the host by the copy-back".
- **Depois da correção**, os 3 passam. **Mutante** que desfaz só a reordenação no `parallel`: o teste 2 reprova; restaurado. Nenhum contêiner `stepyard-session-*` sobrou no M1.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.164 passaram, 0 falharam, 16 ignorados** (+3 testes Docker, rodados à parte com `--ignored`). Job SQLite: verde.

**Não coberto.**
- No timeout, o processo que estourou o tempo ainda roda dentro do contêiner durante a cópia de volta, então um arquivo que ele estava escrevendo pode chegar pela metade. Matar só os processos do `docker exec` (BUG-HARNESS-05) fica para P2.
- O cancelamento (`finalise_cancel`) continua sem cópia de volta, e esse comportamento não mudou.
- Os testes Docker não rodam no CI (TEST-02, P2).

## 10. Sandbox: volumes do YAML e bloqueio do v1 no diretório do host — ✅

**Defeitos.**
- **SEG-RUST-03:** `config.global.sandbox.volumes`, no YAML do workflow ou no `.stepyard/config.yaml` do projeto analisado, ia sem filtro para `docker run -v` nos **dois** motores. Um workflow de terceiro podia montar `/`, `$HOME` ou `/var/run/docker.sock` e sair do sandbox, justamente no modo em que o usuário pediu isolamento.
- **SEG-RUST-01 / BUG-MOTOR-04:** fora do modo `--repo`, o sandbox v1 monta o diretório de trabalho do host em leitura e escrita em `/workspace`, extrai a cópia por cima dele e roda `chown -R stepyard` nele. O contêiner escreve nos arquivos reais e troca o dono deles (no Linux, uid 1000).
- **ARQ-04 / SEG-RUST-11:** as cópias do v1 (`copy_workspace`/`copy_results`) montam `sh -c` com dados do YAML e não recusam symlink. O fix `925bb8b` entrou só no v2. Essas cópias só rodam no modo do diretório do host.

**Correção** (`src/cli/commands.rs`, `src/sandbox/config.rs`), aplicada antes de qualquer contato com o runtime de contêiner:
- **v1 com sandbox e sem `--repo`: recusado**, com mensagem que aponta as três saídas (v2 padrão, que copia o projeto; `--engine v1 --repo OWNER/REPO`; `--no-sandbox`). Isso elimina o bind mount do diretório do host, o `chown -R` e os dois caminhos de cópia via shell do v1. O modo `--repo`, usado pelo Slack e pelo despacho, monta um diretório temporário e clona dentro do contêiner, e continua disponível.
- **`check_workflow_volumes`**, nos dois motores: recusa entrada cuja origem no host resolve **fora do diretório do projeto**. Symlinks são resolvidos e `..` é normalizado, inclusive quando o caminho ainda não existe. Volumes nomeados, volumes anônimos e caminhos dentro do projeto passam. Opt-in do usuário: `STEPYARD_ALLOW_WORKFLOW_MOUNTS=1` no ambiente do `stepyard`, que um arquivo de workflow não consegue definir. A verificação vale também para volumes vindos dos defaults do próprio usuário, então quem monta algo de fora por `~/.stepyard` precisa do opt-in.
- **Documentação:** `docs/DOCKER-SANDBOX.md` (seção nova de volumes declarados e o modo do v1 recusado) e `docs/SLACK.md`. Consequência: mensagem no Slack **sem** URL de repositório não roda mais com sandbox. Antes, rodava no diretório do próprio bot, com bind mount e `chown`.

**Testes.**
- Unitários (`src/sandbox/config.rs`): aceita `./data`, caminho absoluto dentro do projeto, volume nomeado, volume anônimo e caminho ainda inexistente dentro do projeto. Recusa `/`, `docker.sock`, `~/.ssh`, `~`, `../outside`, `<projeto>/../elsewhere` e um **symlink** dentro do projeto que aponta para fora. O opt-in libera.
- Binário (`tests/cli_sandbox_policy.rs`): `DOCKER_HOST` aponta para um socket inexistente, para que um teste que deixasse passar errado não chegue a nenhum daemon real.
  - v1 com sandbox no diretório do host → recusado;
  - v1 `--repo` e v1 `--no-sandbox` → não barrados pela política;
  - v2 com `/:/host`, `docker.sock` e `~/.ssh` → recusados, citando a entrada;
  - volumes do projeto passam; o opt-in tira a recusa e o run para em seguida no Docker inexistente.

**Medido:**
- **Mutantes:** M1 (sem o bloqueio do v1) reprova só o teste do v1; M2 (política que aceita tudo) reprova só o teste dos volumes. Restaurados.
- O teste já existente que roda v1 (`tests/injection_negative.rs:217`) usa `--no-sandbox` e não é afetado.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.176 passaram, 0 falharam, 16 ignorados**. Job SQLite: verde.

**Não coberto.**
- `AUTO_VOLUMES` do modo `--repo` (`~/.claude` rw e `~/.ssh` ro): é pendência sua, porque exige conta para verificar (veja a seção de pendências acima).
- `image:` do YAML continua livre. A imagem roda sem montagens do host além das permitidas, então o risco é menor, e fica para P2.
- O diretório fixo de `/tmp` no modo `--repo` do v1 (BUG-MOTOR-03) é P2.
- O hook de portão de push da sessão barrou uma vez o comando de commit deste item, por falso positivo: o texto do documento continha a palavra do comando de envio. Nada rodou naquela tentativa. O texto foi reescrito, e nenhum envio foi feito.
