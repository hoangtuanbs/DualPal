"""Plain FTP, explicit FTPS and implicit FTPS on one folder, for tests."""

import os
import threading

from OpenSSL import crypto
from pyftpdlib.authorizers import DummyAuthorizer
from pyftpdlib.handlers import FTPHandler, TLS_FTPHandler
from pyftpdlib.ioloop import IOLoop
from pyftpdlib.servers import FTPServer

ROOT = "/data"
CERTS = "/certs"
os.makedirs(ROOT, exist_ok=True)
os.makedirs(CERTS, exist_ok=True)
cert, key = f"{CERTS}/cert.pem", f"{CERTS}/key.pem"
if not os.path.exists(cert):
    k = crypto.PKey()
    k.generate_key(crypto.TYPE_RSA, 2048)
    c = crypto.X509()
    c.set_version(2)  # X.509 v3; TLS libraries refuse v1 certificates
    c.get_subject().CN = "dualpal-test-ftp"
    c.set_serial_number(1)
    c.gmtime_adj_notBefore(0)
    c.gmtime_adj_notAfter(10 * 365 * 24 * 3600)
    c.set_issuer(c.get_subject())
    c.set_pubkey(k)
    c.sign(k, "sha256")
    open(cert, "wb").write(crypto.dump_certificate(crypto.FILETYPE_PEM, c))
    open(key, "wb").write(crypto.dump_privatekey(crypto.FILETYPE_PEM, k))

users = DummyAuthorizer()
users.add_user("tester", "dualpal", ROOT, perm="elradfmwMT")


class Plain(FTPHandler):
    authorizer = users
    passive_ports = range(30000, 30010)


class Explicit(TLS_FTPHandler):
    authorizer = users
    certfile, keyfile = cert, key
    tls_control_required = True
    tls_data_required = True
    passive_ports = range(30010, 30020)


class Implicit(TLS_FTPHandler):
    """TLS from the first byte, with protected transfers."""

    authorizer = users
    certfile, keyfile = cert, key
    passive_ports = range(30020, 30030)

    def handle(self):
        self._pbsz = True
        self._prot = True
        self.secure_connection(self.ssl_context)
        super().handle()


# Each server needs its own event loop to run in its own thread.
servers = [
    FTPServer(("0.0.0.0", port), h, ioloop=IOLoop())
    for port, h in ((2121, Plain), (2990, Explicit), (2991, Implicit))
]
for s in servers[1:]:
    threading.Thread(target=s.serve_forever, daemon=True).start()
print("ftp ready", flush=True)
servers[0].serve_forever()
