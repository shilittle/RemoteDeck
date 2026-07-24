FROM alpine:3.22

COPY codex /usr/local/bin/codex

RUN apk add --no-cache bash btop openssh procps python3 tmux vim \
    && chmod 0755 /usr/local/bin/codex \
    && adduser -D -s /bin/bash remotedeck \
    && echo 'remotedeck:remotedeck-test-only' | chpasswd \
    && ssh-keygen -A \
    && mkdir -p /run/sshd \
    && printf '%s\n' \
      'Port 22' \
      'ListenAddress 0.0.0.0' \
      'PasswordAuthentication yes' \
      'KbdInteractiveAuthentication yes' \
      'PermitRootLogin no' \
      'UsePAM no' \
      'AllowTcpForwarding yes' \
      'GatewayPorts clientspecified' \
      'Subsystem sftp internal-sftp' \
      > /etc/ssh/sshd_config

EXPOSE 22 17890
CMD ["/bin/bash", "-lc", "python3 -m http.server 18080 --bind 127.0.0.1 >/tmp/remotedeck-http.log 2>&1 & exec /usr/sbin/sshd -D -e"]
