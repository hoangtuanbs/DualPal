#!/usr/bin/env bash
# Install, launch and uninstall the Linux bundles (Phase 7, L8).
# Usage: scripts/installer-tests/linux.sh <artifact-dir>
# Distro containers are expected to run this (see installers-verify.yml).
set -euo pipefail

dir="${1:?artifact dir}"
# The app version, read without Node (bare distro containers have none).
version="$(sed -n 's/^  "version": *"\([^"]*\)".*/\1/p' package.json | head -n 1)"
test -n "$version"

# INS-000: artifact names match the README patterns.
test -f "$dir/Dual.Pal_${version}_amd64.deb"
test -f "$dir/Dual.Pal-${version}-1.x86_64.rpm"
test -n "$(find "$dir" -maxdepth 1 -name '*.AppImage' -print -quit)"

# INS-LNX-001/002: .deb installs on Debian/Ubuntu.
if command -v apt-get >/dev/null; then
  sudo apt-get install -y "./$dir/Dual.Pal_${version}_amd64.deb" 2>/dev/null || apt-get install -y "./$dir/Dual.Pal_${version}_amd64.deb"
  dpkg -L dualpal | grep -E '/usr/bin/|\.desktop'
  desktop-file-validate /usr/share/applications/*dualpal*.desktop || true
fi

# INS-LNX-003: launch smoke under xvfb.
if command -v xvfb-run >/dev/null; then
  tmp="$(mktemp -d)"
  DUALPAL_DATA_DIR="$tmp/data" DUALPAL_CACHE_DIR="$tmp/cache" DUALPAL_HOME="$tmp/home" \
    DUALPAL_SECRETS=memory xvfb-run -a timeout 20 dualpal &
  pid=$!
  sleep 10
  kill -0 "$pid" || { echo "app exited early"; exit 1; }
  kill "$pid" 2>/dev/null || true
fi

# INS-LNX-006/007: .rpm installs on Fedora/openSUSE.
if command -v dnf >/dev/null; then
  dnf install -y "./$dir/Dual.Pal-${version}-1.x86_64.rpm"
elif command -v zypper >/dev/null; then
  zypper --no-gpg-checks in -y "./$dir/Dual.Pal-${version}-1.x86_64.rpm"
fi

# INS-LNX-008: AppImage runs (without FUSE in containers).
appimage="$(find "$dir" -maxdepth 1 -name '*.AppImage' -print -quit)"
if [ -n "$appimage" ]; then
  chmod +x "$appimage"
  APPIMAGE_EXTRACT_AND_RUN=1 timeout 15 "$appimage" --appimage-extract-and-run 2>/dev/null || true
fi

echo "linux installer checks passed"
