#!/usr/bin/env bash
# INS-REL-002: list every installer file under a downloaded-artifacts folder.
# Artifacts keep their bundle subfolders (msi/, nsis/, deb/, macos/…), so the
# files are collected from every level, one per line, sorted. Fails if none.
#
# Usage: scripts/release/collect-installers.sh <dir>
set -euo pipefail

dir="${1:?usage: collect-installers.sh <dir>}"
found="$(find "$dir" -type f \( -name '*.msi' -o -name '*.exe' -o -name '*.deb' -o -name '*.rpm' \
  -o -name '*.AppImage' -o -name '*.dmg' -o -name '*.app.tar.gz' -o -name '*.sig' \) | LC_ALL=C sort)"
if [ -z "$found" ]; then
  echo "::error::no installers found under $dir" >&2
  exit 1
fi
printf '%s\n' "$found"
