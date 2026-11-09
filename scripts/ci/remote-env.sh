#!/bin/sh
# Print the environment for the remote (L3) test suite.
#
#   eval "$(scripts/ci/remote-env.sh --export)"   # local shell: quoted exports
#   scripts/ci/remote-env.sh >> "$GITHUB_ENV"     # CI: plain KEY=VALUE lines
#
# Missing variables make tests fail rather than skip, because
# DUALPAL_REQUIRE_REMOTE=1 (see PREP-07).

# The SFTP client key. In CI the runner copies the repo key here with 600 perms;
# locally point it at the copy under /tmp (see docker/test-servers/README.md).
sftp_key="${RUNNER_TEMP:-/tmp}/dualpal_sftp_key"

vars() {
    cat <<VARS
DUALPAL_REQUIRE_REMOTE=1
DUALPAL_TEST_SFTP=tester@127.0.0.1:2222
DUALPAL_TEST_SFTP_KEY=$sftp_key
DUALPAL_TEST_SFTP_PASSWORD=dualpal
DUALPAL_TEST_SFTP_DROP=docker exec dualpal-test-sftp drop-connections
DUALPAL_TEST_SFTP_PASSPHRASE_KEY=$PWD/docker/test-servers/keys/keyonly_ed25519_utf16|dualpal-pass
DUALPAL_TEST_SFTP_KEYONLY_USER=keyonly@127.0.0.1:2222
DUALPAL_TEST_FTP=tester:dualpal@127.0.0.1:2121
DUALPAL_TEST_FTPS=tester:dualpal@127.0.0.1:2990
DUALPAL_TEST_FTPS_IMPLICIT=tester:dualpal@127.0.0.1:2991
DUALPAL_TEST_WEBDAV=tester:dualpal@http://127.0.0.1:8088/dav
DUALPAL_TEST_S3=http://127.0.0.1:9000|dualpal-test|test|test-secret
DUALPAL_TEST_TOXIPROXY=http://127.0.0.1:8474
VARS
}

if [ "${1:-}" = "--export" ]; then
    # Single-quote every value: some hold spaces and pipes.
    vars | while IFS= read -r line; do
        key=${line%%=*}
        value=$(printf '%s' "${line#*=}" | sed "s/'/'\\\\''/g")
        printf "export %s='%s'\n" "$key" "$value"
    done
else
    vars
fi
