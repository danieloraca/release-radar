#!/usr/bin/env bash
# Run as your normal Pi user. This script does not install/enable the service.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $(uname -s) != Linux ]]; then
  echo "Run make deploy on the Raspberry Pi's Linux checkout." >&2
  exit 1
fi
if [[ $(id -u) == 0 ]]; then
  echo "Run make deploy as your normal Pi user, not with sudo." >&2
  exit 1
fi
for tool in git cargo curl sudo systemctl; do
  command -v "$tool" >/dev/null || { echo "Required command missing: $tool" >&2; exit 1; }
done
if [[ -n $(git status --porcelain) ]]; then
  echo "Checkout has local changes. Commit or stash them before deploying." >&2
  exit 1
fi

service_name=release-radar.service
health_url=${RR_HEALTH_URL:-http://127.0.0.1:9977/healthz}

git pull --ff-only
cargo test --locked
cargo build --locked --release
./scripts/install-binary.sh
sudo systemctl restart "$service_name"
systemctl is-active "$service_name"

for attempt in {1..10}; do
  if curl --fail --silent --show-error --max-time 2 "$health_url" >/dev/null 2>&1; then
    echo "Release Radar is responding at $health_url"
    exit 0
  fi
  if [[ $attempt -lt 10 ]]; then sleep 1; fi
done

echo "$service_name restarted, but $health_url did not respond." >&2
echo "Inspect: journalctl -u $service_name -n 50 --no-pager" >&2
exit 1
