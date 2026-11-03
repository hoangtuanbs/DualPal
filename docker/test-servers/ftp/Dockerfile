# A throwaway FTP server for testing Dual Pal (pyftpdlib), in three flavors:
#   2121  plain FTP
#   2990  FTPS, explicit (AUTH TLS)
#   2991  FTPS, implicit (TLS from the first byte)
# User "tester", password "dualpal". The certificate is self-signed, made on
# first start and kept in /certs.
FROM python:3.12-alpine

RUN pip install --no-cache-dir "pyftpdlib==2.*" "pyopenssl==25.*"
COPY server.py /server.py
EXPOSE 2121 2990 2991 30000-30029
CMD ["python", "-u", "/server.py"]
