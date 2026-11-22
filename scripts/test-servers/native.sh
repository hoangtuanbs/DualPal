#!/usr/bin/env bash
# Runs the test servers from docker/test-servers without Docker, as plain
# local processes: the same SFTP, FTP/FTPS, WebDAV and S3 stand-ins, on the
# same 127.0.0.1 ports, with the same users and keys, plus Toxiproxy. Use it
# where Docker or its image registries are not available.
#
#   scripts/test-servers/native.sh up      # install what is missing, start all
#   scripts/test-servers/native.sh down    # stop all
#   scripts/test-servers/native.sh drop    # cut every open SFTP session
#   eval "$(scripts/test-servers/native.sh env)"   # settings for the L3 tests
#
# Needs python3 (3.10 or newer) and go. SFTP runs OpenSSH's sshd, which needs
# root because it signs in as the test users "tester" and "keyonly" (they are
# created if missing); without root, or without sshd, SFTP is skipped.
#
# State (virtualenv, tools, server folders, logs, pids) lives in
# $DUALPAL_NATIVE_DIR, default /tmp/dualpal-test-servers.
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
servers="$repo/docker/test-servers"
keys="$servers/keys"
state="${DUALPAL_NATIVE_DIR:-/tmp/dualpal-test-servers}"
venv="$state/venv"
bin="$state/bin"
host=127.0.0.1

log() { echo "native test servers: $*" >&2; }

wait_port() { # name port
    for _ in $(seq 1 60); do
        if python3 -c "import socket,sys; socket.create_connection(('$host', $2), 1)" 2>/dev/null; then
            return 0
        fi
        sleep 1
    done
    log "$1 did not start on port $2; see $state/$1.log"
    tail -20 "$state/$1.log" >&2 || true
    return 1
}

start() { # name command...
    local name=$1
    shift
    if [ -f "$state/$name.pid" ] && kill -0 "$(cat "$state/$name.pid")" 2>/dev/null; then
        log "$name already running"
        return 0
    fi
    nohup setsid "$@" >"$state/$name.log" 2>&1 &
    echo $! >"$state/$name.pid"
}

install_tools() {
    mkdir -p "$state" "$bin"
    if [ ! -x "$venv/bin/python" ]; then
        log "creating Python environment in $venv"
        python3 -m venv "$venv"
    fi
    if ! "$venv/bin/python" -c "import moto, wsgidav, cheroot, pyftpdlib, OpenSSL, boto3" 2>/dev/null; then
        log "installing moto, WsgiDAV and pyftpdlib"
        "$venv/bin/pip" install -q "moto[server]==5.1.*" boto3 "wsgidav==4.*" cheroot \
            "pyftpdlib==2.*" "pyopenssl==25.*"
    fi
    if [ ! -x "$bin/toxiproxy-server" ] || [ ! -x "$bin/toxiproxy-cli" ]; then
        log "building Toxiproxy 2.9.0"
        GOBIN="$bin" go install github.com/Shopify/toxiproxy/v2/cmd/server@v2.9.0
        GOBIN="$bin" go install github.com/Shopify/toxiproxy/v2/cmd/cli@v2.9.0
        mv "$bin/server" "$bin/toxiproxy-server"
        mv "$bin/cli" "$bin/toxiproxy-cli"
    fi
}

make_keys() {
    mkdir -p "$keys"
    [ -f "$keys/id_ed25519" ] || ssh-keygen -q -t ed25519 -N '' -C dualpal-test -f "$keys/id_ed25519"
    [ -f "$keys/ssh_host_ed25519_key" ] || ssh-keygen -q -t ed25519 -N '' -f "$keys/ssh_host_ed25519_key"
    [ -f "$keys/keyonly_ed25519" ] ||
        ssh-keygen -q -t ed25519 -N 'dualpal-pass' -C dualpal-keyonly -f "$keys/keyonly_ed25519"
    if [ ! -f "$keys/keyonly_ed25519_utf16" ]; then
        # UTF-16 LE with a BOM, for the UTF-16 key flow.
        python3 -c "import sys; d=open(sys.argv[1],encoding='ascii').read(); open(sys.argv[2],'wb').write(b'\xff\xfe'+d.encode('utf-16-le'))" \
            "$keys/keyonly_ed25519" "$keys/keyonly_ed25519_utf16"
    fi
    chmod 644 "$keys"/id_ed25519 "$keys"/id_ed25519.pub "$keys"/keyonly_ed25519*
    chmod 600 "$keys/ssh_host_ed25519_key"
}

add_user() { # name authorized-key-file [password]
    local user=$1
    id "$user" >/dev/null 2>&1 || useradd -m -s /bin/sh "$user"
    if [ -n "${3:-}" ]; then
        echo "$user:$3" | chpasswd
    else
        passwd -d "$user" >/dev/null
    fi
    local home
    home="$(getent passwd "$user" | cut -d: -f6)"
    mkdir -p "$home/.ssh"
    cp "$2" "$home/.ssh/authorized_keys"
    chown -R "$user:" "$home/.ssh"
    chmod 700 "$home/.ssh"
    chmod 600 "$home/.ssh/authorized_keys"
}

up_sftp() {
    local sshd
    sshd="$(command -v sshd || echo /usr/sbin/sshd)"
    if [ "$(id -u)" != 0 ] || [ ! -x "$sshd" ]; then
        log "skipping SFTP: it needs root and OpenSSH's sshd (apt-get install openssh-server)"
        return 0
    fi
    make_keys
    add_user tester "$keys/id_ed25519.pub" dualpal
    add_user keyonly "$keys/keyonly_ed25519.pub"
    mkdir -p /run/sshd
    cat >"$state/sshd_config" <<CONF
Port 2222
ListenAddress $host
HostKey $keys/ssh_host_ed25519_key
PidFile $state/sshd.real.pid
PermitRootLogin no
PubkeyAuthentication yes
PasswordAuthentication yes
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
Subsystem sftp internal-sftp
CONF
    start sftp "$sshd" -D -e -f "$state/sshd_config"
    wait_port sftp 2222
}

up() {
    install_tools
    local py="$venv/bin/python"

    up_sftp

    mkdir -p "$state/ftp-data" "$state/ftp-certs"
    start ftp env FTP_ROOT="$state/ftp-data" FTP_CERTS="$state/ftp-certs" FTP_HOST=$host \
        "$py" -u "$servers/ftp/server.py"

    mkdir -p "$state/webdav-data"
    cat >"$state/wsgidav.yaml" <<CONF
host: $host
port: 8088
provider_mapping:
  "/dav": "$state/webdav-data"
http_authenticator:
  domain_controller: null
  accept_basic: true
  accept_digest: false
  default_to_digest: false
simple_dc:
  user_mapping:
    "*":
      "tester":
        password: "dualpal"
verbose: 2
CONF
    start webdav "$venv/bin/wsgidav" --config "$state/wsgidav.yaml"

    start s3 "$venv/bin/moto_server" -H $host -p 9000

    sed -e 's/0\.0\.0\.0:/127.0.0.1:/' -e 's/"ftp:/"127.0.0.1:/' \
        -e 's/"webdav:/"127.0.0.1:/' -e 's/"s3:/"127.0.0.1:/' \
        "$servers/toxiproxy.json" >"$state/toxiproxy.json"
    start toxiproxy "$bin/toxiproxy-server" -host=$host -config="$state/toxiproxy.json"

    wait_port ftp 2121
    wait_port ftp 2990
    wait_port ftp 2991
    wait_port webdav 8088
    wait_port s3 9000
    "$py" -c "import boto3; boto3.client('s3', endpoint_url='http://$host:9000', aws_access_key_id='test', aws_secret_access_key='test-secret', region_name='us-east-1').create_bucket(Bucket='dualpal-test')"
    wait_port toxiproxy 8474
    log "ready (state and logs in $state)"
}

down() {
    for name in toxiproxy s3 webdav ftp sftp; do
        local pidfile="$state/$name.pid"
        [ -f "$pidfile" ] || continue
        # setsid made each server a process group leader: stop the whole group.
        kill -- "-$(cat "$pidfile")" 2>/dev/null || true
        rm -f "$pidfile"
    done
    log "stopped"
}

drop() {
    # Session processes are "sshd: user@notty" (or "sshd-session: ..." on
    # OpenSSH 9.8 and newer). The listening sshd keeps running.
    local pids
    pids=$(pgrep -f '^sshd(-session)?: [^ ]*@' || true)
    [ -n "$pids" ] && kill -9 $pids
    echo "dropped: ${pids:-none}"
}

env_settings() {
    cp "$keys/id_ed25519" "$state/dualpal_sftp_key"
    chmod 600 "$state/dualpal_sftp_key"
    (cd "$repo" && RUNNER_TEMP="$state" scripts/ci/remote-env.sh --export) |
        grep -v '^export DUALPAL_TEST_SFTP_DROP='
    printf "export DUALPAL_TEST_SFTP_DROP='%s drop'\n" "$repo/scripts/test-servers/native.sh"
}

case "${1:-}" in
up) up ;;
down) down ;;
drop) drop ;;
env) env_settings ;;
*)
    echo "usage: $0 up|down|drop|env" >&2
    exit 2
    ;;
esac
