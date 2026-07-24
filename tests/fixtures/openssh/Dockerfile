FROM alpine:3.22

RUN apk add --no-cache bash openssh python3 \
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
      'GatewayPorts no' \
      'Subsystem sftp internal-sftp' \
      > /etc/ssh/sshd_config

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
