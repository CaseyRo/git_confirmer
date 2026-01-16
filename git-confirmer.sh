#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="${SCRIPT_DIR}/target/release/git_confirmer"

if [[ $# -eq 0 ]]; then
  ARGS=(--root "$HOME/dev")
else
  ARGS=("$@")
fi

if [[ ! -x "$BIN" || "${SCRIPT_DIR}/Cargo.toml" -nt "$BIN" ]]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo not found. Install Rust and build once:" >&2
    echo "  cargo build --release --manifest-path ${SCRIPT_DIR}/Cargo.toml" >&2
    exit 1
  fi

  if find "${SCRIPT_DIR}/src" -type f -newer "$BIN" -print -quit | grep -q .; then
    cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"
  elif [[ "${SCRIPT_DIR}/theme.conf" -nt "$BIN" ]]; then
    cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"
  elif [[ "${SCRIPT_DIR}/config.toml" -nt "$BIN" ]]; then
    cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"
  elif [[ ! -x "$BIN" ]]; then
    cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml"
  fi
fi

if [[ ! -x "$BIN" ]]; then
  echo "build failed: ${BIN} not found" >&2
  exit 1
fi

"$BIN" "${ARGS[@]}"
