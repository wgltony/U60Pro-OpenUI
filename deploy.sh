#!/bin/bash
set -euo pipefail
ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$ROOT_DIR"
fail() { echo "$1" >&2; exit 1; }
for tool in cargo rustup zig python3; do
    command -v "$tool" >/dev/null 2>&1 || fail "Missing build prerequisite: $tool"
done
cargo zigbuild --version >/dev/null 2>&1 || fail 'Install cargo-zigbuild first: cargo install --locked cargo-zigbuild'
rustup target list --installed | grep -qx aarch64-unknown-linux-musl || fail 'Run: rustup target add aarch64-unknown-linux-musl'
[ "$(zig version)" = 0.14.1 ] || fail 'Source builds require Zig 0.14.1 on PATH.'
echo 'Building agent…'
cargo zigbuild --locked --release --target aarch64-unknown-linux-musl -p zte-agent
exec python3 "$ROOT_DIR/scripts/deploy-components.py" --agent "${CARGO_TARGET_DIR:-$ROOT_DIR/target}/aarch64-unknown-linux-musl/release/zte-agent" "$@"
