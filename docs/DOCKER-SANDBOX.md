# Docker Sandbox

Stepyard runs workflows inside a Docker container by default. Each step
(or only agent steps, depending on the mode) executes in an isolated environment
with resource limits. Network `allow` and `deny` lists are stored and are
not enforced; see the configuration section.

## Requirements

- Docker Desktop **4.40+** (or Docker Engine with equivalent features)
- Docker daemon reachable from the host (the CLI shells out to `docker run`)

If the daemon isn't reachable, `stepyard execute` exits with a clear message and
no partial state. Use `--no-sandbox` to run locally on the host instead.

## CLI flags

```bash
stepyard execute workflow.yaml -- <target>               # sandboxed by default (v2 engine)
stepyard execute --no-sandbox workflow.yaml -- <target>  # run on host
stepyard execute --engine v1 --repo OWNER/REPO workflow.yaml -- <target>  # clone repo inside the container (v1 only)
stepyard execute --sandbox-runtime podman workflow.yaml -- <target>  # use Podman instead of Docker
stepyard execute --engine v1 --no-sandbox workflow.yaml -- <target>  # legacy engine (deprecated) on the host
```

The default engine is **v2** (`stepyard-harness` + `stepyard-sandbox-orchestrator`).
Pass `--engine v1` only when you need the legacy monolithic engine.

## Sandbox modes

Resolved in [`sandbox/mod.rs::resolve_mode`](../src/sandbox/mod.rs) from CLI
flags + `config.global.sandbox.*`:

| Mode            | Trigger                                        | Behavior                                                        |
|-----------------|------------------------------------------------|-----------------------------------------------------------------|
| **Disabled**    | `--no-sandbox` (or `sandbox: false` in config) | All steps run on the host                                       |
| **AgentOnly**   | `config.agent.sandbox: true`                   | Only `agent`/`chat` steps run in the container                  |
| **Full**        | default                                        | Every step runs inside the container                            |

## Configuration

```yaml
config:
  global:
    sandbox:
      enabled: true
      image: "stepyard-sandbox:latest"   # optional — defaults to the built-in image
      workspace: "/workspace"              # mount point inside the container
      network:
        allow: ["api.anthropic.com", "api.github.com"]
        deny:  ["169.254.169.254"]         # recorded only; not enforced yet
      resources:
        cpus: 2
        memory: "2g"
      exclude:                             # paths NOT copied into the container
        - "node_modules"
        - "target"
        - ".git"
```

See [`sandbox/config.rs`](../src/sandbox/config.rs) for the full option set and
defaults. `from_global_config` parses the `config.global.sandbox` block.

A non-empty `allow` or `deny` list only adds `--network bridge` when the
container is created. It does not block `169.254.169.254` or any other
destination. Do not treat the list as an egress filter.

## Workflow-declared volumes

`config.global.sandbox.volumes` in a workflow (or in the project's
`.stepyard/config.yaml`) is passed to `docker run -v`. Both engines refuse an
entry whose host source resolves **outside the project directory** — `/`,
`$HOME`, `~/.ssh`, `/var/run/docker.sock`, `../elsewhere`, or a symlink inside
the project that points outside — before any container starts. Named volumes
(`cache:/cache`), anonymous volumes (`/data`) and paths inside the project are
accepted. If you trust a workflow that needs an outside mount, set
`STEPYARD_ALLOW_WORKFLOW_MOUNTS=1` in the environment of `stepyard`; a workflow
file cannot set it.

## Workspace modes

- **CWD copy (default):** the host's current working directory is copied into
  the container's `workspace` path, and files that changed are copied back to
  the host when the workflow ends (v2). On v1 this mode is **refused**: v1
  bind-mounts the working directory read-write and runs `chown -R` on it, so
  the container would write and re-own your real files. Use the default v2
  engine, `--engine v1 --repo OWNER/REPO`, or `--no-sandbox`.
- **Repo clone (`--repo OWNER/REPO`):** the container clones the given repo at
  start, using `GH_TOKEN`/`GITHUB_TOKEN` (or `gh auth token` fallback). The
  host CWD is not copied. Useful for CI-style isolation.

`exclude` patterns prevent the CWD copy from sweeping in large directories like
`node_modules` or `target`.

## Secure API proxy

When a workflow runs inside a Docker/Podman sandbox and `ANTHROPIC_API_KEY` is
set on the host, the real key **stays on the host**. The engine starts a small
proxy ([`sandbox/proxy.rs`](../src/sandbox/proxy.rs) on v1;
v2 wires the same proxy from `execute_v2` into sandbox cmd steps via
`stepyard-harness`) that the container reaches by URL.

**What the container sees:**

| Variable | Value |
|----------|-------|
| `ANTHROPIC_BASE_URL` | `http://host.docker.internal:<port>` |
| `ANTHROPIC_API_KEY` | Per-session **dummy token** (not the real key) |

**What the proxy does:**

1. Validates the dummy token on each request (`x-api-key` header)
2. Forwards only `/v1/*` paths to `api.anthropic.com`
3. Injects the real `ANTHROPIC_API_KEY` on the upstream request

If the proxy fails to start, the workflow **aborts** — there is no fallback that
injects the real key into the container.

**v2 note:** with a container runtime, `cmd` and `agent` steps run in the
container and get the dummy token, never the real key. `agent` steps run as
`docker|podman exec -i --user stepyard -w /workspace -e KEY ...`: env values are
read by the exec client from its own environment, so they do not appear in the
process list. `chat` steps run on the host.

Architecture diagram: see the "Sandbox and API key protection" section of the README.

## Lifecycle

**v2 engine (default):**

1. `stepyard execute` starts; API proxy starts if sandbox + `ANTHROPIC_API_KEY`
2. Before the first cmd/agent step, the engine creates (or reuses) the session
   container and copies the project into `/workspace` with a `tar` stream
   (excluding `target`, `node_modules`, `.stepyard`, `.venv`, `__pycache__`).
   If `/workspace` has no `.git`, a baseline commit is made there. A marker
   file (`/tmp/.stepyard-workspace-ready`) prevents a second copy when a run
   resumes with the same container.
3. Cmd steps run via `docker exec` in `/workspace`; agent steps run the Claude
   CLI in the container as `stepyard`; chat steps run on the host
4. At the end (success or step failure), files reported by `git status` in
   `/workspace` are copied back to the project (`docker cp`; deletions are
   applied; paths with `..` or absolute paths are rejected), then
   `destroy_by_session` removes the container. A cancel removes the container
   without copying back.

The default image must be available for Podman: build it with
`podman build -t stepyard-sandbox:latest -f Dockerfile.sandbox .`.

**v1 engine (`--engine v1`):**

1. `stepyard execute` starts; API proxy starts if sandbox + `ANTHROPIC_API_KEY`
2. `sandbox_up()` creates the container
3. Steps execute (dispatched to host or container per mode)
4. `sandbox_down()` destroys the container
5. Cleanup runs even if a step fails

A container left behind by a crash can be removed with `docker rm -f` using
the name printed at startup.

## Typical failure modes

| Symptom                                            | Cause / Fix                                                 |
|----------------------------------------------------|-------------------------------------------------------------|
| `docker daemon not reachable`                      | Start Docker Desktop; or pass `--no-sandbox`                |
| `failed to clone <repo>`                           | `GH_TOKEN` missing or `gh auth status` stale                |
| `image pull failed`                                | Authenticate to the registry or change `image`              |
| Steps hang at network calls                        | `network.allow` is not an egress filter; check DNS and the daemon |

## See also

- [`README.md`](../README.md) — CLI flags, security model, dashboard setup
- [`REMOTE.md`](REMOTE.md) — remote dispatch and VPS deployment
- `YAML-SPEC.md` — workflow schema
- `CONFIG.md` — how `config.global.sandbox` layers with step-inline overrides
