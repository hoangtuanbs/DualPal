#!/usr/bin/env bash
# INS-001: the app version in Cargo.toml (workspace), package.json and
# tauri.conf.json must agree. INS-REL-001: when a tag is given (e.g. the
# GITHUB_REF_NAME of a release build), it must be "v" + that version.
#
# Usage: scripts/release/check-version.sh [tag]
# DUALPAL_ROOT points at another checkout (the self-test uses fixtures).
set -euo pipefail

root="${DUALPAL_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
cd "$root"

# The first `version = "…"` after [workspace.package].
cargo_v="$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/.*= *"|".*/,""); print; exit}' Cargo.toml)"
node_v="$(node -p "require('./package.json').version")"
tauri_v="$(node -p "require('./src-tauri/tauri.conf.json').version")"

if [ -z "$cargo_v" ] || [ "$cargo_v" != "$node_v" ] || [ "$node_v" != "$tauri_v" ]; then
  echo "::error::versions differ: Cargo.toml=$cargo_v package.json=$node_v tauri.conf.json=$tauri_v"
  exit 1
fi

tag="${1:-}"
if [ -n "$tag" ] && [ "$tag" != "v$node_v" ]; then
  echo "::error::tag $tag does not match version $node_v"
  exit 1
fi

echo "version $node_v${tag:+ matches tag $tag}"
