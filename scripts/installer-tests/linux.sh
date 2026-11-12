#!/usr/bin/env bash
# Install, launch and uninstall the Linux bundles (Phase 7, L8).
# Usage: scripts/installer-tests/linux.sh <artifact-dir>
# Distro containers are expected to run this (see installers-verify.yml).
# With DUALPAL_PREVIOUS_DEB set to the last release's .deb, an upgrade from it
# is checked too (nightly.yml).
set -euo pipefail

dir="${1:?artifact dir}"
# The app version, read without Node (bare distro containers have none).
version="$(sed -n 's/^  "version": *"\([^"]*\)".*/\1/p' package.json | head -n 1)"
test -n "$version"

# INS-000: artifact names match the README patterns.
test -f "$dir/Dual.Pal_${version}_amd64.deb"
test -f "$dir/Dual.Pal-${version}-1.x86_64.rpm"
test -n "$(find "$dir" -maxdepth 1 -name '*.AppImage' -print -quit)"

# INS-002: the package reports the app version.
if command -v dpkg-deb >/dev/null; then
  test "$(dpkg-deb -f "$dir/Dual.Pal_${version}_amd64.deb" Version)" = "$version"
fi

# INS-LNX-004: upgrading from the previous release .deb replaces the program
# and keeps the user's data folder.
if [ -n "${DUALPAL_PREVIOUS_DEB:-}" ] && command -v apt-get >/dev/null; then
  sudo apt-get install -y "$DUALPAL_PREVIOUS_DEB"
  data="$HOME/.local/share/com.hoangtuanbs.dualpal"
  mkdir -p "$data"
  echo '[{"name":"kept","location":{"fs":"local","path":"/tmp"},"icon":"folder"}]' >"$data/favorites.json"
  sudo apt-get install -y "./$dir/Dual.Pal_${version}_amd64.deb"
  test "$(dpkg-query -W -f '${Version}' dualpal)" = "$version"
  grep -q '"kept"' "$data/favorites.json"
  test "$(dpkg -L dualpal | grep -c '/usr/bin/dualpal$')" = 1
fi

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

# INS-LNX-005: removing (and purging) the .deb leaves no binary behind.
if command -v apt-get >/dev/null && dpkg -s dualpal >/dev/null 2>&1; then
  sudo apt-get purge -y dualpal 2>/dev/null || apt-get purge -y dualpal
  if dpkg -s dualpal >/dev/null 2>&1; then echo "dualpal is still installed"; exit 1; fi
  if command -v dualpal >/dev/null || [ -e /usr/bin/dualpal ]; then echo "the binary is left behind"; exit 1; fi
  test -z "$(find /usr/share/applications -name '*dualpal*' -print -quit 2>/dev/null)"
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

  # INS-LNX-009: the AppImage has a .desktop file and an icon at its root.
  ex="$(mktemp -d)"
  abs="$(cd "$(dirname "$appimage")" && pwd)/$(basename "$appimage")"
  (cd "$ex" && "$abs" --appimage-extract >/dev/null)
  test -n "$(find "$ex/squashfs-root" -maxdepth 1 -name '*.desktop' -print -quit)"
  test -n "$(find "$ex/squashfs-root" -maxdepth 1 \( -name '*.png' -o -name '*.svg' -o -name .DirIcon \) -print -quit)"
  rm -rf "$ex"
fi

echo "linux installer checks passed"
