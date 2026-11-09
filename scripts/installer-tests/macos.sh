#!/usr/bin/env bash
# macOS installer smoke checks (Phase 7, L8).
# Usage: scripts/installer-tests/macos.sh <artifact-dir>
set -euo pipefail

dir="${1:?artifact dir}"
version="$(node -p "require('./package.json').version")"

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

echo "macos installer checks passed"
