#!/usr/bin/env bash
# Run one registration by default. Pass --dry-run to inspect configuration offline.
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cd "$ROOT"
if [[ -x "$ROOT/bin/sentinel-rs" ]]; then
  BINARY="$ROOT/bin/sentinel-rs"
else
  BINARY="$ROOT/target/release/sentinel-rs"
fi
if [[ ! -x "$BINARY" ]]; then
  printf 'Missing Linux executable. Run ./build.sh --bin sentinel-rs first.\n' >&2
  exit 1
fi
umask 077
exec "$BINARY" register --live --2fa "$@"
