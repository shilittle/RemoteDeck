#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
desktop_dir="$(cd -- "${script_dir}/.." && pwd)"
fixture_dir="${desktop_dir}/tests/fixtures/openssh"
key_dir="$(mktemp -d -t remotedeck-openssh-key.XXXXXXXXXX)"

export REMOTEDECK_DIRECT_PORT="${REMOTEDECK_DIRECT_PORT:-22221}"
export REMOTEDECK_JUMP_PORT="${REMOTEDECK_JUMP_PORT:-22222}"
export REMOTEDECK_REMOTE_FORWARD_PORT="${REMOTEDECK_REMOTE_FORWARD_PORT:-23080}"
export REMOTEDECK_INTEGRATION_IDENTITY="${key_dir}/id_ed25519"
export REMOTEDECK_FIXTURE_AUTHORIZED_KEYS="${key_dir}/id_ed25519.pub"
run_id="${GITHUB_RUN_ID:-local-$$}"
export COMPOSE_PROJECT_NAME="remotedeck-openssh-${run_id}-${GITHUB_RUN_ATTEMPT:-1}"

cleanup() {
  local status=$?
  trap - EXIT
  set +e
  if [ "$status" -ne 0 ]; then
    docker compose --file "${fixture_dir}/compose.yml" logs --no-color --tail 200
  fi
  docker compose --file "${fixture_dir}/compose.yml" down --volumes --remove-orphans
  rm -f -- "${key_dir}/id_ed25519" "${key_dir}/id_ed25519.pub"
  rmdir -- "${key_dir}"
  exit "$status"
}
trap cleanup EXIT

for required_command in cargo docker ssh-keygen; do
  if ! command -v "${required_command}" >/dev/null 2>&1; then
    printf 'required command is unavailable: %s\n' "${required_command}" >&2
    exit 127
  fi
done
docker compose version >/dev/null

ssh-keygen -q -t ed25519 -N '' -f "${REMOTEDECK_INTEGRATION_IDENTITY}"
chmod 0600 "${REMOTEDECK_INTEGRATION_IDENTITY}"
chmod 0644 "${REMOTEDECK_FIXTURE_AUTHORIZED_KEYS}"

docker compose --file "${fixture_dir}/compose.yml" build
docker compose --file "${fixture_dir}/compose.yml" up --detach --wait --wait-timeout 120
docker compose --file "${fixture_dir}/compose.yml" ps

cd -- "${desktop_dir}"
cargo test \
  --manifest-path src-tauri/Cargo.toml \
  --locked \
  --no-default-features \
  --features openssh-integration \
  --lib \
  openssh_integration \
  -- \
  --ignored \
  --test-threads=1
