# Minion Engine — Docker Sandbox

Running `cmd` and `agent` steps inside a Docker container provides isolation: untrusted commands cannot affect the host system.

---

## Basic Usage

Override the `command` config to wrap execution in Docker:

```yaml
name: sandboxed-workflow
config:
  cmd:
    # All cmd steps run inside the container
    shell: "docker run --rm -v $(pwd):/workspace -w /workspace my-sandbox"
  agent:
    # Claude CLI runs inside a container with tools available
    command: "docker run --rm -i -v $(pwd):/workspace -w /workspace my-sandbox claude"
```

---

## Building a Sandbox Image

```dockerfile
# Dockerfile.sandbox
FROM ubuntu:22.04

RUN apt-get update && apt-get install -y \
    curl git gh jq nodejs npm cargo \
    && rm -rf /var/lib/apt/lists/*

# Install Claude CLI
RUN curl -fsSL https://claude.ai/install.sh | bash

WORKDIR /workspace
```

Build it:

```bash
docker build -f Dockerfile.sandbox -t my-sandbox .
```

---

## Mounting Claude Credentials

The Claude CLI stores credentials in `~/.claude`. Mount the config directory so the containerized CLI can authenticate:

```bash
docker run --rm -i \
  -v $(pwd):/workspace \
  -v $HOME/.claude:/root/.claude:ro \
  -w /workspace \
  my-sandbox claude -p ...
```

In your workflow config:

```yaml
config:
  agent:
    command: >-
      docker run --rm -i
      -v $PWD:/workspace
      -v $HOME/.claude:/root/.claude:ro
      -w /workspace
      my-sandbox claude
```

---

## Network Isolation

To prevent `cmd` steps from making outbound network calls while still allowing the Claude CLI:

```yaml
config:
  cmd:
    shell: "docker run --rm --network none -v $(pwd):/workspace -w /workspace my-sandbox"
  agent:
    command: "docker run --rm -i -v $(pwd):/workspace -w /workspace my-sandbox claude"
```

---

## Resource Limits

Constrain CPU and memory:

```yaml
config:
  agent:
    command: >-
      docker run --rm -i
      --cpus=2
      --memory=2g
      -v $PWD:/workspace -w /workspace
      my-sandbox claude
```

---

## Notes

- The `--rm` flag removes the container after each step — there is no state between steps unless you mount a shared volume.
- Use named volumes or bind mounts to persist output across steps.
- For long-running workflows, consider building a custom image with all required tools pre-installed to reduce per-step container startup overhead.
