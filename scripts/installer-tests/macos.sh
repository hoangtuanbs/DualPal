#!/usr/bin/env bash
# macOS installer smoke checks (Phase 7, L8).
# Usage: scripts/installer-tests/macos.sh <artifact-dir>
set -euo pipefail

dir="${1:?artifact dir}"
# The app version, read without Node (bare distro containers have none).
version="$(sed -n 's/^  "version": *"\([^"]*\)".*/\1/p' package.json | head -n 1)"
test -n "$version"

dmg="$dir/Dual.Pal_${version}_universal.dmg"
test -f "$dmg"

# INS-MAC-001: mount and check the bundle.
mnt="$(mktemp -d)"
hdiutil attach -nobrowse -mountpoint "$mnt" "$dmg" >/dev/null
trap 'hdiutil detach "$mnt" >/dev/null || true' EXIT

app="$mnt/Dual Pal.app"
test -d "$app"
test -d "$mnt/Applications" || test -L "$mnt/Applications"

# INS-MAC-002: universal binary.
lipo -archs "$app/Contents/MacOS/dualpal" | grep -q 'x86_64 arm64'

# INS-MAC-004: bundle id, version and minimum system.
plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist" | grep -q 'com.hoangtuanbs.dualpal'
plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist" | grep -q "$version"
plutil -extract LSMinimumSystemVersion raw "$app/Contents/Info.plist" | grep -q '11.0'

# INS-MAC-003: ad-hoc signature is valid.
codesign --verify --deep --strict "$app"

# INS-MAC-005: after clearing the quarantine flag (what users are told to do
# for an unsigned app), it launches and stays up for 10 s.
copy="$(mktemp -d)/Dual Pal.app"
cp -R "$app" "$copy"
xattr -cr "$copy"
tmp="$(mktemp -d)"
DUALPAL_DATA_DIR="$tmp/data" DUALPAL_CACHE_DIR="$tmp/cache" DUALPAL_HOME="$tmp/home" DUALPAL_SECRETS=memory   "$copy/Contents/MacOS/dualpal" &
pid=$!
sleep 10
kill -0 "$pid" || { echo "app exited early"; exit 1; }
kill "$pid" 2>/dev/null || true

echo "macos installer checks passed"
