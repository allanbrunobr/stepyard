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
| 0 | Contradições do pré-PR do #1 e o dry-run do v1 | checagem pré-PR; regressão do P1-10 | ✅ feito | `d5dfeda` |
| 1 | Segredos do host fora do alcance do YAML (`${VAR}`, `base_url` do chat, env no argv) | SEG-RUST-05 (parcial), SEG-RUST-12 | ✅ feito | `882c323` |
| 2 | Aviso quando o build roda no host por padrão; contêiner de sessão endurecido | SEG-RUST-06, SEG-RUST-15 | ✅ feito | `be5f483` |
| 3 | Perdas de trabalho: diretório por execução no `--repo` do v1, commits feitos no contêiner, sessão presa após conflito de merge | BUG-MOTOR-03, BUG-MOTOR-01, BUG-HARNESS-06 | ✅ feito | `029fef2` |
| 4 | Contrato de eventos do dashboard (`null` recusado pela API; fixture compartilhado) | ARQ-03 (BUG-CLI-08 pendente) | ✅ feito | `9addfd9` |
| 5 | Pânicos por fatiamento de UTF-8 no Slack e no `remote status` | BUG-CLI-06, BUG-MOTOR-14, BUG-CLI-10 (BUG-CLI-09 pendente) | ✅ feito | `29cb4d5` |
| 6 | Upload de artefatos acima de 100 KB (parser por rota, autenticação antes do corpo grande) | SEG-TS-06 (SEG-TS-05/08 pendentes) | ✅ feito | `99fcdf2` |
| 7 | `stepyard validate` valida para o motor padrão (v2) | BUG-CLI-13, MANUT-06 | ✅ feito | `1a08590` |
| 8 | Integridade dos testes: testes de binário que nunca rodavam; mock que confundia `destroy` e `destroy_by_session` | TEST-04, TEST-01 (TEST-02/03/13, MANUT-05 dependem de `.github/`) | ✅ feito | `1d70483`, `8c28951` |
| 8b | A suíte de testes registrava worktrees no repositório do desenvolvedor | achado novo | ✅ feito | (este) |

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

## 1. Segredos do host fora do alcance do YAML — ✅ (parcial)

**Defeitos (SEG-RUST-05, SEG-RUST-12).**
- `env:` com `${NOME}` lia **qualquer** variável do host, inclusive `${GH_TOKEN}` e `${ANTHROPIC_API_KEY}`. O YAML e o `.stepyard/defaults.yaml` do repositório analisado não são confiáveis.
- Um passo `chat` com `base_url` e `api_key_env` livres fazia o **host** enviar o valor de qualquer variável, como chave de API, para um servidor escolhido pelo workflow. O caminho era o mesmo nos dois motores.
- Os valores do `env` dos passos `cmd` no sandbox iam no argv (`docker exec --env K=V`), visível com `ps`.

**Correção.**
- `stepyard_harness::host_var_allowed` (em `resolve_env`): um `${NOME}` com cara de credencial (`TOKEN`, `SECRET`, `PASSWORD`, `CREDENTIAL`, `PRIVATE`, `AUTH`, `COOKIE`, `SESSION`, `DATABASE_URL`, `*_KEY`, `APIKEY`) só é expandido se o usuário o listar em `STEPYARD_ENV_ALLOW`. Os outros (`${HOME}`, `${CI}`) seguem como antes.
- `stepyard_harness::chat_base_url_allowed`, chamado no cliente de chat do v2 (`src/chat_client.rs`) e no do v1 (`src/steps/chat.rs`) **antes** de qualquer requisição: só aceita loopback, com o host analisado como IP (então `127.0.0.1.evil.example` não passa), ou um prefixo listado pelo usuário em `STEPYARD_CHAT_BASE_URL_ALLOW`.
- `stepyard_sandbox_orchestrator::env_by_name`: o argv do `docker`/`podman exec` leva só `--env NOME`, e o valor vai pelo ambiente do processo da CLI. É o mesmo padrão que o caminho do agente (`container.rs`) já usava.
- `docs/CONFIG.md`: seção nova "Host secrets and workflows".

**Mudança de comportamento:** workflow que use `${GITHUB_TOKEN}` (ou outro nome de segredo) no `env:` precisa de `STEPYARD_ENV_ALLOW=GITHUB_TOKEN`. Chat com `base_url` remota (por exemplo um LiteLLM em outra máquina) precisa de `STEPYARD_CHAT_BASE_URL_ALLOW`. O teste antigo `tests/env_cascade.rs`, que expandia `${GITHUB_TOKEN}`, passou a usar o opt-in e ganhou o caso negativo.

**Testes:**
- política de nomes e erro do `resolve_env`;
- política de `base_url` (loopback IPv4/IPv6/`localhost` aceitos; `evil.example`, `127.0.0.1.evil.example`, `localhost@evil.example`, `localhost.evil.example`, `ftp://` e URL sem esquema recusados);
- o chat do v1 recusa uma `base_url` remota sem tocar a rede;
- `env_by_name` não põe valor nem `=` no argv;
- os 4 testes Docker **reais** de `exec_with_env` (M1) passam: o valor ainda chega ao contêiner literalmente, com metacaracteres de shell.

**Medido:**
- **Mutantes:** política de `base_url` aceitando tudo → reprovam os testes do v2 e do v1; `${VAR}` aceitando tudo → reprova o teste do harness; valor de volta ao argv → reprova o teste do helper. Restaurados.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.189 passaram, 0 falharam, 16 ignorados**. Job SQLite: verde.

**Não coberto (resto do SEG-RUST-05):** o subscriber do dashboard do v1 manda o `DASHBOARD_API_SECRET` do host (fallback quando o YAML não define `secret`) para a `url` que o **workflow** define, junto com os `artifacts` que o workflow lista. Corrigir exige decidir se a URL do dashboard pode vir do YAML. Fica registrado para decisão. O `docker create`/`run` do v1 (`src/sandbox/docker.rs`) ainda põe valores de `env` no argv; o v1 só roda em modo `--repo` com sandbox.

## 2. Aviso de runtime no host e contêiner endurecido — ✅

**Defeitos.**
- **SEG-RUST-06:** o binário publicado (perfil SQLite, `release.yml`) usa o runtime `local` por padrão. Um workflow de repositório de terceiro rodava **no host** sem aviso nenhum, e o `--help` dizia "Run inside a Docker sandbox (default: true)".
- **SEG-RUST-15:** o contêiner de sessão do v2 subia sem endurecimento nenhum.

**Correção.**
- `src/cli/commands.rs`: quando o runtime efetivo é `local` e o usuário não o escolheu (nem `--sandbox-runtime`, nem `--no-sandbox`, nem `STEPYARD_SANDBOX`), o CLI imprime no stderr que os passos vão rodar nesta máquina e como isolá-los. O `--help` de `--sandbox` agora descreve o padrão real de cada build.
- `stepyard_sandbox_orchestrator::HARDENING_ARGS`, aplicado no `docker run` e no `podman run` do v2: `--security-opt no-new-privileges`, `--pids-limit 4096` e `--cap-drop` de `NET_RAW`, `MKNOD`, `AUDIT_WRITE`, `SETFCAP` e `SYS_CHROOT`. É conservador de propósito: o script de preparo ainda faz `chown`, e o agente roda com `--user`. Nada na imagem nem no código usa `sudo` ou setuid (conferido).

**Testes.**
- `tests/cli_lite_smoke.rs` (roda no job SQLite do CI): o padrão avisa; `--sandbox-runtime local` explícito não avisa.
- `crates/stepyard-sandbox-orchestrator/tests/lifecycle.rs` (Docker, `#[ignore]`): cria um contêiner de sessão real, executa um comando e confere com `docker inspect` que `SecurityOpt`, `PidsLimit` e `CapDrop` estão aplicados.
- Os testes Docker existentes passaram **com** o endurecimento: os 3 do workspace v2, duas vezes, mais os 9 do orquestrador.

**Medido:**
- **Mutantes:** sem as flags, o teste do `docker inspect` reprova; sem o aviso, o teste do smoke SQLite reprova. Restaurados.
- **Incidente:** a primeira rodada dos testes Docker do workspace teve uma falha, porque o teste estourou o timeout de 300 s. A causa foi que a imagem `stepyard-sandbox:latest`, apagada do M1 pelo Allan, foi **reconstruída** pelo pré-voo do v2 durante esse teste. Com a imagem pronta, as três rodadas seguintes passaram. **A imagem voltou a existir no M1 por causa destes testes.**
- Portões do CI: todos com status 0. Suíte completa com banco: **1.189 passaram, 0 falharam, 17 ignorados** (+1 teste Docker). Job SQLite: verde (smoke 6/6).

**Não coberto.** O descarte de **todas** as capabilities (`--cap-drop ALL` com reposição seletiva) exige revisar o script de preparo e a imagem, e fica para depois. O `docker create` do v1 não recebeu o endurecimento: o v1 só roda em modo `--repo` com sandbox, e sua remoção é o P3.

## 3. Perdas de trabalho restantes — ✅

**Defeitos e correções.**
1. **BUG-MOTOR-03 — diretório fixo no modo `--repo` do v1.**
   - **Defeito:** todo run `--repo` (uma menção no Slack, um despacho) montava o **mesmo** diretório de `/tmp` em `/workspace` e o esvaziava antes de clonar. Dois runs simultâneos destruíam o clone um do outro no meio de um passo. O erro de `create_dir_all` era engolido.
   - **Correção:** `create_repo_workspace` cria um diretório **único e privado (0700)** por execução e propaga erros. O `sandbox_down` o remove no fim, com melhor esforço, porque no Linux o contêiner pode deixar arquivos com outro uid.
2. **BUG-MOTOR-01 — trabalho commitado dentro do contêiner se perdia.**
   - **Defeito:** a cópia de volta comparava a árvore de trabalho com o `HEAD` **do contêiner**. Um arquivo que um passo commitava lá dentro (o `fix-test.yaml` commita antes do push) parecia inalterado e nunca chegava ao host.
   - **Correção:** o script de preparo grava o commit da cópia de ida em `.git/stepyard-base`, e `changed_paths` compara contra ele (`git diff --name-status <base>` mais os não rastreados). Sem o arquivo, cai no `git status` de antes.
3. **BUG-HARNESS-06 — conflito de merge deixava a sessão `running`.**
   - **Defeito:** `finalise_success` saía pelo `?` antes de qualquer status terminal.
   - **Correção:** a sessão é marcada `failed` antes de devolver o erro. O evento `MergeConflict` já estava no log.

**Testes e medição.**
- **Docker real (M1) e Postgres real**, em `tests/v2_container_workspace_docker.rs`: um passo commita `committed.txt` dentro do contêiner, e outro deixa `loose.txt` sem commit. **No código antigo, o teste reprovou** ("a file committed inside the container must reach the host"). No novo, passa, junto com os 3 testes Docker do workspace que já existiam e os 2 do orquestrador, que cobrem o caminho de fallback sem commit-base.
- `crates/stepyard-harness/tests/workspace_finalize.rs` (Postgres): depois do conflito, a sessão está `Failed` na memória **e no banco**. **No código antigo reprovou** com `left: Running`.
- Unitários: `parse_since_base` (pares status/caminho, não rastreados, entrada vazia) e `create_repo_workspace` (dois diretórios distintos, 0700). **Mutante** com caminho fixo: reprova. Restaurado.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.192 passaram, 0 falharam, 18 ignorados** (+1 teste Docker). Job SQLite: verde.

**Não coberto.** O modo `--repo` do v1 não foi exercitado de ponta a ponta, porque exigiria clonar um repositório real do GitHub dentro do Docker, com rede e conta. Só a criação do diretório foi testada.

## 4. Contrato de eventos do dashboard — ✅ (parcial)

**Defeito (ARQ-03), medido.** O subscriber do dashboard do v1 serializava `tokens_in`, `tokens_out` e `error` como `null`. O schema da API (`StepSchema`, Zod 4 `.optional()`) **recusa** `null`. Medido com um teste de contrato contra o schema real, ele devolveu `expected number, received null` para `steps.0.tokens_in`/`tokens_out` e `expected string, received null` para `steps.0.error`. Isso vale para todo passo `cmd`, que não tem tokens, e para todo passo bem-sucedido, que não tem erro. Ou seja, a ingestão do run inteiro era rejeitada com 400, e o dashboard ficava sem os runs do v1 (o motor de todo despacho e do Slack). Os dois lados tinham o contrato digitado à mão.

**Correção.**
- **Rust** (`src/events/subscribers.rs`): os campos `Option` de `TrackedStep` são omitidos quando ausentes (`skip_serializing_if`), e `started_at`, exigido pela API, nunca sai nulo: usa o horário de término como fallback.
- **API** (`routes/events.ts`): os campos opcionais do evento e dos passos aceitam `null` (`.nullish()`), para os CLIs antigos já instalados.
- **Contrato compartilhado:** `tests/fixtures/dashboard-event.json` é o payload que o Rust **de fato** serializa para um run com um passo `cmd` sem tokens e um passo `agent` que falhou.
  - O teste golden do Rust (`dashboard_payload_matches_the_shared_contract_fixture`) exige igualdade com o fixture (regenera com `UPDATE_FIXTURES=1`) e ausência de `null`.
  - O teste da API (`events-contract.test.ts`) valida o mesmo arquivo contra o `EventSchema`.
  - Se um dos lados derivar, um dos testes reprova.

**Medido:**
- O teste de contrato da API **reprovou no código antigo**, e o teste com `null` reprovou com os três erros acima. No novo, os dois passam.
- **Mutante** no Rust (`tokens_in` volta a sair como `null`): o teste golden reprova. Restaurado.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.194 passaram, 0 falharam, 18 ignorados**. Job SQLite: verde. Job TypeScript: verde (**24 testes**).

**Não coberto: BUG-CLI-08.** O v1 emite `WorkflowCompleted` **sempre**, até quando o run falha (por exemplo num gate), e o núcleo não tem evento `WorkflowFailed`. O status enviado ao dashboard sai dos passos ("failed" só se algum passo falhou), então um run reprovado por gate aparece como `success`. Corrigir exige uma variante nova no contrato de eventos do núcleo (`stepyard-core`), que atravessa os dois motores, e fica registrado. O POST "dispara e esquece", com espera fixa de 500 ms, também continua (BUG-CLI-14).

## 5. Pânicos por fatiamento de UTF-8 — ✅ (parcial)

**Defeitos (BUG-CLI-06, BUG-MOTOR-14, BUG-CLI-10).**
- O bot do Slack cortava a saída do workflow com `&stdout[stdout.len() - 1500..]` e `&stderr[stderr.len() - 1000..]`. Quando esse índice caía no meio de um caractere de vários bytes (saída acentuada ou com emoji, comum em português), o processo entrava em pânico e o resultado nunca era postado.
- `stepyard remote status` fazia o mesmo no início do texto, com `&target[..18]` e `&workflow[..22]`.

**Correção** (`src/display.rs`): `tail_at_char_boundary`, nova, e `truncate_at_char_boundary`, que já existia e virou `pub(crate)`, cortam sempre numa fronteira de caractere e ficam dentro do limite de bytes. Os três pontos passaram a usá-las.

**Teste** (`display.rs`): para **todo** limite de 0 ao tamanho de um texto com `ação 🚀 çã` repetido, os dois cortes ficam dentro do orçamento, não partem caractere e são prefixo ou sufixo do original. O mesmo teste traz o **controle**: a expressão antiga do Slack, aplicada a `é` seguido de 1.499 `a`, entra em pânico (`catch_unwind`), e a nova devolve 1.499 bytes.

**Medido:**
- Os testes do Slack (`--features slack`) passam, e o `clippy` com essa feature sai com status 0.
- O clippy padrão barrou a primeira versão: a função nova só é usada pelo bot, então vira código morto sem a feature. Ficou anotada com `cfg_attr`.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.196 passaram, 0 falharam, 18 ignorados**. Job SQLite: verde.

**Não coberto: BUG-CLI-09.** O Slack continua sem deduplicação de retries (o Slack reenvia o mesmo evento se não receber resposta em 3 s) e sem limite de tarefas simultâneas. Fica para um próximo item.

## 6. Upload de artefatos acima de 100 KB — ✅ (parcial)

**Defeito (SEG-TS-06).** O parser JSON global da API tinha limite de 100 KB, e o upload de artefato manda o arquivo em base64 no corpo JSON. Todo artefato acima de cerca de 75 KB falhava com 413, embora o limite documentado (`STEPYARD_ARTIFACT_MAX_BYTES`) seja 10 MiB.

**Correção** (`packages/api/src/body.ts`, novo, usado em `index.ts`): `jsonBodyParser()` mantém 100 KB em todas as rotas, menos em `POST /api/workflows/:id/artifacts`. Essa rota ganha um limite dimensionado para a forma base64 do artefato máximo (4/3 do tamanho mais 64 KB), e a **autenticação roda antes** de o corpo grande ser lido, para que um cliente sem credencial não faça a API armazenar megabytes. `artifactMaxBytes` saiu de `routes/workflows.ts` para esse módulo, e os dois usam a mesma função.

**Testes** (`body.test.ts`, servidor HTTP real):
- artefato autenticado de 300 KB aceito; no código antigo dava 413;
- o mesmo upload sem credencial → 401, sem ler o corpo;
- outra rota com 200 KB → 413, ou seja, o limite de 100 KB continua valendo;
- artefato acima do limite configurado → 413.

**Medido:**
- **Mutantes:** limite único de 100 KB (o código antigo) reprova o teste de 300 KB e o do 401. Corpo grande lido sem autenticar antes reprova o teste do 401. Restaurado.
- Job TypeScript do CI: build da API, build do web e testes, todos com status 0 (**28 testes**). Nada no Rust mudou.

**Não coberto.**
- **SEG-TS-08** (cookie `Secure` decidido por `req.hostname`): atrás do proxy do Vite, o host visto pela API é `api`, então o cookie sai `Secure` mesmo no deploy HTTP documentado, e o navegador o descarta. A correção depende de como o proxy de produção repassa host e protocolo (`trust proxy`, `X-Forwarded-*`), e trocar a regra sem esse deploy à mão poderia afrouxar o `Secure` em produção. Fica para verificação com o deploy real.
- **SEG-TS-05** (um único segredo para telemetria, despacho e login; sem rate limit): é desenho de credenciais e precisa de decisão sua.

## 7. `stepyard validate` valida para o motor padrão — ✅

**Defeito (BUG-CLI-13 / MANUT-06).** O `validate` só rodava o validador do v1 e aprovava workflows que o v2, o motor padrão do `execute`, recusava ao rodar. Exemplo: um `cmd` com `config.timeout: 30` (número sem unidade) passava no `validate` e falhava no `execute`.

**Correção** (`src/cli/commands.rs`): o `validate` ganhou `--engine v1|v2`, com padrão `v2`, como o `execute`. Em `v2`, depois do validador do v1, ele aplica os defaults, roda o adaptador v1→v2 e a validação do harness: as mesmas checagens que o `execute` faz antes de rodar qualquer passo. Em caso de erro, informa a falha do v2 e que o arquivo passa em `--engine v1`. README e `docs/YAML-SPEC.md` foram atualizados.

**Testes** (`tests/cli_validate.rs`, binário real, sem banco nem Docker):
- o caso `timeout: 30` reprova por padrão, citando `--engine v2` e `timeout`;
- com `--engine v1`, passa;
- os 12 workflows embutidos passam no `validate` padrão.

**Medido:**
- **Mutante** (checagem do v2 desligada): o primeiro teste reprova. Restaurado.
- Portões do CI: todos com status 0. Suíte completa com banco: **1.199 passaram, 0 falharam, 18 ignorados**. Job SQLite: verde.

**Não coberto.** A correção de `ARCHITECTURE.md`, `YAML-SPEC.md` e `STEP-TYPES.md` onde prometem o que o v2 não faz (ARQ-05, MANUT-03) não entrou neste item.

## 8. Integridade dos testes — ✅ (parcial)

**TEST-04 — dois testes que nunca rodavam localmente.** `signal_handler` (SIGTERM → saída 143) e `session_list_cli` procuravam o binário em `../../target`, mas o `.cargo/config.toml` manda o build para `/tmp/stepyard-target`. **Medido** com `--nocapture` e Postgres real: os dois imprimiam `[skip] workspace stepyard binary not built` e **contavam como aprovados**, em 0,00 s. No CI eles rodavam, porque lá `CARGO_TARGET_DIR=target`.
- **Correção:** o binário é procurado a partir do próprio executável do teste (`<target>/<perfil>/deps/<teste>` → `<target>/<perfil>/stepyard`), o que vale para qualquer diretório de build, com os caminhos antigos como alternativa. Depois disso os dois **executam de verdade** (6 s e 2,4 s), e o de SIGTERM mede saída 143 em 0,42 s.

**TEST-01 — o mock não distinguia `destroy` de `destroy_by_session`.** No Docker real, `destroy(id)` não faz nada; só `destroy_by_session` remove o contêiner. O `MockLifecycle` usava a implementação padrão, que transforma uma chamada na outra, e registrava `Destroy` nos dois casos. Um engine que regredisse para `destroy()` passaria em todos os testes.
- **Correção:** o mock registra `MockCall::DestroyBySession { session_id }` e depois delega a `destroy`, preservando o contrato padrão que o teste do `lifecycle.rs` verifica. Os quatro testes do harness que conferem o teardown (`signal_cancel`, `step_timeout`, `cancel_cleanup` e `timeout_zero`) passaram a exigir `DestroyBySession` com o UUID da sessão.
- **Mutante:** o engine chamando `destroy()` no cancelamento faz o `cancel_cleanup` **reprovar**. Com o mock antigo, essa regressão passava.

**Efeito colateral encontrado e corrigido.** Com os dois testes do TEST-04 rodando, a suíte completa reprovou uma vez em `startup_reconcile`: a segunda reconciliação, que devia ser idempotente, reconciliou 2 sessões.
- **Causa:** o `session_list_cli` deixava sessões `running` no banco compartilhado. Desde o lease da P1-1 elas viram candidatas ao completar 60 s, e na ordem da suíte duas cruzaram esse limite entre as duas chamadas do teste.
- **Correção:**
  - a idempotência passou a ser verificada **por objeto** (a sessão do teste continua `failed` com um único `crash_recovery`; o contêiner do teste foi removido), em vez de contadores do banco inteiro;
  - o `session_list_cli` encerra as sessões que cria.
- **Medido:** duas rodadas completas seguidas, verdes.

**Medido (item):** portões do CI todos com status 0. Suíte completa com banco: **1.199 passaram, 0 falharam, 18 ignorados**, em duas rodadas. Job SQLite: verde.

**Não coberto (dependem de `.github/`, fora do escopo):**
- **TEST-03:** os ~94 testes de banco ainda "passam" sem `DATABASE_URL`. Proposta: uma variável `STEPYARD_REQUIRE_DB=1` que transforme o `[skip]` em falha, ligada no CI.
- **TEST-02:** job com Docker para os testes `#[ignore]`.
- **MANUT-05:** compilar e testar `--features slack` no CI.
- **TEST-13:** atalho "docs-only" do `check.yml`.

### 8b. A suíte de testes registrava worktrees no repositório do desenvolvedor — ✅

**Achado novo, encontrado durante o item 8.** O v2 prepara uma **git worktree por sessão** em `<cwd>/.stepyard/workspaces`, e a limpeza só remove as que têm mais de 24 h. Os testes de CLI que rodam `stepyard execute` a partir da raiz do repositório registravam essas worktrees no `.git` do desenvolvedor, que é o mesmo `.git` compartilhado com o checkout principal. **Medido** por binário de teste, com Postgres: `cli_execute` +2 (já existia), `cli_v2_step_config` +4 e `cli_concurrent_runs` +3 (escritos nesta sessão) e, antes da sua correção, `signal_handler` +1 por execução. As rodadas desta sessão acumularam **230** worktrees `stepyard-session-*` registradas (havia mais 2 no checkout principal, anteriores e não criadas aqui).

**Correção.**
- `tests/support/mod.rs` (novo): `temp_git_repo()` e `repo_path()`.
- `cli_execute`, `cli_v2_step_config` e `cli_concurrent_runs` rodam `execute` num repositório git temporário, com o workflow por caminho absoluto. Mesma correção no `signal_handler` (commit `8c28951`).

**Medido:** depois da correção, cada um desses binários registra **+0** worktrees e continua passando. A suíte completa inteira registra **+0** (antes, de 9 a 10 por rodada). Portões do CI: todos com status 0. Suíte: **1.199 passaram, 0 falharam, 18 ignorados**. Job SQLite: verde.

**Pendente, decisão do Allan:** limpar as 230 worktrees já registradas por esta sessão (`analise/.stepyard/workspaces/stepyard-session-*` e `analise/crates/stepyard-harness/.stepyard/workspaces/stepyard-session-*`). Não as removi porque o registro de cada uma vive no `.git` do checkout principal, fora desta pasta. Comando sugerido, revisando a lista antes:

    git worktree list --porcelain | awk '/^worktree .*\/orca\/workspaces\/stepyard\/analise\/.*stepyard-session-/{print $2}'   # revisar
    # depois, para cada caminho listado: git worktree remove --force <caminho>; e por fim: git worktree prune
