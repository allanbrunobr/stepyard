#!/usr/bin/env python3
"""Record the README demo as an asciicast v2 file (real commands, real output).

Usage: scripts/demo/record.py OUT.cast
Needs `stepyard` and `claude` (Claude Code, logged in) on PATH.
The demo project is copied to /tmp/stepyard-demo, so the repo is never touched.
"""
import json, os, pty, select, shutil, subprocess, sys, time

COLS, ROWS = 92, 30
PROMPT = "\x1b[1;36mcalculator\x1b[0m \x1b[2m$\x1b[0m "
TYPE_DELAY = 0.045
# Long silent stretches (the agent thinking) are squeezed to this many
# seconds so the GIF stays short; the output itself is unchanged.
MAX_GAP = 1.6
# Spinner-only redraws beyond this many seconds in a row are dropped for the
# same reason (the agent step takes ~15s; the GIF shows ~2.5s of it).
MAX_SPINNER = 2.5

STEPS = [
    "cat calc.py",
    "python3 -m unittest -q",
    "stepyard execute fix-tests.yaml",
    "git diff",
    "python3 -m unittest -q",
]


def main(out_path):
    here = os.path.dirname(os.path.abspath(__file__))
    work = "/tmp/stepyard-demo/calculator"  # short path for tracebacks
    shutil.rmtree(os.path.dirname(work), ignore_errors=True)
    shutil.copytree(os.path.join(here, "project"), work)
    git = ["git", "-c", "user.name=demo", "-c", "user.email=demo@example.com"]
    subprocess.run(["git", "init", "-q"], cwd=work, check=True)
    with open(os.path.join(work, ".gitignore"), "w") as f:
        f.write(".stepyard/\n__pycache__/\n.gitignore\n")
    subprocess.run(git + ["add", "-A"], cwd=work, check=True)
    subprocess.run(git + ["commit", "-qm", "init"], cwd=work, check=True)

    events, clock = [], 0.0

    def emit(text, dt=0.0):
        nonlocal clock
        clock += dt
        events.append([round(clock, 3), "o", text])

    env = dict(os.environ, COLUMNS=str(COLS), LINES=str(ROWS), TERM="xterm-256color",
               PYTHONDONTWRITEBYTECODE="1", GIT_PAGER="cat", CLICOLOR_FORCE="1")
    emit(PROMPT, 0.3)
    for cmd in STEPS:
        for ch in cmd:
            emit(ch, TYPE_DELAY)
        emit("\r\n", 0.35)
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(work)
            os.execvpe("bash", ["bash", "--norc", "--noprofile", "-c",
                                f"stty cols {COLS} rows {ROWS}; " + cmd.replace("git diff", "git -c color.ui=always diff")], env)
        last = time.monotonic()
        spinner = 0.0
        while True:
            r, _, _ = select.select([fd], [], [], 0.05)
            if not r:
                if os.waitpid(pid, os.WNOHANG)[0]:
                    break
                continue
            try:
                data = os.read(fd, 4096)
            except OSError:
                break
            if not data:
                break
            now = time.monotonic()
            text = data.decode("utf-8", "replace")
            if "\n" not in text:  # spinner redraw, no new output line
                spinner += now - last
                if spinner > MAX_SPINNER:
                    # keep the bytes (they also clear lines), drop the time
                    emit(text, 0.0)
                    last = now
                    continue
            else:
                spinner = 0.0
            emit(text, min(now - last, MAX_GAP))
            last = now
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass
        emit(PROMPT, 0.25)
        clock += 1.2  # pause so the viewer can read the output
    emit("", 1.5)

    with open(out_path, "w") as f:
        f.write(json.dumps({"version": 2, "width": COLS, "height": ROWS,
                            "env": {"TERM": "xterm-256color"}}) + "\n")
        for e in events:
            f.write(json.dumps(e) + "\n")
    print(f"wrote {out_path} ({clock:.1f}s)")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "demo.cast")
