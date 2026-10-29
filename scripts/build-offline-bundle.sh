#!/bin/bash
# Prepare a legacy agent/dashboard development bundle, not a supported GUI release.
set -euo pipefail
umask 077
ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
DESTINATION=${1:-"$ROOT_DIR/dist/install-bundle"}
[ ! -e "$DESTINATION" ] || { echo 'Choose a new output directory; an existing bundle will not be overwritten.' >&2; exit 1; }
fail() { echo "$1" >&2; exit 1; }
for tool in cargo rustup zig npm python3 tar; do
    command -v "$tool" >/dev/null 2>&1 || fail "Missing build prerequisite: $tool"
done
cargo zigbuild --version >/dev/null 2>&1 || fail 'Install cargo-zigbuild first: cargo install --locked cargo-zigbuild'
rustup target list --installed | grep -qx aarch64-unknown-linux-musl || fail 'Run: rustup target add aarch64-unknown-linux-musl'
[ "$(zig version)" = 0.14.1 ] || fail 'Source builds require Zig 0.14.1 on PATH.'
mkdir -p "$(dirname "$DESTINATION")"
WORK=$(mktemp -d "$(dirname "$DESTINATION")/.bundle-XXXXXX")
trap 'rm -rf "$WORK"' EXIT
cd "$ROOT_DIR"
cargo zigbuild --locked --release --target aarch64-unknown-linux-musl -p zte-agent
(cd web-app && npm ci && npm run build)
python3 scripts/deploy-components.py --prepare-only --bundle-output "$WORK"
cp "${CARGO_TARGET_DIR:-target}/aarch64-unknown-linux-musl/release/zte-agent" "$WORK/zte-agent"
tar czf "$WORK/dashboard-dist.tar.gz" -C web-app/dist .
python3 - "$WORK" <<'PY'
import hashlib,json,sys
from pathlib import Path
root=Path(sys.argv[1])
checksums={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in ['zte-agent','dashboard-dist.tar.gz']}
(root/'sha256sums.txt').write_text(''.join(f'{checksum}  {name}\n' for name,checksum in checksums.items()))
files={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in ['zte-agent','dashboard-dist.tar.gz','sha256sums.txt','dropbear.ipk','uhttpd.ipk']}
(root/'bundle.json').write_text(json.dumps({'format_version':1,'release':'local working tree','files':files},indent=2))
PY
mv "$WORK" "$DESTINATION"
echo "Offline bundle ready: $DESTINATION"
