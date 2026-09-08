ARG BASE_IMAGE=ubuntu:24.04@sha256:52df9b1ee71626e0088f7d400d5c6b5f7bb916f8f0c82b474289a4ece6cf3faf
FROM ${BASE_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive

# The base image is digest-pinned. Package indexes intentionally receive current
# Noble security fixes when CI rebuilds this disposable test-only image.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        btop \
        ca-certificates \
        openssh-client \
        openssh-server \
        python3 \
        tmux \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell /bin/bash remotedeck \
    && printf '%s:%s\n' remotedeck fixture-password-disabled-by-sshd | chpasswd

COPY sshd_config /etc/ssh/sshd_config
COPY entrypoint.sh /usr/local/bin/remotedeck-fixture-entrypoint

RUN chmod 0755 /usr/local/bin/remotedeck-fixture-entrypoint

EXPOSE 22

HEALTHCHECK --interval=2s --timeout=2s --start-period=5s --retries=30 \
  CMD ssh-keyscan -T 1 -p 22 127.0.0.1 >/dev/null 2>&1 || exit 1

ENTRYPOINT ["/usr/local/bin/remotedeck-fixture-entrypoint"]
