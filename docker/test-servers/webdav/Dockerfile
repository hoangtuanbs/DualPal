# A throwaway WebDAV server for testing Dual Pal (WsgiDAV), at
# http://127.0.0.1:8088/dav with user "tester", password "dualpal".
FROM python:3.12-alpine

RUN pip install --no-cache-dir "wsgidav==4.*" cheroot
RUN mkdir -p /data && printf '%s\n' \
  'host: 0.0.0.0' 'port: 8088' \
  'provider_mapping:' '  "/dav": "/data"' \
  'http_authenticator:' '  domain_controller: null' '  accept_basic: true' '  accept_digest: false' '  default_to_digest: false' \
  'simple_dc:' '  user_mapping:' '    "*":' '      "tester":' '        password: "dualpal"' \
  'verbose: 2' > /wsgidav.yaml
EXPOSE 8088
CMD ["wsgidav", "--config", "/wsgidav.yaml"]
