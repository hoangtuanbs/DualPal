#!/usr/bin/env bash
# Installs the Debian/Ubuntu packages named as arguments, but only the ones
# that are missing. On a self-hosted runner where they are already installed
# this needs no sudo at all. If something is missing it uses `sudo -n`, which
# fails at once instead of waiting for a password, and prints the one command
# to run by hand on the runner machine.
set -euo pipefail

missing=()
for pkg in "$@"; do
  if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "install ok installed"; then
    missing+=("$pkg")
  fi
done

if [ ${#missing[@]} -eq 0 ]; then
  echo "All system packages already installed."
  exit 0
fi

echo "Missing system packages: ${missing[*]}"
if sudo -n true 2>/dev/null; then
  sudo -n apt-get update
  sudo -n DEBIAN_FRONTEND=noninteractive apt-get install -y "${missing[@]}"
else
  echo "::error::sudo needs a password on this runner. Install once on the runner machine:"
  echo "::error::  sudo apt-get install -y ${missing[*]}"
  exit 1
fi
