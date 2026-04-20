# Remote Execution

Dispatch workflows to a Stepyard engine running on a VPS, from any machine. No SSH
in your shell history; no shell-wrangling for each run. The model follows
ARCHITECTURE.md Invariant 10 ("um engine por VPS") — your local CLI is a thin
client; everything executes remotely inside `minion-sandbox:latest` containers
on the VPS.

Introduced by **Epic 5 — Remote-First Execution**:
- Story 5.1 — `POST /api/workflows/dispatch` on the Dashboard API
- Story 5.2 — `stepyard remote` subcommand on the local CLI

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
# default_repo = "allanbrunobr/stepyard"   # optional — used when --repo omitted
```

Then:

```bash
stepyard remote exec fix-issue --repo allanbrunobr/test-project -- 42
stepyard remote exec code-review --branch feature/xyz -- PR-123
stepyard remote exec my-workflow --var foo=1 --var bar=two -- target-value

stepyard remote status --limit 5
stepyard remote status --workflow fix-issue

stepyard remote logs <run_id>    # stub until Story 5.3
```

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

- **5.3** — log streaming via SSE from the host to the local CLI
- **5.4** — warm sandbox pool on the VPS to eliminate 60-90s startup
- **5.5** — upload arbitrary artifacts from the container back to the dashboard
