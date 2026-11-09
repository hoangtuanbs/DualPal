#!/usr/bin/env bash
# Traceability check (§11.4): every P0/P1 test ID in the plan's catalogue must
# appear in test code. Lists the missing IDs; with --strict it also fails.
# IDs written as ranges in comments ("CMD-016/017", "UNIT-001…003") count.
set -euo pipefail

strict=0
[ "${1:-}" = "--strict" ] && strict=1

plan="docs/testing/AUTOMATION_TEST_PLAN.md"
id='[A-Z][A-Z0-9]+(-[A-Z0-9]+)*-[0-9]{3}'

# P0/P1 rows of the catalogue tables, plus the keyboard table (no P column).
plan_ids="$(grep -E "^\| \**${id}\**[[:space:]]*\|" "$plan" \
  | grep -E '\| *P[01] *\||^\| UI-KEY-' \
  | grep -oE "^\| \**${id}" | grep -oE "$id" | sort -u)"

# Every ID mentioned in code, expanding "X-016/017" and "X-001…003" / "X-001..003".
code_ids="$(grep -rhoE "${id}((/[0-9]{3})+|(…|\.\.)[0-9]{3})?" \
  --exclude-dir=node_modules --exclude-dir=target --exclude=trace-check.sh \
  crates src-tauri ui e2e-desktop scripts 2>/dev/null \
  | python3 -c '
import re, sys
out = set()
for t in sys.stdin:
    pre, first, rest = re.match(r"(.*-)(\d{3})(.*)", t.strip()).groups()
    out.add(pre + first)
    out.update(pre + x for x in re.findall(r"/(\d{3})", rest))
    r = re.match(r"(?:…|\.\.)(\d{3})", rest)
    if r:
        out.update(f"{pre}{i:03d}" for i in range(int(first), int(r.group(1)) + 1))
print("\n".join(sorted(out)))
')"

missing="$(comm -23 <(echo "$plan_ids") <(echo "$code_ids"))"
total="$(echo "$plan_ids" | grep -c . || true)"
count="$(echo "$missing" | grep -c . || true)"
echo "P0/P1 test IDs: $((total - count)) of $total found in test code"
if [ "$count" -gt 0 ]; then
  echo "missing:"
  echo "$missing" | paste -sd' ' | fold -s -w 100
  [ "$strict" -eq 1 ] && exit 1
fi
exit 0
