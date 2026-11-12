#!/usr/bin/env bash
# Self-test of the release scripts against fixtures (INS-001, INS-REL-001,
# INS-REL-002). Runs in the CI lint job; needs bash, node and find.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail=0
expect() { # expect <0|1> <description> <command…>
  local want="$1" what="$2"
  shift 2
  if "$@" >"$tmp/out" 2>&1; then got=0; else got=1; fi
  if [ "$got" != "$want" ]; then
    echo "FAIL: $what (exit $got, wanted $want)"
    sed 's/^/    /' "$tmp/out"
    fail=1
  else
    echo "ok: $what"
  fi
}

# A checkout with the three version files.
fixture() { # fixture <dir> <cargo> <node> <tauri>
  mkdir -p "$1/src-tauri"
  printf '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "%s"\nedition = "2021"\n\n[workspace.dependencies]\nfoo = { version = "9.9.9" }\n' "$2" >"$1/Cargo.toml"
  printf '{ "name": "x", "version": "%s" }\n' "$3" >"$1/package.json"
  printf '{ "productName": "Dual Pal", "version": "%s" }\n' "$4" >"$1/src-tauri/tauri.conf.json"
}

# INS-001
fixture "$tmp/same" 1.2.3 1.2.3 1.2.3
fixture "$tmp/cargo" 1.2.4 1.2.3 1.2.3
fixture "$tmp/tauri" 1.2.3 1.2.3 1.3.0
expect 0 "INS-001 equal versions pass" env DUALPAL_ROOT="$tmp/same" bash "$here/check-version.sh"
expect 1 "INS-001 a different Cargo.toml version fails" env DUALPAL_ROOT="$tmp/cargo" bash "$here/check-version.sh"
expect 1 "INS-001 a different tauri.conf.json version fails" env DUALPAL_ROOT="$tmp/tauri" bash "$here/check-version.sh"
expect 0 "INS-001 this repository's versions agree" bash "$here/check-version.sh"

# INS-REL-001
expect 0 "INS-REL-001 matching tag passes" env DUALPAL_ROOT="$tmp/same" bash "$here/check-version.sh" v1.2.3
expect 1 "INS-REL-001 tag without v fails" env DUALPAL_ROOT="$tmp/same" bash "$here/check-version.sh" 1.2.3
expect 1 "INS-REL-001 other tag fails" env DUALPAL_ROOT="$tmp/same" bash "$here/check-version.sh" v1.2.4

# INS-REL-002: nested artifact folders, a name with spaces, and noise.
art="$tmp/installers"
mkdir -p "$art/windows/msi" "$art/windows/nsis" "$art/linux/deb" "$art/linux/appimage" "$art/macos/dmg"
touch "$art/windows/msi/Dual Pal_1.2.3_x64_en-US.msi" "$art/windows/nsis/Dual Pal_1.2.3_x64-setup.exe" \
  "$art/linux/deb/dual-pal_1.2.3_amd64.deb" "$art/linux/appimage/dual-pal_1.2.3_amd64.AppImage" \
  "$art/macos/dmg/Dual Pal_1.2.3_aarch64.dmg" "$art/linux/deb/control.txt"
expect 0 "INS-REL-002 collects from every level" bash "$here/collect-installers.sh" "$art"
count="$(bash "$here/collect-installers.sh" "$art" | wc -l | tr -d ' ')"
if [ "$count" != 5 ]; then
  echo "FAIL: INS-REL-002 expected 5 installers, got $count"
  bash "$here/collect-installers.sh" "$art"
  fail=1
else
  echo "ok: INS-REL-002 five installers, other files left out"
fi
mkdir -p "$tmp/empty/sub"
expect 1 "INS-REL-002 no installers fails" bash "$here/collect-installers.sh" "$tmp/empty"

exit "$fail"
