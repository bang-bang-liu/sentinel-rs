#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
for command in cargo cc cmake; do
  command -v "$command" >/dev/null || { printf 'Missing build tool: %s\n' "$command" >&2; exit 1; }
done
# BoringSSL/bindgen also requires libclang (for Debian/Ubuntu: libclang-dev).
cargo build --release --locked
cargo test --release --locked
