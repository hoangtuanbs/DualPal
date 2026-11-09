# A throwaway SSH/SFTP server for Dual Pal's tests. Not for production use.
# Start it with compose.yaml in the folder above; see README.md there.
FROM alpine:3.22

RUN apk add --no-cache openssh-server openssh-sftp-server openssh-keygen rsync coreutils procps \
 && ssh-keygen -A

COPY entrypoint.sh /usr/local/bin/entrypoint.sh
COPY drop-connections /usr/local/bin/drop-connections
RUN chmod +x /usr/local/bin/entrypoint.sh /usr/local/bin/drop-connections

# The client key is created here on first start if there is none, and the
# host key is kept here so that its fingerprint stays the same.
VOLUME /keys
EXPOSE 22
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
