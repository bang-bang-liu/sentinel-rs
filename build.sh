#!/usr/bin/env bash
# Linux build entry point; shared tools may be selected with CARGO_HOME/RUSTUP_HOME.
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cd "$ROOT"

if [[ -d /opt/sentinel-build-tools/cargo/bin && -z ${CARGO_HOME:-} ]]; then
  export CARGO_HOME=/opt/sentinel-build-tools/cargo
fi
if [[ -d /opt/sentinel-build-tools/rustup && -z ${RUSTUP_HOME:-} ]]; then
  export RUSTUP_HOME=/opt/sentinel-build-tools/rustup
fi
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-1}
export CMAKE_GENERATOR=${CMAKE_GENERATOR:-Ninja}
if [[ -z ${LIBCLANG_PATH:-} ]] && command -v llvm-config >/dev/null 2>&1; then
  export LIBCLANG_PATH
  LIBCLANG_PATH=$(llvm-config --libdir)
fi
for tool in cargo cc c++ cmake ninja git; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'Missing build dependency: %s\n' "$tool" >&2
    exit 1
  fi
done

ACTION=build
if [[ ${1:-} == --test ]]; then ACTION=test; shift; fi
exec cargo "$ACTION" --release --locked "$@"
