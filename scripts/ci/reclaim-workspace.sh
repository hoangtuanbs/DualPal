#!/usr/bin/env bash
# Gives every file in the job's work folder back to the runner user.
#
# Containers started by a job (the test servers) can write files as root into
# folders mounted from the work folder. A self-hosted runner keeps that folder
# for the next job, whose actions/checkout then fails to clean it ("EACCES:
# permission denied"). This finds such files and changes their owner from
# inside a throwaway container, which needs no sudo on the runner.
set -euo pipefail

ws="${1:-${GITHUB_WORKSPACE:-$PWD}}"
[ -d "$ws" ] || exit 0
if find "$ws" ! -user "$(id -u)" -print -quit | grep -q .; then
    echo "reclaim-workspace: giving files in $ws back to $(id -un)"
    docker run --rm -v "$ws:/w" alpine:3.20 chown -R "$(id -u):$(id -g)" /w
fi
