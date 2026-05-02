#!/bin/sh
set -eu

limit_mib="${STEPYARD_CARGO_TARGET_MAX_MIB:-1024}"
repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
workspace_target="$repo_root/target"
target_dir="${CARGO_TARGET_DIR:-/tmp/stepyard-minion-engine-target}"

case "$limit_mib" in
  ''|*[!0-9]*|0)
    echo "ERROR: STEPYARD_CARGO_TARGET_MAX_MIB must be a positive integer; got '$limit_mib'." >&2
    exit 2
    ;;
esac

limit_kib=$((limit_mib * 1024))

human_kib() {
  kib="$1"
  if [ "$kib" -ge 1048576 ]; then
    awk "BEGIN { printf \"%.2f GiB\", $kib / 1048576 }"
  else
    awk "BEGIN { printf \"%.2f MiB\", $kib / 1024 }"
  fi
}

size_kib() {
  du -sk "$1" | awk '{ print $1 }'
}

fail=0

if [ -d "$workspace_target" ]; then
  workspace_kib="$(size_kib "$workspace_target")"
  echo "ERROR: repo-local target/ exists at $workspace_target ($(human_kib "$workspace_kib"))." >&2
  echo "This repo intentionally keeps Cargo build artifacts outside the checkout." >&2
  echo "Run: rm -rf '$workspace_target'" >&2
  fail=1
fi

if [ -d "$target_dir" ]; then
  target_kib="$(size_kib "$target_dir")"
  target_human="$(human_kib "$target_kib")"
  limit_human="$(human_kib "$limit_kib")"
  if [ "$target_kib" -gt "$limit_kib" ]; then
    echo "ERROR: Cargo target dir is $target_human, above limit $limit_human." >&2
    echo "Path: $target_dir" >&2
    echo "Clean it with: cargo clean" >&2
    fail=1
  else
    echo "OK: Cargo target dir is $target_human at $target_dir (limit $limit_human)."
  fi
else
  echo "OK: Cargo target dir does not exist yet: $target_dir (limit $(human_kib "$limit_kib"))."
fi

exit "$fail"
