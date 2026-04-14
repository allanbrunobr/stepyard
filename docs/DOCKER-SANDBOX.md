# Docker Sandbox

Minion Engine runs workflows inside a Docker container by default. Each step
(or only agent steps, depending on the mode) executes in an isolated environment
with bounded network and resource access.

## Requirements

- Docker Desktop **4.40+** (or Docker Engine with equivalent features)
- Docker daemon reachable from the host (the CLI shells out to `docker run`)

If the daemon isn't reachable, `minion execute` exits with a clear message and
no partial state. Use `--no-sandbox` to run locally on the host instead.

## CLI flags

```bash
minion execute workflow.yaml -- <target>               # sandboxed by default
minion execute --no-sandbox workflow.yaml -- <target>  # run on host
minion execute --repo OWNER/REPO workflow.yaml -- <target>  # clone repo inside the container
```

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
      image: "minion-engine/base:latest"   # optional — defaults to the built-in image
      workspace: "/workspace"              # mount point inside the container
      network:
        allow: ["api.anthropic.com", "api.github.com"]
        deny:  ["169.254.169.254"]         # block cloud metadata endpoints
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

## Workspace modes

- **CWD copy (default):** the host's current working directory is copied into
  the container's `workspace` path. Changes inside the container stay there.
- **Repo clone (`--repo OWNER/REPO`):** the container clones the given repo at
  start, using `GH_TOKEN`/`GITHUB_TOKEN` (or `gh auth token` fallback). The
  host CWD is not copied. Useful for CI-style isolation.

`exclude` patterns prevent the CWD copy from sweeping in large directories like
`node_modules` or `target`.

## Secure API proxy

When running `chat` steps in a sandbox, secrets (`ANTHROPIC_API_KEY`) stay on
the host. The engine starts a small proxy ([`sandbox/proxy.rs`](../src/sandbox/proxy.rs))
that the container reaches by URL only — the key is never copied into the
container. This lets you run third-party code in the sandbox without exposing
your API key.

Architecture diagram: `docs/architecture-api-proxy.jpg`.

## Lifecycle

1. `minion execute` starts
2. If not `Disabled`, `sandbox_up()` creates the container
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
| Steps hang at network calls                        | Domain not in `network.allow`                               |

## See also

- `YAML-SPEC.md` — workflow schema
- `CONFIG.md` — how `config.global.sandbox` layers with step-inline overrides
- Architecture image: `docs/architecture-docker-sandbox.jpg`
