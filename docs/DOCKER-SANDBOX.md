# Minion Engine — Docker Sandbox

Running `cmd` and `agent` steps inside a Docker container provides isolation: untrusted commands cannot affect the host system.

---

## Quick Start

```bash
# 1. Build the reference sandbox image (one-time)
docker build -f Dockerfile.sandbox -t minion-sandbox:latest .

# 2. Run any workflow with --sandbox
minion execute workflows/code-review.yaml -- 142 --sandbox --verbose
```

That's it. The engine will:
1. Create a container from `minion-sandbox:latest`
2. Auto-forward `ANTHROPIC_API_KEY`, `GH_TOKEN` and other credentials
3. Mount `~/.config/gh`, `~/.claude`, `~/.ssh`, `~/.gitconfig` read-only
4. Copy your workspace (excluding `node_modules`, `target`, etc.)
5. Execute all steps inside the container
6. Copy results back and destroy the container

---

## Sandbox Configuration

### Minimal (zero-config)

```yaml
name: my-workflow
config:
  global:
    sandbox:
      enabled: true
```

With zero config, the engine uses **smart defaults**:
- **Image**: `minion-sandbox:latest`
- **Env vars auto-forwarded**: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GH_TOKEN`, `GITHUB_TOKEN`
- **Volumes auto-mounted (ro)**: `~/.config/gh`, `~/.claude`, `~/.ssh`, `~/.gitconfig`
- **HOME**: always set to `/root`

### Full Configuration

```yaml
config:
  global:
    sandbox:
      enabled: true
      image: "minion-sandbox:latest"

      # Environment variables to forward from host → container.
      # When omitted, auto-forwards: ANTHROPIC_API_KEY, OPENAI_API_KEY, GH_TOKEN, GITHUB_TOKEN
      env:
        - ANTHROPIC_API_KEY
        - GH_TOKEN
        - NPM_TOKEN
        - CUSTOM_API_KEY

      # Extra read-only volume mounts (host:container:mode).
      # Tilde (~) is expanded to $HOME. Non-existent paths are skipped.
      # When omitted, auto-mounts: ~/.config/gh, ~/.claude, ~/.ssh, ~/.gitconfig
      volumes:
        - "~/.config/gh:/root/.config/gh:ro"
        - "~/.claude:/root/.claude:ro"
        - "~/.ssh:/root/.ssh:ro"
        - "~/.gitconfig:/root/.gitconfig:ro"
        - "~/.npmrc:/root/.npmrc:ro"

      # Patterns to exclude from workspace copy (saves time on large projects)
      exclude:
        - node_modules
        - target
        - .git/objects
        - dist
        - build
        - __pycache__

      # DNS servers (useful when default bridge DNS is unreliable)
      dns:
        - "8.8.8.8"
        - "1.1.1.1"

      # Network allow/deny lists
      network:
        allow:
          - "api.anthropic.com"
          - "api.github.com"
          - "registry.npmjs.org"
        deny: []

      # Resource limits
      resources:
        cpus: 2.0
        memory: "4g"
```

---

## Three Sandbox Modes

| Mode | Trigger | What runs in Docker |
|------|---------|---------------------|
| **Full Workflow** | `--sandbox` CLI flag | All steps |
| **Agent Only** | `config.agent.sandbox: true` | Only `agent` steps |
| **Devbox** | `config.global.sandbox.enabled: true` | All steps (with full config) |

---

## Building the Sandbox Image

A reference `Dockerfile.sandbox` is included with the project:

```bash
docker build -f Dockerfile.sandbox -t minion-sandbox:latest .
```

This image includes: `git`, `gh`, `jq`, `curl`, `node`, `npm`, `cargo`, `clippy`, `python3`, and `claude` CLI.

### Custom Image

For project-specific needs, extend the reference image:

```dockerfile
FROM minion-sandbox:latest

# Add project-specific tools
RUN pip3 install pytest black mypy
RUN npm install -g eslint prettier

# Pre-install project dependencies for faster runs
COPY package.json package-lock.json /workspace/
RUN cd /workspace && npm ci
```

---

## How Credentials Work

### Automatic (zero-config)

When you don't specify `env:` or `volumes:`, the engine automatically:

1. **Forwards env vars** (if set on host): `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GH_TOKEN`, `GITHUB_TOKEN`
2. **Mounts directories** (if they exist): `~/.config/gh`, `~/.claude`, `~/.ssh`, `~/.gitconfig`

This means `gh`, `claude`, `git push`, and API calls work out-of-the-box.

### Explicit Override

If you specify `env:` or `volumes:` in the config, the auto-lists are **replaced** (not merged). This lets you lock down exactly what enters the container:

```yaml
sandbox:
  env:
    - ANTHROPIC_API_KEY    # only this, no GH_TOKEN
  volumes:
    - "~/.claude:/root/.claude:ro"  # only this, no ~/.ssh
```

---

## Workspace Copy & Exclusions

By default, the entire workspace is copied with `docker cp`. For large projects, configure `exclude` to skip heavy directories:

```yaml
sandbox:
  exclude:
    - node_modules    # can be gigabytes
    - target          # Rust build artifacts
    - .git/objects    # git object store (history)
    - dist
    - build
```

The engine uses `tar --exclude` piped into the container, so the excluded files never leave the host.

> **Tip**: Dependencies like `node_modules` should be pre-installed in the Docker image or installed via a `cmd` step (`npm ci`) inside the container.

---

## Network Isolation

```yaml
sandbox:
  network:
    allow:
      - "api.anthropic.com"     # Claude API
      - "api.github.com"        # GitHub API
      - "registry.npmjs.org"    # npm
    deny:
      - "0.0.0.0/0"            # block everything else
```

For full offline mode (cmd steps only):
```yaml
config:
  cmd:
    shell: "docker run --rm --network none -v $(pwd):/workspace -w /workspace minion-sandbox"
```

---

## Resource Limits

```yaml
sandbox:
  resources:
    cpus: 2.0
    memory: "4g"
```

---

## Notes

- The `--rm` flag removes the container after execution — zero state between runs.
- Host paths in `volumes:` that don't exist are silently skipped (no error).
- The container always runs as `root` inside; credential files are mounted read-only for safety.
- For long-running workflows, pre-bake dependencies into the image to avoid `npm install` on every run.
