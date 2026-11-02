# Test servers

Two throwaway servers for trying Dual Pal and running its integration tests
without touching a real server. Both listen on 127.0.0.1 only and are not
meant for anything else.

| Server | Address | Sign-in |
|---|---|---|
| SSH / SFTP (OpenSSH on Alpine) | `127.0.0.1:2222` | user `tester`, password `dualpal`, or the key in `keys/id_ed25519` |
| S3 (moto emulator) | `http://127.0.0.1:9000` | bucket `dualpal-test`, access key `test`, secret `test-secret`, path-style |

## Start and stop

```sh
docker compose -f docker/test-servers/compose.yaml up -d --build
docker compose -f docker/test-servers/compose.yaml down
```

On first start the SSH server creates a client key in `keys/` (ignored by
git) and keeps its host key there, so the fingerprint stays the same across
restarts. To also allow your own key, set `AUTHORIZED_KEYS` on the `sftp`
service.

In Dual Pal, add a server with host `127.0.0.1`, port `2222`, user `tester`,
and either the password or the key file `docker/test-servers/keys/id_ed25519`.

## Simulate a dropped connection

```sh
docker exec dualpal-test-sftp drop-connections
```

This cuts every open SSH session while the server keeps running, as a
network drop would. A copy in progress should pause briefly, reconnect and
finish by itself.

## Run the integration tests against them

```sh
cp docker/test-servers/keys/id_ed25519 /tmp/dualpal_test_key && chmod 600 /tmp/dualpal_test_key
DUALPAL_TEST_SFTP=tester@127.0.0.1:2222 \
DUALPAL_TEST_SFTP_KEY=/tmp/dualpal_test_key \
DUALPAL_TEST_SFTP_PASSWORD=dualpal \
DUALPAL_TEST_SFTP_DROP="docker exec dualpal-test-sftp drop-connections" \
DUALPAL_TEST_S3="http://127.0.0.1:9000|dualpal-test|test|test-secret" \
  cargo test -p dualpal-core --test remote
```

Linux CI does the same on every pull request.
