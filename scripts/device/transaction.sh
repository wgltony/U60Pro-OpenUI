#!/bin/sh
# Durable deployment snapshots and an owned-service journal; never run at boot.
set -eu
umask 077
root=${U60_TEST_ROOT:-}
base="$root/data/local/tmp/open-u60-transactions"
proc="$root/proc"
action=${1:?action required}
id=${2:?transaction id required}
identity=${3:?identity fingerprint required}
case "$id" in ''|*[!a-zA-Z0-9-]*) exit 64 ;; esac
case "$identity" in ''|*[!a-f0-9]*) exit 64 ;; esac
# Host tests must supply an explicit fake ubus/uci/service/signal backend. Never
# silently skip runtime recovery or send a signal to a host PID.
if [ -n "$root" ]; then test -x "${U60_TEST_BACKEND:?test runtime backend required}"; fi
transaction="$base/$id"
legacy_targets='data/zte-agent
data/open-u60-manifest.json
data/www
data/www.current
data/local/tmp/start_zte_agent.sh
data/local/tmp/start_dashboard.sh
data/local/tmp/start_dropbear.sh
data/bin/dropbear
data/bin/dbclient
data/bin/dropbearkey
data/bin/dashboard-uhttpd
etc/rc.local
etc/dropbear
data/dropbear
etc/config/uhttpd'
targets="$legacy_targets
data/bin/wg
data/bin/mihomo
data/share/mihomo
data/bin/openui-screen
data/local/tmp/openui-screen-verified
data/local/tmp/openui-screen-licenses
data/bin/lpac
data/local/tmp/openui-lpac-licenses
data/bin/tailscale
data/bin/tailscaled
data/bin/relay-band-guard.sh
etc/init.d/openui-tailscale
etc/config/zwrt_zte_dm
etc/config/wireless
data/local/tmp/relay-band-guard.disabled
data/local/tmp/openui-imei-helpers"

runtime() (
    # Runtime services may outlive this transaction. Never let a daemon inherit
    # controller.lock: only the transaction parent retains it through final sync.
    # This child scope also covers the explicit host backend and legacy starts.
    exec 9>&-
    if [ -n "$root" ]; then "$U60_TEST_BACKEND" "$@"; return; fi
    operation=$1; shift
    case "$operation" in
        procd-list) ubus call service list '{"name":"openui-tailscale"}' ;;
        procd-delete) ubus call service delete '{"name":"openui-tailscale"}' ;;
        fota-get) uci -q get zwrt_zte_dm.dm_update.dm_update_mode ;;
        fota-call) ubus call zwrt_zte_dm set_update_mode "{\"dm_update_mode\":\"$1\"}" ;;
        exe-id) stat -Lc '%d:%i' "/proc/$1/exe" ;;
        signal) kill -TERM "$1" ;;
        wait) sleep 1 ;;
        start)
            case "$1" in
                tailscale) /etc/init.d/openui-tailscale start ;;
                guard) nohup /data/bin/relay-band-guard.sh >/dev/null 2>&1 </dev/null & ;;
                agent)
                    # The canonical startup launches its guard asynchronously.
                    # Suppress that launch when it was originally stopped,
                    # rather than racing a later scan against a background fork.
                    (
                        nohup() {
                            if [ "$1" = /data/bin/relay-band-guard.sh ] && [ ! -s "$transaction/runtime/guard" ]; then return 0; fi
                            command nohup "$@"
                        }
                        . /data/local/tmp/start_zte_agent.sh
                    ) ;;
                dropbear|dashboard) sh "/data/local/tmp/start_$1.sh" ;;
            esac ;;
        http-restart) /etc/init.d/uhttpd restart ;;
        *) return 64 ;;
    esac
)

restore_fota() {
    mode=$(cat "$transaction/runtime/fota")
    case "$mode" in 0|1) ;; *) return 65 ;; esac
    reply=$(runtime fota-call "$mode") || return 65
    # Do not let the already-restored UCI file mask an RPC business error.
    # Only an empty JSON object is accepted as the setter acknowledgement;
    # error fields, malformed data and unknown reply schemas stay pending.
    test "$(printf '%s' "$reply" | tr -d '\011\012\015\040')" = '{}' || { echo 'Unrecognized FOTA setter acknowledgement; recovery remains pending' >&2; return 65; }
    test "$(runtime fota-get)" = "$mode"
}

argv_hash() { printf '%s\000' "$@" | sha256sum | awk '{print $1}'; }
guard_argv() {
    test "$1" = "$(argv_hash /bin/sh /data/bin/relay-band-guard.sh)" ||
    test "$1" = "$(argv_hash /bin/ash /data/bin/relay-band-guard.sh)" ||
    test "$1" = "$(argv_hash /data/bin/relay-band-guard.sh)"
}

# A process token covers start time, executable (including deleted mappings),
# and ALL NUL-separated arguments. Never trust a pidfile or a substring alone.
process_token() {
    p=$1
    test -r "$proc/$p/stat" && test -r "$proc/$p/cmdline" || return 1
    stat=$(sed 's/^.*) //' "$proc/$p/stat")
    test "${stat%% *}" != Z || return 1
    start=$(printf '%s\n' "$stat" | awk '{print $20}')
    case "$start" in ''|*[!0-9]*) return 1 ;; esac
    test "$(awk '/^Uid:/ {print $2 ":" $3 ":" $4 ":" $5}' "$proc/$p/status")" = 0:0:0:0 || return 1
    exe=$(readlink "$proc/$p/exe") || return 1
    inode=$(runtime exe-id "$p") || return 1
    argv=$(sha256sum "$proc/$p/cmdline" | awk '{print $1}')
    printf '%s|%s|%s|%s\n' "$start" "$exe" "$inode" "$argv"
}

listener() {
    listen_pid=$1; listen_port=$2
    inodes=$(awk -v port=":$listen_port" '$2 ~ port"$" && $4 == "0A" {print $10}' "$proc/net/tcp" "$proc/net/tcp6" 2>/dev/null || true)
    for fd in "$proc/$listen_pid"/fd/*; do
        socket=$(readlink "$fd" 2>/dev/null || true)
        for inode in $inodes; do [ "$socket" != "socket:[$inode]" ] || return 0; done
    done
    return 1
}

scan() {
    kind=$1
    for entry in "$proc"/[0-9]*; do
        [ -d "$entry" ] || continue
        pid=${entry##*/}
        actual=$(readlink "$entry/exe" 2>/dev/null || true)
        args=$(tr '\000' '\n' < "$entry/cmdline" 2>/dev/null || true)
        case "$kind" in
            agent)
                case "$actual" in '/data/zte-agent'|'/data/zte-agent (deleted)') ;; *) continue ;; esac
                hash=$(sha256sum "$entry/cmdline" | awk '{print $1}')
                if [ "$hash" != "$(argv_hash /data/zte-agent)" ] && [ "$hash" != "$(argv_hash /data/zte-agent log-writer)" ]; then
                    echo 'Unrecognized owned agent command' >&2; return 65
                fi ;;
            guard)
                # Any exact script argument is a candidate, not proof of ownership.
                printf '%s\n' "$args" | grep -qx /data/bin/relay-band-guard.sh || continue
                case "$actual" in /bin/busybox|/bin/ash|/bin/sh) ;; *) echo 'Unrecognized guard executable' >&2; return 65 ;; esac
                hash=$(sha256sum "$entry/cmdline" | awk '{print $1}')
                guard_argv "$hash" || { echo 'Unrecognized guard command' >&2; return 65; } ;;
            dropbear|dashboard)
                if [ "$kind" = dropbear ]; then binary=dropbear; port=08AE; else binary=dashboard-uhttpd; port=1F90; fi
                case "$actual" in "/data/bin/$binary"|"/data/bin/$binary (deleted)") ;; *) continue ;; esac
                listener "$pid" "$port" || continue
                test "${args%%
*}" = "/data/bin/$binary" || { echo 'Unrecognized owned listener command' >&2; return 65; } ;;
        esac
        token=$(process_token "$pid") || { gone "$pid" && continue; return 65; }
        printf '%s|%s\n' "$pid" "$token"
    done
    if [ "$kind" = guard ] && [ -s "$root/var/run/relay-band-guard.pid" ]; then
        hint=$(cut -d '|' -f1 "$root/var/run/relay-band-guard.pid")
        case "$hint" in ''|*[!0-9]*) return 65 ;; esac
        if [ -d "$proc/$hint" ]; then
            hint_hash=$(sha256sum "$proc/$hint/cmdline" | awk '{print $1}')
            guard_argv "$hint_hash" || { echo 'Guard pidfile points to an unidentified process' >&2; return 65; }
            saved=$(cat "$root/var/run/relay-band-guard.pid")
            case "$saved" in *'|'*) test "$saved" = "$hint|$(process_token "$hint")" || { echo 'Guard PID identity changed' >&2; return 65; } ;; esac
        fi
    fi
}

# Native B31 has Lua/luci.jsonc, not Python. The explicit host backend replaces
# ONLY this JSON parser and ubus/uci/process effects; proc ownership checks above
# and the entire transaction lifecycle still run unchanged in tests.
procd_status() {
    runtime procd-list > "$transaction/procd.json" || return 65
    if [ -n "$root" ]; then
        "$U60_TEST_BACKEND" procd-parse < "$transaction/procd.json"
    else
        lua -e '
local j = require("luci.jsonc").parse(io.read("*a"))
local function check(v) if not v then error("Unrecognized openui-tailscale ownership") end end
check(type(j) == "table")
local s = j["openui-tailscale"]
if s == nil then check(next(j) == nil); print("absent"); return end
check(type(s) == "table" and type(s.instances) == "table")
local n, instance = 0, nil
for _, v in pairs(s.instances) do n = n + 1; instance = v end
check(n == 1 and type(instance) == "table")
local c = instance.command
check(type(c) == "table" and #c == 6 and (instance.user == nil or instance.user == "root"))
local bin = c[1]
check(bin == "/data/bin/tailscaled" or bin == "/data/mu5250-admin/bin/tailscaled")
local state = c[3] and c[3]:match("^%-%-statedir=(.+)$")
check(state == "/data/tailscale" or state == "/data/mu5250-admin/tailscale")
check(c[2] == "--state=" .. state .. "/tailscaled.state" and c[4] == "--socket=/var/run/tailscale/tailscaled.sock" and c[5] == "--tun=tailscale0" and c[6] == "--port=41641")
check(type(instance.running) == "boolean")
if instance.running then
 check(type(instance.pid) == "number" and instance.pid > 1 and instance.pid % 1 == 0)
 print("running " .. instance.pid .. " " .. bin .. " " .. state)
else print("stopped") end'
    fi
}

tailscale_token() {
    # procd command ownership was checked before accepting its PID. Match the
    # live executable AND full argv too, even when its on-disk file was removed.
    set -- $1
    test "$1" = running || return 0
    ts_pid=$2; ts_bin=$3; ts_state=$4
    ts_token=$(process_token "$ts_pid") || return 65
    ts_exe=$(readlink "$proc/$ts_pid/exe")
    case "$ts_exe" in "$ts_bin"|"$ts_bin (deleted)") ;; *) return 65 ;; esac
    expected=$(printf '%s\000' "$ts_bin" "--state=$ts_state/tailscaled.state" "--statedir=$ts_state" --socket=/var/run/tailscale/tailscaled.sock --tun=tailscale0 --port=41641 | sha256sum | awk '{print $1}')
    test "${ts_token##*|}" = "$expected" || { echo 'Tailscale process command does not match procd' >&2; return 65; }
    printf '%s|%s\n' "$ts_pid" "$ts_token"
}

gone() {
    [ ! -d "$proc/$1" ] || [ "$(sed 's/^.*) //' "$proc/$1/stat" | cut -d ' ' -f1)" = Z ]
}

stop_tokens() {
    while IFS='|' read -r stop_pid stop_identity; do
        [ -n "$stop_pid" ] || continue
        # Identity changes are errors, not permission to signal a reused PID.
        current=$(process_token "$stop_pid") || { gone "$stop_pid" && continue; return 65; }
        test "$current" = "$stop_identity" || { echo 'PID identity changed before stop' >&2; return 65; }
        runtime signal "$stop_pid" || { gone "$stop_pid" && continue; return 65; }
        attempts=0
        while current=$(process_token "$stop_pid"); do
            test "$current" = "$stop_identity" || { echo 'PID identity changed while stopping' >&2; return 65; }
            attempts=$((attempts+1)); test "$attempts" -le 5 || return 65
            runtime wait || return 65
        done
        gone "$stop_pid" || { echo 'Stopped process became unidentifiable' >&2; return 65; }
    done < "$1"
}

stop_service() {
    stopping=$1
    if [ "$stopping" = tailscale ]; then
        status=$(procd_status) || return 65
        tailscale_token "$status" > "$transaction/stop-tailscale" || return 65
        if [ "$status" != absent ]; then
            # Delete by validated service name, not init-script existence. procd
            # can respawn a new binary even after rollback removes that script.
            runtime procd-delete || return 65
        fi
        # procd may already have reaped it. Otherwise signal only the captured
        # identity and wait before replacing/removing its executable.
        stop_tokens "$transaction/stop-tailscale" || return 65
        test "$(procd_status)" = absent
    else
        scan "$stopping" > "$transaction/stop-$stopping" || return 65
        stop_tokens "$transaction/stop-$stopping"
    fi
}

capture_runtime() {
    mkdir "$transaction/runtime"
    for service in agent guard dropbear dashboard; do
        scan "$service" > "$transaction/runtime/$service"
    done
    status=$(procd_status)
    tailscale_token "$status" > "$transaction/runtime/tailscale"
    printf '%s\n' "$status" > "$transaction/runtime/procd"
    mode=$(runtime fota-get)
    case "$mode" in 0|1) ;; *) echo 'Unsupported saved FOTA update mode' >&2; return 65 ;; esac
    printf '%s\n' "$mode" > "$transaction/runtime/fota"
}

live_service() {
    if [ "$1" = tailscale ]; then
        status=$(procd_status) || return 65
        tailscale_token "$status"
    else scan "$1"; fi
}

restore_runtime() {
    # Do not abandon a known management service because an independent sidecar
    # failed. Collect errors, attempt each old service, then verify all states.
    runtime_failed=0
    for service in $restore_services; do
        if [ -s "$transaction/runtime/$service" ]; then
            if live=$(live_service "$service"); then
                if [ -z "$live" ]; then runtime start "$service" || runtime_failed=1; fi
            else runtime_failed=1; fi
        elif [ "$service" = guard ] || [ "$service" = tailscale ]; then
            stop_service "$service" || runtime_failed=1
        fi
    done
    for service in $restore_services; do
        attempts=0
        while :; do
            if ! live=$(live_service "$service"); then runtime_failed=1; break; fi
            if { [ -n "$live" ] && [ -s "$transaction/runtime/$service" ]; } ||
               { [ -z "$live" ] && [ ! -s "$transaction/runtime/$service" ]; }; then break; fi
            attempts=$((attempts+1))
            if [ "$attempts" -gt 5 ]; then runtime_failed=1; break; fi
            runtime wait || { runtime_failed=1; break; }
        done
    done
    test "$runtime_failed" = 0
}

changed() {
    name=$(printf '%s' "$1" | tr / _)
    old="$transaction/before/$name"; now="$root/$1"
    if [ -L "$old" ] || [ -L "$now" ]; then
        test "$(readlink "$old" 2>/dev/null || true)" != "$(readlink "$now" 2>/dev/null || true)"
    elif [ -f "$old" ] && [ -f "$now" ]; then
        test "$(sha256sum "$old" | awk '{print $1}')" != "$(sha256sum "$now" | awk '{print $1}')"
    else
        { [ -e "$old" ] && [ ! -e "$now" ]; } || { [ ! -e "$old" ] && [ -e "$now" ]; }
    fi
}

write_state() {
    printf '%s\n' "$1" > "$transaction/state.new"
    sync
    mv "$transaction/state.new" "$transaction/state"
    sync
}

# The IMEI controller holds this same flock through each durable operation.
# Take it BEFORE publishing active, then keep it until the marker is durable:
# workers recheck that marker after acquiring their lease. Never replace a
# controller/helper while an unfinished NV/EFS operation needs its old version.
imei_deployment_guard() {
    imei_state="$root/data/openui-imei"
    # Create the shared lock even before the first controller operation; skipping
    # an absent directory would race a concurrently queued first CHECK.
    test ! -L "$root/data" && test -d "$root/data"
    if [ ! -e "$imei_state" ] && [ ! -L "$imei_state" ]; then mkdir -m 700 "$imei_state"; fi
    test ! -L "$imei_state" && test -d "$imei_state"
    test "$(stat -c '%u:%a' "$imei_state")" = '0:700'
    if [ ! -e "$imei_state/controller.lock" ] && [ ! -L "$imei_state/controller.lock" ]; then
        (umask 077; set -C; : > "$imei_state/controller.lock")
    fi
    test ! -L "$imei_state/controller.lock" && test -f "$imei_state/controller.lock"
    test "$(stat -c '%u:%a:%h' "$imei_state/controller.lock")" = '0:600:1'
    exec 9<> "$imei_state/controller.lock"
    flock -n 9 || { echo 'An IMEI operation is busy; no installation changes were made.' >&2; return 75; }
    if [ -e "$imei_state/current.json" ] || [ -L "$imei_state/current.json" ]; then
        test ! -L "$imei_state/current.json" && test -f "$imei_state/current.json"
        test "$(stat -c '%u:%a:%h' "$imei_state/current.json")" = '0:600:1'
        lua - "$imei_state/current.json" <<'LUA'
local f=assert(io.open(arg[1], 'rb'))
local b=f:read(65537); f:close()
assert(type(b)=='string' and #b<=65536, 'Invalid IMEI journal')
local j=require('luci.jsonc').parse(b)
assert(type(j)=='table' and j.schema==1 and j.stage=='complete' and j.uncertain==false,
       'Finish IMEI recovery before deployment; no installation changes were made')
LUA
    fi
}

case "$action" in
begin)
    imei_deployment_guard
    mkdir -p "$base"
    if ! mkdir "$base/lock" 2>/dev/null; then
        echo 'An unfinished deployment exists. Recover it before starting another installation.' >&2
        exit 73
    fi
    printf '%s\n' "$id" > "$base/lock/owner"
    if ! mkdir "$transaction"; then rm -f "$base/lock/owner"; rmdir "$base/lock"; exit 73; fi
    snapshot_cleanup() {
        # Once pending exists, even failure while publishing active must keep
        # this recovery point and its owned lock. restore can republish active.
        [ ! -f "$transaction/state" ] || return 0
        rm -rf "$transaction"
        rm -f "$base/lock/owner"; rmdir "$base/lock" 2>/dev/null || true
    }
    trap snapshot_cleanup EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
    mkdir "$transaction/before" "$transaction/present"
    printf '%s\n' "$targets" > "$transaction/targets"
    printf '%s\n' "$identity" > "$transaction/identity"
    capture_runtime
    for target in $targets; do
        name=$(printf '%s' "$target" | tr / _)
        if [ -e "$root/$target" ] || [ -L "$root/$target" ]; then
            cp -a "$root/$target" "$transaction/before/$name"
            : > "$transaction/present/$name"
        fi
    done
    sync
    write_state pending
    printf '%s\n' "$id" > "$base/active.new"
    mv "$base/active.new" "$base/active"
    sync
    trap - EXIT HUP INT TERM
    ;;
discard-incomplete)
    test ! -e "$base/active"
    test ! -e "$transaction/state"
    test "$(cat "$base/lock/owner")" = "$id"
    test "$(cat "$transaction/identity")" = "$identity"
    rm -rf "$transaction"
    rm -f "$base/lock/owner"
    rmdir "$base/lock"
    sync
    ;;
quiesce|restore|complete)
    inventory=true
    if [ -f "$transaction/targets" ]; then targets=$(cat "$transaction/targets"); else targets=$legacy_targets; inventory=false; fi
    test "$(cat "$transaction/identity")" = "$identity"
    case "$(cat "$transaction/state")" in pending|committed|restored) ;; *) exit 65 ;; esac
    if [ "$action" = restore ]; then imei_deployment_guard; fi
    if [ "$action" = restore ] && [ ! -e "$base/active" ]; then
        if [ -d "$base/lock" ]; then test "$(cat "$base/lock/owner")" = "$id"
        else mkdir "$base/lock"; printf '%s\n' "$id" > "$base/lock/owner"; fi
        printf '%s\n' "$id" > "$base/active"
    fi
    test "$(cat "$base/active")" = "$id"
    test "$(cat "$base/lock/owner")" = "$id"
    if [ "$action" = quiesce ]; then
        test "$(cat "$transaction/state")" = pending
        scope=${4:?quiesce scope required}
        case "$scope" in all|tailscale|none) ;; *) exit 64 ;; esac
        printf '%s\n' "$scope" > "$transaction/scope"; sync
        case "$scope" in
            all) stop_service agent; stop_service guard; stop_service tailscale ;;
            tailscale) stop_service tailscale ;;
        esac
        exit 0
    fi
    if [ "$action" = complete ]; then test "$(cat "$transaction/state")" = pending; fi
    if [ "$action" = restore ]; then
        # Mark recovery pending BEFORE any effect, including a retry of an older
        # committed/restored snapshot. No successful state on partial recovery.
        write_state pending
        reload_stock_http=false
        if [ -f "$transaction/present/etc_config_uhttpd" ]; then
            previous=$(sha256sum "$transaction/before/etc_config_uhttpd" | awk '{print $1}')
            current=$(sha256sum "$root/etc/config/uhttpd" 2>/dev/null | awk '{print $1}')
            [ "$previous" = "$current" ] || reload_stock_http=true
        fi
        # Old snapshots have neither inventory nor a runtime journal. Their
        # coverage remains explicitly limited; do not touch untracked sidecars.
        restore_services='agent dropbear dashboard'
        scope=legacy
        if [ "$inventory" = true ] && [ -d "$transaction/runtime" ]; then
            scope=$(cat "$transaction/scope" 2>/dev/null || printf none)
            if [ ! -f "$transaction/restore-services" ]; then
                case "$scope" in all) restore_services='agent guard tailscale' ;; tailscale) restore_services=tailscale ;; none) restore_services='' ;; *) exit 65 ;; esac
                if changed data/bin/dropbear || changed data/local/tmp/start_dropbear.sh; then restore_services="$restore_services dropbear"; fi
                if changed data/bin/dashboard-uhttpd || changed data/local/tmp/start_dashboard.sh || changed data/www.current || changed etc/config/uhttpd; then restore_services="$restore_services dashboard"; fi
                printf '%s\n' "$restore_services" > "$transaction/restore-services.new"
                mv "$transaction/restore-services.new" "$transaction/restore-services"; sync
            fi
            restore_services=$(cat "$transaction/restore-services")
        fi
        for service in $restore_services; do stop_service "$service"; done
        for target in $targets; do
            # A dashboard-only rollback must not rewind a live relay's UCI or
            # marker, nor replace an unaffected sidecar under its running shell.
            case "$target:$scope" in
                *:legacy) ;;
                etc/config/wireless:all|data/local/tmp/relay-band-guard.disabled:all|data/bin/relay-band-guard.sh:all) ;;
                etc/config/wireless:*|data/local/tmp/relay-band-guard.disabled:*|data/bin/relay-band-guard.sh:*) continue ;;
                data/bin/tailscale:all|data/bin/tailscaled:all|etc/init.d/openui-tailscale:all|data/bin/tailscale:tailscale|data/bin/tailscaled:tailscale|etc/init.d/openui-tailscale:tailscale) ;;
                data/bin/tailscale:*|data/bin/tailscaled:*|etc/init.d/openui-tailscale:*) continue ;;
            esac
            name=$(printf '%s' "$target" | tr / _)
            destination="$root/$target"
            staged="$destination.restore-$id"
            if [ -f "$transaction/present/$name" ]; then
                mkdir -p "${destination%/*}"
                rm -rf "$staged"
                cp -a "$transaction/before/$name" "$staged"
                case "$target" in etc/rc.local|data/local/tmp/start_*.sh) sh -n "$staged" ;; esac
                if [ -d "$destination" ] && [ ! -L "$destination" ]; then
                    failed="$transaction/failed-$name"
                    if [ ! -e "$failed" ]; then mv "$destination" "$failed"; else rm -rf "$destination"; fi
                fi
                mv -Tf "$staged" "$destination"
            else
                rm -rf "$destination"
            fi
        done
        recovery_failed=0
        if [ "$reload_stock_http" = true ]; then runtime http-restart || recovery_failed=1; fi
        if [ "$inventory" = true ] && [ -d "$transaction/runtime" ]; then
            restore_fota || recovery_failed=1
            restore_runtime || recovery_failed=1
        else
            echo 'Legacy snapshot: limited file/service coverage; sidecars and FOTA runtime were not recorded.' >&2
            for service in dropbear agent dashboard; do
                case "$service" in agent) startup=start_zte_agent.sh ;; *) startup=start_$service.sh ;; esac
                if [ -f "$root/data/local/tmp/$startup" ]; then runtime start "$service" || recovery_failed=1; fi
            done
        fi
        test "$recovery_failed" = 0
        write_state restored
    else
        write_state committed
    fi
    sync
    rm -f "$base/active"
    rm -f "$base/lock/owner"
    rmdir "$base/lock"
    sync
    ;;
*) exit 64 ;;
esac
