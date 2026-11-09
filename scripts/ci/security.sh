#!/usr/bin/env bash
# Supply-chain and secret checks (Phase 8, L9: SEC-007, SEC-002).
set -euo pipefail

echo "== cargo audit =="
cargo audit

echo "== cargo deny =="
cargo deny check

echo "== npm audit =="
npm audit --omit=dev --audit-level=high

# SEC-002: no test secret may leak into the data/cache dirs or captured output.
if [ -n "${DUALPAL_TEST_DATA_DIR:-}" ]; then
  if grep -rIl -e S3CR3T -e test-secret -e dualpal-pass "${DUALPAL_TEST_DATA_DIR}" 2>/dev/null; then
    echo "a test secret leaked into the data dir"; exit 1
  fi
fi

echo "security checks passed"
