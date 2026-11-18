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

# The bundler names files "Dual Pal_<version>_..." with a space; GitHub release
# assets turn the space into a dot. Use the release names, so CI build
# artifacts and release downloads look the same here.
for f in "$dir"/Dual\ Pal[_-]*; do
  if [ -e "$f" ]; then mv "$f" "$dir/$(basename "$f" | tr ' ' .)"; fi
done

# INS-000: artifact names match the README patterns.
test -f "$dir/Dual.Pal_${version}_amd64.deb"
test -f "$dir/Dual.Pal-${version}-1.x86_64.rpm"
test -n "$(find "$dir" -maxdepth 1 -name '*.AppImage' -print -quit)"

# The Debian package name comes from the product name ("dual-pal"), not the
# binary name ("dualpal"); read it from the .deb.
pkg=dual-pal
# INS-002: the package reports the app version.
if command -v dpkg-deb >/dev/null; then
  pkg="$(dpkg-deb -f "$dir/Dual.Pal_${version}_amd64.deb" Package)"
  test "$(dpkg-deb -f "$dir/Dual.Pal_${version}_amd64.deb" Version)" = "$version"
fi

# Run as root: through sudo on a runner, directly in a distro container
# (which has no sudo).
as_root() { if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi; }

# The launch check needs a virtual display; bare distro containers have none.
if ! command -v xvfb-run >/dev/null; then
  if command -v apt-get >/dev/null; then
    as_root apt-get update -qq
    DEBIAN_FRONTEND=noninteractive as_root apt-get install -y -qq xvfb xauth
  elif command -v dnf >/dev/null; then
    dnf install -y xorg-x11-server-Xvfb xorg-x11-xauth which
  elif command -v zypper >/dev/null; then
    zypper --non-interactive install xvfb-run
  fi
fi

# Fails when a library or glibc version the binary needs is missing (a build
# made on a newer distro than this one), then starts the app under xvfb and
# checks it is still running after 10 seconds.
check_launch() {
  local bin
  bin="$(command -v dualpal)"
  if ldd "$bin" 2>&1 | grep -E 'not found'; then
    echo "dualpal cannot load on this system"; exit 1
  fi
  local tmp
  tmp="$(mktemp -d)"
  DUALPAL_DATA_DIR="$tmp/data" DUALPAL_CACHE_DIR="$tmp/cache" DUALPAL_HOME="$tmp/home" \
    DUALPAL_SECRETS=memory xvfb-run -a timeout 20 "$bin" &
  local pid=$!
  sleep 10
  kill -0 "$pid" || { echo "app exited early"; exit 1; }
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

# Bare Debian/Ubuntu containers start with empty package lists, so the .deb's
# dependencies (WebKitGTK, GTK) cannot be resolved until they are fetched.
if command -v apt-get >/dev/null; then
  as_root apt-get update -qq
fi

# INS-LNX-004: upgrading from the previous release .deb replaces the program
# and keeps the user's data folder.
if [ -n "${DUALPAL_PREVIOUS_DEB:-}" ] && command -v apt-get >/dev/null; then
  as_root apt-get install -y "$DUALPAL_PREVIOUS_DEB"
  data="$HOME/.local/share/com.hoangtuanbs.dualpal"
  mkdir -p "$data"
  echo '[{"name":"kept","location":{"fs":"local","path":"/tmp"},"icon":"folder"}]' >"$data/favorites.json"
  as_root apt-get install -y "./$dir/Dual.Pal_${version}_amd64.deb"
  test "$(dpkg-query -W -f '${Version}' "$pkg")" = "$version"
  grep -q '"kept"' "$data/favorites.json"
  test "$(dpkg -L "$pkg" | grep -c '/usr/bin/dualpal$')" = 1
fi

# INS-LNX-001/002: .deb installs on Debian/Ubuntu.
if command -v apt-get >/dev/null; then
  as_root apt-get install -y "./$dir/Dual.Pal_${version}_amd64.deb"
  dpkg -L "$pkg" | grep -E '/usr/bin/|\.desktop'
  desktop-file-validate /usr/share/applications/*dualpal*.desktop || true
fi

# INS-LNX-003: the installed app starts. Run after each package install.
check_launch

# INS-LNX-005: removing (and purging) the .deb leaves no binary behind.
if command -v apt-get >/dev/null && dpkg -s "$pkg" >/dev/null 2>&1; then
  as_root apt-get purge -y "$pkg"
  if dpkg -s "$pkg" >/dev/null 2>&1; then echo "$pkg is still installed"; exit 1; fi
  if command -v dualpal >/dev/null || [ -e /usr/bin/dualpal ]; then echo "the binary is left behind"; exit 1; fi
  test -z "$(find /usr/share/applications -name '*dualpal*' -print -quit 2>/dev/null)"
fi

# INS-LNX-006/007: .rpm installs on Fedora/openSUSE.
if command -v dnf >/dev/null; then
  dnf install -y "./$dir/Dual.Pal-${version}-1.x86_64.rpm"
elif command -v zypper >/dev/null; then
  zypper --no-gpg-checks in -y "./$dir/Dual.Pal-${version}-1.x86_64.rpm"
fi
if command -v dnf >/dev/null || command -v zypper >/dev/null; then
  check_launch
fi

# INS-LNX-008: AppImage runs (without FUSE in containers).
appimage="$(find "$dir" -maxdepth 1 -name '*.AppImage' -print -quit)"
if [ -n "$appimage" ]; then
  chmod +x "$appimage"
  # Still running when the timeout stops it (124) means it started.
  status=0
  APPIMAGE_EXTRACT_AND_RUN=1 xvfb-run -a timeout 15 "$appimage" --appimage-extract-and-run || status=$?
  if [ "$status" != 124 ]; then echo "AppImage exited early ($status)"; exit 1; fi

  # INS-LNX-009: the AppImage has a .desktop file and an icon at its root.
  ex="$(mktemp -d)"
  abs="$(cd "$(dirname "$appimage")" && pwd)/$(basename "$appimage")"
  (cd "$ex" && "$abs" --appimage-extract >/dev/null)
  test -n "$(find "$ex/squashfs-root" -maxdepth 1 -name '*.desktop' -print -quit)"
  test -n "$(find "$ex/squashfs-root" -maxdepth 1 \( -name '*.png' -o -name '*.svg' -o -name .DirIcon \) -print -quit)"
  rm -rf "$ex"
fi

echo "linux installer checks passed"
