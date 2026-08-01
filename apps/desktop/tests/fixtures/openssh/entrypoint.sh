#!/bin/sh
set -eu

if [ ! -s /fixture/authorized_keys ]; then
  printf '%s\n' 'fixture authorized_keys is missing or empty' >&2
  exit 64
fi

install -d -m 0755 /run/sshd
install -d -m 0700 -o remotedeck -g remotedeck /home/remotedeck/.ssh
install -m 0600 -o remotedeck -g remotedeck /fixture/authorized_keys /home/remotedeck/.ssh/authorized_keys

# Host keys are deliberately ephemeral and never committed to the repository.
rm -f /etc/ssh/ssh_host_*_key /etc/ssh/ssh_host_*_key.pub
ssh-keygen -q -t ed25519 -N '' -f /etc/ssh/ssh_host_ed25519_key

fixture_name="${REMOTEDECK_FIXTURE_NAME:-unknown}"
install -d -m 0755 /tmp/remotedeck-fixture-www
printf 'RemoteDeck OpenSSH fixture: %s\n' "$fixture_name" > /tmp/remotedeck-fixture-www/index.html
python3 -m http.server 18080 --bind 0.0.0.0 --directory /tmp/remotedeck-fixture-www \
  >/tmp/remotedeck-fixture-http.log 2>&1 &

exec /usr/sbin/sshd -D -e
