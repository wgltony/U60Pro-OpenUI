#!/bin/bash
# One-command install for the ZTE U60 Pro (MU5250, CN B31): unlocks ADB when the
# device is still locked, then installs the agent, web dashboard, touchscreen UI,
# WireGuard tool and (optionally) Tailscale in one verified deployment that rolls
# back on failure. Guide: docs/INSTALL.md (中文: docs/INSTALL.zh.md).
set -euo pipefail
umask 077
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$ROOT"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
info() { echo -e "${CYAN}[*]${NC} $1"; }
ok()   { echo -e "${GREEN}[+]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
fail() { echo -e "${RED}[-]${NC} $1" >&2; exit 1; }

usage() {
	cat <<'EOF'
Usage: bash install.sh [options]

  --update             Update an existing install: keep its password and PIN,
                       skip unchanged parts, no unlock and no SSH hardening
  --with-tailscale     Also install Tailscale (official static build, ~60 MB)
  --skip-screen        Leave the touchscreen program as it is
  --from-source        Build agent, dashboard and screen here (Rust, Zig, Node)
                       instead of downloading this repository's release
  --release TAG        Release to download (default: the latest)
  --repo OWNER/NAME    GitHub repository with the releases (default: origin)
  --gateway ADDRESS    Router address (default: 192.168.0.1)
  --adb-serial SERIAL  ADB device to use when several are connected
  --dry-run            Check the computer, the files and the device; write nothing
  -h, --help           This help

Non-interactive: ZTE_AGENT_PASSWORD, ZTE_AGENT_PIN (six digits, optional),
ROUTER_PW and ZTE_BACKUP_SUFFIX (only for the unlock of a locked device).
EOF
}

UPDATE=0 TAILSCALE=0 SCREEN=1 SOURCE=0 DRY=0 TAG='' REPO='' SERIAL=''
GATEWAY="${ZTE_GATEWAY:-192.168.0.1}"
while [ $# -gt 0 ]; do
	case "$1" in
		--update) UPDATE=1 ;;
		--with-tailscale) TAILSCALE=1 ;;
		--skip-screen) SCREEN=0 ;;
		--from-source) SOURCE=1 ;;
		--release) TAG=${2:?--release needs a tag}; shift ;;
		--repo) REPO=${2:?--repo needs OWNER/NAME}; shift ;;
		--gateway) GATEWAY=${2:?--gateway needs an address}; shift ;;
		--adb-serial) SERIAL=${2:?--adb-serial needs a serial}; shift ;;
		--dry-run) DRY=1 ;;
		-h|--help) usage; exit 0 ;;
		*) usage >&2; fail "Unknown option: $1" ;;
	esac
	shift
done

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# ── 1. This computer ───────────────────────────────────────────────────────────
info 'Checking this computer…'
NEED=(python3 openssl adb curl ssh ssh-keygen tar)
[ "$SOURCE" = 1 ] && NEED+=(cargo zig node npm)
MISSING=()
for tool in "${NEED[@]}"; do
	if command -v "$tool" >/dev/null 2>&1; then echo -e "  ${GREEN}✓${NC} $tool"; else echo -e "  ${RED}✗${NC} $tool"; MISSING+=("$tool"); fi
done
if [ ${#MISSING[@]} -gt 0 ]; then
	echo
	echo 'Install the missing tools, then run this again:'
	echo '  macOS:  brew install python openssl android-platform-tools'
	echo '  Debian/Ubuntu:  sudo apt-get install python3 openssl adb curl openssh-client'
	[ "$SOURCE" = 1 ] && echo '  Building: https://rustup.rs, Zig 0.14.1 (https://ziglang.org/download/), Node.js 22'
	fail "Missing: ${MISSING[*]}"
fi
if [ "$SOURCE" = 1 ] && command -v rustup >/dev/null && ! rustup target list --installed | grep -qx aarch64-unknown-linux-musl; then
	fail 'Run: rustup target add aarch64-unknown-linux-musl'
fi

# ── 2. Agent password ──────────────────────────────────────────────────────────
# The password protects the web dashboard and the agent API (port 9090).
if [ "$UPDATE" = 0 ]; then
	if [ -z "${ZTE_AGENT_PASSWORD:-}" ]; then
		read -rsp 'Choose a password for the web dashboard: ' ZTE_AGENT_PASSWORD; echo
		read -rsp 'Repeat it: ' again; echo
		[ "$ZTE_AGENT_PASSWORD" = "$again" ] || fail 'The passwords do not match.'
	fi
	export ZTE_AGENT_PASSWORD ZTE_AGENT_PIN="${ZTE_AGENT_PIN:-}"
	# Same checks the deployment applies, before anything is downloaded or written.
	python3 -c 'import importlib.util,os; s=importlib.util.spec_from_file_location("startup","scripts/render-agent-startup.py"); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); m.render(os.environ["ZTE_AGENT_PASSWORD"], os.environ.get("ZTE_AGENT_PIN",""))' \
		|| fail 'The password must not be empty; a PIN must be exactly six digits.'
fi

# ── 3. Files to install (all checked before the device is touched) ─────────────
if [ "$SOURCE" = 1 ]; then
	info 'Building the agent…'
	cargo build --locked --release --target aarch64-unknown-linux-musl -p zte-agent
	cp "${CARGO_TARGET_DIR:-target}/aarch64-unknown-linux-musl/release/zte-agent" "$WORK/zte-agent"
	info 'Building the web dashboard…'
	(cd web-app && npm ci --no-audit --no-fund && npm run build)
	tar czf "$WORK/dashboard-dist.tar.gz" -C web-app/dist .
	if [ "$SCREEN" = 1 ]; then
		info 'Building the touchscreen UI…'
		ZIG=${ZIG:-zig} sh screen/build.sh
		cp build/screen/openui-screen "$WORK/openui-screen"
	fi
else
	if [ -z "$REPO" ]; then
		url=$(git config --get remote.origin.url 2>/dev/null || true)
		url=${url%/}; url=${url%.git}
		case "$url" in
			https://github.com/*/*) REPO=${url#https://github.com/} ;;
			git@github.com:*/*) REPO=${url#git@github.com:} ;;
		esac
		[ -n "$REPO" ] || fail 'Cannot tell which GitHub repository to download from; pass --repo OWNER/NAME.'
	fi
	[ -n "$TAG" ] || TAG=$(python3 scripts/download-release.py --repo "$REPO" --latest) \
		|| fail "No release found in $REPO. Use --from-source to build instead."
	info "Downloading $REPO $TAG…"
	FILES=(zte-agent dashboard-dist.tar.gz)
	[ "$SCREEN" = 1 ] && FILES+=(openui-screen)
	python3 scripts/download-release.py --repo "$REPO" --tag "$TAG" --output "$WORK" "${FILES[@]}"
fi
info 'Preparing the WireGuard tool…'
python3 scripts/prepare-wireguard.py --output "$WORK/wg"
if [ "$TAILSCALE" = 1 ]; then
	info 'Preparing Tailscale…'
	python3 scripts/prepare-tailscale.py --output "$WORK/tailscale"
fi
if [ "$UPDATE" = 0 ]; then
	# Pinned Dropbear (SSH on port 2222) and the dashboard's web server.
	python3 scripts/deploy-components.py --prepare-only --bundle-output "$WORK/packages"
fi

# ── 4. Reach the device (unlock only when it is still locked) ──────────────────
EXTRA=()
[ -n "$SERIAL" ] && EXTRA+=(--adb-serial "$SERIAL")
SSH=(ssh -p 2222 -o BatchMode=yes -o StrictHostKeyChecking=accept-new
	-o "UserKnownHostsFile=$HOME/.ssh/known_hosts.d/zte" -o ConnectTimeout=5 "root@$GATEWAY")
mkdir -p "$HOME/.ssh/known_hosts.d"
if "${SSH[@]}" true >/dev/null 2>&1; then
	ok 'SSH (port 2222) works; the device identity is checked before any write.'
elif adb devices 2>/dev/null | grep -q 'device$'; then
	ok 'ADB works; the device identity is checked before any write.'
	[ -n "$SERIAL" ] || EXTRA+=(--auto-adb)
elif [ "$UPDATE" = 1 ]; then
	fail 'Neither SSH (port 2222) nor ADB reaches the device. Connect it over USB or Wi-Fi and try again.'
else
	warn 'The device is locked (no ADB, no SSH): ADB will be enabled through a patched settings backup.'
	echo '     This needs the router admin password and the backup-key suffix; read docs/INSTALL.md first.'
	ROUTER_PW=${ROUTER_PW:-}
	[ -n "$ROUTER_PW" ] || { read -rsp 'Router admin password (the 192.168.0.1 login): ' ROUTER_PW; echo; }
	SUFFIX=${ZTE_BACKUP_SUFFIX:-}
	[ -n "$SUFFIX" ] || { read -rsp 'Backup-key suffix: ' SUFFIX; echo; }
	[ -n "$ROUTER_PW" ] && [ -n "$SUFFIX" ] || fail 'Both the router password and the suffix are needed to unlock.'
	ROUTER_PW="$ROUTER_PW" ZTE_BACKUP_SUFFIX="$SUFFIX" python3 scripts/zunlock.py --gw "$GATEWAY" --dry-run --keep-work "$WORK/unlock"
	if [ "$DRY" = 1 ]; then
		ok 'Unlock dry run passed. Nothing was written; the device checks need ADB, so they did not run.'
		exit 0
	fi
	ROUTER_PW="$ROUTER_PW" ZTE_BACKUP_SUFFIX="$SUFFIX" python3 scripts/zunlock.py --gw "$GATEWAY" --keep-work "$WORK/unlock"
	info 'The device restores and reboots; waiting up to three minutes for ADB…'
	python3 -c 'import subprocess; subprocess.run(["adb", "wait-for-device"], timeout=180, check=True)'
	EXTRA+=(--auto-adb --identity-file "$WORK/unlock/identity.json")
fi

# ── 5. Install (one transaction; a failure restores the previous state) ────────
ARGS=(--gateway "$GATEWAY" --agent "$WORK/zte-agent" --dashboard "$WORK/dashboard-dist.tar.gz"
	--wireguard-tools "$WORK/wg")
[ "$SCREEN" = 1 ] && ARGS+=(--screen "$WORK/openui-screen")
[ "$TAILSCALE" = 1 ] && ARGS+=(--tailscale "$WORK/tailscale")
if [ "$UPDATE" = 1 ]; then
	ARGS+=(--keep-credentials --skip-unchanged)
else
	ARGS+=(--harden --bundle "$WORK/packages")
fi
[ "$DRY" = 1 ] && ARGS+=(--dry-run)
info 'Installing…'
if ! python3 scripts/deploy-components.py "${ARGS[@]}" "${EXTRA[@]}"; then
	echo
	warn 'A failed deployment restores the previous state by itself (if it could not, the recovery command is printed above).'
	echo '     "The touchscreen is running OpenUI": tap "Stock UI" (原厂界面) on the touchscreen,'
	echo '     then run this again (or add --skip-screen to keep the current screen program).'
	fail 'Installation stopped.'
fi
[ "$DRY" = 1 ] && { ok 'Dry run passed: nothing was written to the device.'; exit 0; }

# ── 6. Touchscreen check (the web "Open DevUI screen" button waits for it) ─────
KEY="$HOME/.ssh/id_ed25519"
if [ "$SCREEN" = 0 ]; then
	:
elif "${SSH[@]}" -i "$KEY" true >/dev/null 2>&1; then
	echo
	echo 'The touchscreen UI must pass a supervised check once (about three minutes): it takes'
	echo 'over the screen and tests recovery from idle, crash and hang. Keep the router in view.'
	read -rp 'Run it now? [Y/n] ' answer
	case "$answer" in
		[nN]*) warn "Later: python3 scripts/verify-screen.py --gateway $GATEWAY --ssh-key $KEY --if-needed" ;;
		*) python3 scripts/verify-screen.py --gateway "$GATEWAY" --ssh-key "$KEY" --if-needed ;;
	esac
else
	warn 'Touchscreen check skipped: it needs SSH on port 2222 (see docs/INSTALL.md, "Touchscreen").'
fi

echo
ok 'Done.'
echo "  Web dashboard:  http://$GATEWAY:8080  (the password you chose)"
[ "$UPDATE" = 0 ] && echo "  SSH:            ssh -p 2222 root@$GATEWAY  (key $KEY)"
echo '  Touchscreen:    web dashboard > DevUI > Open DevUI screen (or enable the power-button shortcut there)'
echo '  Next steps:     docs/INSTALL.md, "After installing"'
if [ "$UPDATE" = 0 ]; then
	warn 'Reboot the router when convenient: USB returns to normal tethering and ADB turns off (use SSH).'
fi
exit 0
