# README demo

`docs/assets/demo.gif` is a real run, recorded and rendered by these scripts.

```bash
cd scripts/demo
npm install --no-save asciinema-player gifenc pngjs playwright
./record.py demo.cast                                # see "Requirements" below
node render.mjs demo.cast ../../docs/assets/demo.gif # uses the local Google Chrome
```

Requirements: `stepyard` and `docker` on PATH, the `stepyard-sandbox:latest`
image and `ANTHROPIC_API_KEY` in the environment. The
workflow runs with `--sandbox-runtime docker`, so Claude Code runs in the
container.

`record.py` copies `project/` (a one-function Python repo with a failing test)
to `/tmp/stepyard-demo/calculator`, types the commands, and captures the real
output through a pty. Only the waiting time while the agent works is shortened;
the printed durations are the real ones.
