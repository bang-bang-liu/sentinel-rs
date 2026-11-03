#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
for tool in cargo cc cmake; do
  command -v "$tool" >/dev/null || { printf 'Missing build tool: %s\n' "$tool" >&2; exit 1; }
done
# wreq/BoringSSL also needs libclang. This workspace has its own lock and target.
cargo test --release --locked
cargo build --release --locked -p sentinel-relogin
