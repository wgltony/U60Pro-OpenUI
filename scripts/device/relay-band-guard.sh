#!/bin/sh
# relay-band-guard: keep the U60Pro radio within its "1 STA + 1 AP" limit while
# the OpenUI Wi-Fi relay (ou-sta0) is up.
#
# When ou-sta0 associates on a band, the driver (gWlanMccToSccSwitchMode=3)
# force-moves the same-band hotspot onto the upstream channel. On a DFS upstream
# channel that switch fails and leaves the hotspot stuck (hostapd and driver
# disagree on the channel), so clients can no longer join. This guard disables
# the same-band hotspot while the relay is associated and re-enables it once
# the relay is gone. The other band's hotspot keeps serving clients.
#
# The paused band is also written as wireless.<radio>.disabled=1 (committed,
# no reload), so the OpenUI web and screen Wi-Fi pages, which read UCI, show it
# as off. Committing is deliberate: the firmware's WPS hook commits wireless on
# hostapd events anyway. The state file lives on /data so a reboot during a
# relay still gets the band back once the relay is gone.
#
# Started at boot by start_zte_agent.sh (deploy-components.py installs it with the
# agent); a second start while one runs exits at once.
#
# Usage: relay-band-guard.sh          watch loop
#        relay-band-guard.sh restore  give back a paused band and exit

STA=${RELAY_STA:-ou-sta0}
AP_2G=wlan0
AP_5G=wlan2
CTRL=/data/vendor/wifi/hostapd
STATE=/data/local/tmp/relay-band-guard.disabled   # the AP this guard disabled, if any
BOOT_GRACE=180   # seconds after boot to wait for the relay before giving a band back
PIDFILE=/var/run/relay-band-guard.pid

LOG=/data/local/tmp/openui-logs/relay-guard.log   # shown on the web page's log card
# The firmware's syslogd keeps only local0, so the guard writes its own file too
# (512 KiB, the previous one kept as .1).
log() {
	logger -t relay-band-guard "$*"
	mkdir -p "${LOG%/*}" && chmod 700 "${LOG%/*}"
	[ "$(wc -c < "$LOG" 2>/dev/null || echo 0)" -lt 524288 ] || mv -f "$LOG" "$LOG.1"
	echo "$(date -u '+%Y-%m-%d %H:%M:%S') $*" >> "$LOG"
}
hapd() { hostapd_cli -p "$CTRL" -i "$1" "$2" 2>/dev/null; }
ap_state() { hapd "$1" status | sed -n 's/^state=//p'; }
radio_of() { [ "$1" = "$AP_2G" ] && echo wifi0 || echo wifi1; }
uptime_s() { cut -d. -f1 /proc/uptime; }

# Mark the paused band as off in UCI for the UI, without a Wi-Fi reload.
uci_pause() {
	key="wireless.$(radio_of "$1").disabled"
	[ "$(uci -q get "$key")" = 1 ] && return 0
	uci -q set "$key=1" && uci -q commit wireless
}

uci_resume() {
	key="wireless.$(radio_of "$1").disabled"
	[ "$(uci -q get "$key")" = 1 ] || return 0
	uci -q set "$key=0" && uci -q commit wireless
}

ap_disable() {
	if [ "$(ap_state "$1")" = ENABLED ]; then
		hapd "$1" disable >/dev/null || return 0
		echo "$1" > "$STATE"
		log "relay on $2, disabled $1"
	fi
	# Re-apply after the band was switched back on from the UI while the relay is up.
	[ -s "$STATE" ] && uci_pause "$1"
}

ap_restore() {
	[ -s "$STATE" ] || return 0
	ap=$(cat "$STATE")
	rm -f "$STATE"
	uci_resume "$ap"
	if [ ! -d "/sys/class/net/$ap" ]; then
		# A Wi-Fi reload while paused removed the interface; bring it back from UCI.
		ubus call zwrt_wlan reload >/dev/null 2>&1
		log "$1, re-enabled $ap with a Wi-Fi reload"
	elif [ "$(ap_state "$ap")" != ENABLED ]; then
		hapd "$ap" enable >/dev/null && log "$1, re-enabled $ap"
	fi
}

sta_freq() {
	iw dev "$STA" link 2>/dev/null | sed -n 's/^[[:space:]]*freq: *\([0-9]*\).*/\1/p'
}

if [ "$1" = restore ]; then
	ap_restore "guard stopped"
	exit 0
fi

# One watcher only. A PID or a command substring is not ownership: check the
# complete interpreter/script argv, executable and Linux start time. Keep a
# token for transaction recovery and never replace a live unidentified owner.
identity() {
	start=$(sed 's/^.*) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}')
	exe=$(readlink "/proc/$1/exe") || return 1
	hash=$(sha256sum "/proc/$1/cmdline" | awk '{print $1}')
	case "$exe" in /bin/busybox|/bin/ash|/bin/sh) ;; *) return 1 ;; esac
	argv_hash() { printf '%s\000' "$@" | sha256sum | awk '{print $1}'; }
	if [ "$hash" != "$(argv_hash /bin/sh /data/bin/relay-band-guard.sh)" ] &&
	   [ "$hash" != "$(argv_hash /bin/ash /data/bin/relay-band-guard.sh)" ] &&
	   [ "$hash" != "$(argv_hash /data/bin/relay-band-guard.sh)" ]; then return 1; fi
	case "$start" in ''|*[!0-9]*) return 1 ;; esac
	[ "$(awk '/^Uid:/ {print $2 ":" $3 ":" $4 ":" $5}' "/proc/$1/status")" = 0:0:0:0 ] || return 1
	inode=$(stat -Lc '%d:%i' "/proc/$1/exe") || return 1
	printf '%s|%s|%s|%s|%s\n' "$1" "$start" "$exe" "$inode" "$(sha256sum "/proc/$1/cmdline" | awk '{print $1}')"
}
record=$(cat "$PIDFILE" 2>/dev/null)
pid=${record%%|*}
if [ -n "$pid" ]; then
	case "$pid" in *[!0-9]*) echo 'Invalid guard pidfile' >&2; exit 1 ;; esac
	if [ -d "/proc/$pid" ]; then
		live=$(identity "$pid") || { echo 'Unidentified guard pidfile owner' >&2; exit 1; }
		case "$record" in *'|'*) [ "$record" = "$live" ] || { echo 'Guard PID identity changed' >&2; exit 1; } ;; esac
		exit 0
	fi
fi
lock=/var/run/relay-band-guard.lock
# Only discard a stale lock when its recorded process is demonstrably gone.
if [ -n "$pid" ] && [ ! -d "/proc/$pid" ]; then rmdir "$lock" 2>/dev/null || true; fi
mkdir "$lock" 2>/dev/null || { echo 'Guard startup is already locked' >&2; exit 1; }
owner=$(identity $$) || { rmdir "$lock"; exit 1; }
printf '%s\n' "$owner" > "$PIDFILE"
cleanup() {
	if [ "$(cat "$PIDFILE" 2>/dev/null)" = "$owner" ]; then rm -f "$PIDFILE"; rmdir "$lock" 2>/dev/null || true; fi
}
# Transaction stop must NOT restore an AP or reload Wi-Fi. Its saved UCI/marker
# is restored separately; explicit `restore` above retains its old meaning.
trap cleanup EXIT
trap 'exit 0' HUP INT TERM
log "guard started"

while :; do
	if [ -d "/sys/class/net/$STA" ]; then
		freq=$(sta_freq)
		if [ -n "$freq" ]; then
			if [ "$freq" -lt 3000 ]; then want=$AP_2G; band=2.4G; else want=$AP_5G; band=5G; fi
			# Upstream moved to the other band: give back the AP disabled before.
			if [ -s "$STATE" ] && [ "$(cat "$STATE")" != "$want" ]; then
				ap_restore "relay moved to $band"
			fi
			ap_disable "$want" "$band ($freq MHz)"
		fi
		# Not associated yet / between networks: keep whatever is disabled.
	elif [ "$(uptime_s)" -ge "$BOOT_GRACE" ]; then
		# Right after boot the relay may not be up yet; don't give the band back early.
		ap_restore "relay off"
	fi
	sleep 1
done
