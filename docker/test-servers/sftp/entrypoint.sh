#!/bin/sh
# Set up the test user and keys, then run sshd in the foreground.
#   SFTP_USER        login name (default: tester)
#   SFTP_PASSWORD    password, which also signs in (default: dualpal)
#   AUTHORIZED_KEYS  more public key lines allowed to sign in
set -eu

user="${SFTP_USER:-tester}"
password="${SFTP_PASSWORD:-dualpal}"
mkdir -p /keys

# A client key for the tests, readable by whoever started the container.
if [ ! -f /keys/id_ed25519 ]; then
    ssh-keygen -q -t ed25519 -N '' -C dualpal-test -f /keys/id_ed25519
fi
chmod 644 /keys/id_ed25519 /keys/id_ed25519.pub
# Keep the host key across restarts, so known_hosts stays valid.
if [ ! -f /keys/ssh_host_ed25519_key ]; then
    cp /etc/ssh/ssh_host_ed25519_key /etc/ssh/ssh_host_ed25519_key.pub /keys/
fi

if ! id "$user" >/dev/null 2>&1; then
    adduser -D -s /bin/sh "$user"
fi
echo "$user:$password" | chpasswd

home="$(getent passwd "$user" | cut -d: -f6)"
mkdir -p "$home/.ssh"
cat /keys/id_ed25519.pub > "$home/.ssh/authorized_keys"
if [ -n "${AUTHORIZED_KEYS:-}" ]; then
    printf '%s\n' "$AUTHORIZED_KEYS" >> "$home/.ssh/authorized_keys"
fi
chown -R "$user:$user" "$home/.ssh"
chmod 700 "$home/.ssh"
chmod 600 "$home/.ssh/authorized_keys"

cat > /etc/ssh/sshd_config <<CONF
Port 22
HostKey /keys/ssh_host_ed25519_key
PermitRootLogin no
PubkeyAuthentication yes
PasswordAuthentication yes
KbdInteractiveAuthentication no
Subsystem sftp internal-sftp
CONF

echo "Dual Pal test server: $user@<host>:2222, password $password, key /keys/id_ed25519"
ssh-keygen -l -f /keys/ssh_host_ed25519_key.pub
exec /usr/sbin/sshd -D -e
