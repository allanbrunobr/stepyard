# README demo

`docs/assets/demo.gif` is a real run, recorded and rendered by these scripts.

```bash
cd scripts/demo
npm install --no-save asciinema-player gifenc pngjs playwright
./record.py demo.cast                                # needs stepyard + claude on PATH
node render.mjs demo.cast ../../docs/assets/demo.gif # uses the local Google Chrome
```

`record.py` copies `project/` (a one-function Python repo with a failing test)
to `/tmp/stepyard-demo/calculator`, types the commands, and captures the real
output through a pty. Only the waiting time while the agent works is shortened;
the printed durations are the real ones.
