#!/bin/sh
# Print the environment for the remote (L3) test suite, ready to `eval`:
#
#   eval "$(scripts/ci/remote-env.sh)"
#
# CI runs this after `docker compose up --wait` and pipes it into $GITHUB_ENV.
# Values are expanded here (not escaped), so both uses get plain KEY=VALUE.
#
# Missing variables make tests fail rather than skip, because
# DUALPAL_REQUIRE_REMOTE=1 (see PREP-07).

# The SFTP client key. In CI the runner copies the repo key here with 600 perms;
# locally point it at the copy under /tmp (see docker/test-servers/README.md).
sftp_key="${RUNNER_TEMP:-/tmp}/dualpal_sftp_key"

cat <<EOF
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
EOF
