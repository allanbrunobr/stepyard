# Remote Execution

Dispatch workflows to a Stepyard engine running on a VPS, from any machine. No SSH
in your shell history; no shell-wrangling for each run. The model follows
ARCHITECTURE.md Invariant 10 ("um engine por VPS") — your local CLI is a thin
client; everything executes remotely inside `minion-sandbox:latest` containers
on the VPS.

Introduced by **Epic 5 — Remote-First Execution**:
- Story 5.1 — `POST /api/workflows/dispatch` on the Dashboard API
- Story 5.2 — `stepyard remote` subcommand on the local CLI
- Story 5.3 — `GET /api/workflows/:run_id/logs/stream` SSE snapshots consumed
  by `stepyard remote logs`

## Architecture

```
┌─ Local (Mac) ───────────────┐              ┌─ VPS ─────────────────────────────┐
│                             │              │                                   │
│  ~/.stepyard/remote.toml      │              │  Dashboard (docker-compose)       │
│    url    = http://VPS:3001 │              │  ├── api (Node.js + ssh-client)   │
│    secret = $API_SECRET     │              │  │    POST /api/workflows/        │
│                             │              │  │      dispatch                  │
│  stepyard remote exec \       │   HTTP POST  │  │    └─► ssh host.docker.internal│
│    fix-issue --repo X/Y \   │ ───────────► │  │         "stepyard execute ..."   │
│    -- 42                    │   (Bearer    │  │                                 │
│                             │    auth)     │  └── host ────────────────────────│
│  stepyard remote status       │ ◄─────────── │      └─► stepyard 0.7.6 runs        │
│    (shows run list)         │              │          └─► minion-sandbox ctr   │
│                             │              │              └─► git clone, run  │
│                             │              │                  push back, emit │
│                             │              │                  event to api    │
└─────────────────────────────┘              └───────────────────────────────────┘
```

## Local CLI setup

Create `~/.stepyard/remote.toml`:

```toml
url = "http://187.45.254.82:3001"
secret = "<API_SECRET from dashboard .env on the VPS>"
default_repo = "allanbrunobr/stepyard"   # used when --repo omitted
```

Then:

```bash
stepyard remote exec fix-issue --repo allanbrunobr/test-project -- 42
stepyard remote exec code-review --branch feature/xyz -- PR-123
stepyard remote exec my-workflow --var foo=1 --var bar=two -- target-value

stepyard remote status --limit 5
stepyard remote status --workflow fix-issue

stepyard remote logs <run_id>
```

Remote dispatch is repo-mode by default: every run must provide `--repo` or
`default_repo`. The API rejects repo-less dispatches because that path depends
on a host workspace mounted/copied into a per-run sandbox, which is not a stable
remote contract. Operators can temporarily restore the old behavior with
`STEPYARD_DISPATCH_ALLOW_LOCAL_WORKSPACE=true`, but that is an explicit
compatibility escape hatch, not the supported remote mode.

## VPS deployment (Dashboard API)

The dispatch endpoint spawns `stepyard execute` on the **host**, not inside the
API container. The container uses SSH with a mounted key. Setup:

### 1. Generate (or reuse) an SSH key for the dispatcher

```bash
ssh root@allanbruno.vps-kinghost.net \
  'test -f /root/.ssh/id_ed25519 || ssh-keygen -t ed25519 -N "" -f /root/.ssh/id_ed25519'

# Make root able to SSH into itself (host.docker.internal resolves to the host)
ssh root@allanbruno.vps-kinghost.net \
  'grep -qxf /root/.ssh/id_ed25519.pub /root/.ssh/authorized_keys \
    || cat /root/.ssh/id_ed25519.pub >> /root/.ssh/authorized_keys'
```

### 2. Configure `.env` on the VPS

Edit `/root/stepyard-dashboard/.env`:

```env
# Existing
API_SECRET=<unchanged>
POSTGRES_USER=stepyard
POSTGRES_PASSWORD=<unchanged>
POSTGRES_DB=minion_engine
# …

# NEW — Story 5.1
STEPYARD_DISPATCH_SSH_HOST=root@host.docker.internal
STEPYARD_WORKFLOWS_DIR=/root/.stepyard/workflows
ANTHROPIC_API_KEY=<your key>         # forwarded by ssh when dispatching
GH_TOKEN=<gh token with repo scope>  # forwarded; needed for --repo mode
HOST_SSH_DIR=/root/.ssh              # mount source for api container
```

### 3. Rebuild the api container

```bash
ssh root@allanbruno.vps-kinghost.net \
  'cd /root/stepyard-dashboard && docker compose up -d --build api'
```

### 4. Smoke test

Local:

```bash
stepyard remote exec hello-world -- smoke
stepyard remote status --limit 1
```

You should see the run in the dashboard at `http://<vps>:5173/workflows` and a
new `minion-sandbox:latest` container appear briefly in Portainer.

## Remote logs

`stepyard remote logs <run_id>` opens an authenticated SSE stream to
`/api/workflows/:run_id/logs/stream`. The dispatch API pre-generates the
dashboard `run_id`, records a placeholder row, and links that row to the
detached process log. The stream emits both sanitized process-log chunks and
dashboard snapshots: run status plus the current step rows.

## Remote artifacts

The Dashboard API accepts authenticated per-run artifacts through
`POST /api/workflows/:run_id/artifacts`. The first implementation is an API
contract: upload JSON with a plain filename and base64 content, then list or
download the artifact from the same run.

```bash
CONTENT=$(base64 -i report.zip | tr -d '\n')
curl -H "Authorization: Bearer $API_SECRET" \
  -H "Content-Type: application/json" \
  -d "{\"name\":\"report.zip\",\"content_base64\":\"$CONTENT\",\"content_type\":\"application/zip\"}" \
  "$API_URL/api/workflows/$RUN_ID/artifacts"

curl -H "Authorization: Bearer $API_SECRET" \
  "$API_URL/api/workflows/$RUN_ID/artifacts"

curl -L -H "Authorization: Bearer $API_SECRET" \
  "$API_URL/api/workflows/$RUN_ID/artifacts/$ARTIFACT_ID" \
  -o report.zip
```

Uploads require the same Bearer secret as dispatch. Artifact listing and
download follow the dashboard's current read-endpoint model: access is
controlled by the dashboard/network boundary, not by a browser-visible token.
Artifacts are stored on the API host under `STEPYARD_ARTIFACT_DIR`, defaulting
to `/tmp/stepyard-artifacts`. The server never uses the submitted filename as a
filesystem path; it stores bytes under a generated artifact id and keeps the
original name only as metadata/download presentation. Uploads are capped by
`STEPYARD_ARTIFACT_MAX_BYTES`, defaulting to 10 MiB.

Workflow YAML can also ask the dashboard subscriber to upload files after the
run reaches `WorkflowCompleted`:

```yaml
config:
  events:
    dashboard:
      url: http://host.docker.internal:3001/api/events
      secret: ${DASHBOARD_API_SECRET}
      artifacts:
        - reports/report.zip
        - coverage/lcov.info
```

Artifact paths are resolved by the engine process after sandbox copy-back. In
remote repo-mode runs, use paths inside the checked-out workspace that will
exist on the host after the sandbox lifecycle copies changed files back.

## Security notes

- **API secret**: `API_SECRET` in `.env` is the Bearer token for all dispatch
  calls. Use a long random string. Never commit the `.env`.
- **SSH key scope**: the dispatcher key is root-on-VPS. It can do anything root
  can do. Consider a dedicated `stepyard` user with only the permissions to run
  `stepyard execute` — a post-MVP hardening.
- **GH_TOKEN**: stored in `.env` on the VPS and forwarded over SSH env. Needs
  at minimum `repo` scope for `--repo` mode clones. Rotate regularly.
- **ANTHROPIC_API_KEY**: same story. The engine's secure API proxy keeps it on
  the host during agent steps (never injected into the sandbox container).
- **Network exposure**: the dispatch endpoint runs on port 3001. If your VPS
  exposes this publicly, put it behind HTTPS (e.g. Caddy/nginx reverse proxy).
  Without TLS, `API_SECRET` is sent in the clear.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `401 Unauthorized` from `stepyard remote` | `secret` in `~/.stepyard/remote.toml` doesn't match `API_SECRET` on the VPS |
| `404 WORKFLOW_NOT_FOUND` | Workflow basename doesn't have a matching YAML in `STEPYARD_WORKFLOWS_DIR` |
| API logs `Permission denied (publickey)` | Host's `authorized_keys` missing the dispatcher pubkey |
| Sandbox container can't reach `:3001` for events | In a workflow YAML, set `config.events.dashboard.url` to `http://host.docker.internal:3001/api/events` (on Linux with Docker 20.10+) |
| `git push` fails inside container | `GH_TOKEN` not forwarded — check `STEPYARD_SSH_ENV_FORWARD` in `.env` |

## What's next (deferred to later stories)

- **5.4 follow-up** — true warm sandbox pool. This requires a worker/queue,
  container leases, workspace reset semantics, and secret-isolation rules; it is
  not safe to bolt onto the current detached `POST /dispatch` flow.
- **5.5 follow-up** — authenticated dashboard sessions. Artifact reads follow
  the existing dashboard read model until the dashboard grows login/session
  auth.
