#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust first: https://www.rust-lang.org/tools/install" >&2
  exit 1
fi

cargo install --path "${SCRIPT_DIR}"

echo "Installed git_confirmer to ~/.cargo/bin." 
if ! command -v git_confirmer >/dev/null 2>&1; then
  echo "Ensure ~/.cargo/bin is on your PATH." 
  echo "Add this to your shell profile:" 
  echo "  export PATH=\"$HOME/.cargo/bin:$PATH\""
fi
