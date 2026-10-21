#!/bin/sh
set -eu

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
guard="$repo_root/scripts/guard-cargo-target-size.sh"

if [ "$#" -eq 0 ]; then
  echo "Usage: scripts/cargo-guarded.sh <cargo-args...>" >&2
  echo "Example: scripts/cargo-guarded.sh test -p stepyard-core" >&2
  exit 2
fi

"$guard"
cargo "$@"
"$guard"
