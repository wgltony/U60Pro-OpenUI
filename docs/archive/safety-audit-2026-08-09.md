> **Historical audit, not current installation guidance or a no-risk guarantee.** Descriptions of deployed components, Tailscale and relay predate the current source. Use [current safety notes](../SAFETY.md) and [install.sh guide](../INSTALL.md).

# Safety audit — 2026-08-09

Scope: every commit (`git log --all`), the working tree, and the deploy
path. Goal: confirm nothing in this repo can brick the device when ADB is
regained and this is deployed again.

**Verdict: the deploy path is clean.** Findings and dispositions below.

## 1. Deploy path

| Surface | What it does | Verdict |
|---|---|---|
| `setup.sh` | unlock (via zunlock.py) + agent push, startup script, rc.local line | idempotent, grep-guarded rc.local edits; safe |
| `deploy.sh` | ssh-only binary push + restart | safe |
| `deploy-dashboard.sh` | builds `web-app`, tars to `/data/www`, restarts/verifies isolated dashboard uhttpd | safe (data partition only) |
| `scripts/zharden.sh` | Dropbear + isolated upstream uhttpd to `/data`, rc.local cleanup, checked :8080 setup, FOTA off | pinned package hashes; v2 removed the firewall-include bootstrap — the last boot-critical hook |
| `scripts/zunlock.py` / `zbackup.py` | config backup patch + restore (the unlock itself) | highest-risk by nature, but gated: `--dry-run`, explicit confirm, sha256 upload verification, payload auto-discovered from the device's own rc.local |

## 2. Agent boot-time behavior — acceptable, documented

- `main.rs` runs `/data/local/tmp/start_ttl.sh` (iptables mangle TTL/HL
  rules). Runtime firewall only; no persistence beyond that script. Safe.
- `usb::enforce_usb_mode_on_boot()` rebuilds the USB configfs gadget **only
  if NCM was explicitly persisted** (`/data/local/tmp/usb_config.json`),
  waits up to 75 s for the stock USB stack to finish (bridge-membership
  check), and skips in power-off-charging states. configfs is runtime sysfs —
  a failure cannot persist across reboot. Worst case: tethering needs a
  reboot. Acceptable.
- All agent state files live under `/data/local/tmp/` (writable data
  partition). The agent never writes `/etc`, `/zteoverlay`, or raw
  partitions, except via `uci commit wireless`/`dhcp` and the
  documented rc.local lines.

## 3. Findings acted on (2026-08-09)

1. **Exploit tooling quarantined.** `zacs.py` (rogue TR-069 ACS with
   SetParameterValues), `zrce.py`, `zinj.py`, `zdns.py`, `zstrings.py`,
   `zgap.py` (fac_reset/fac_reboot/FOTA probes), `zadb.py`, `zhidden.py`,
   `zwrite.py` moved to `scripts/research/` with a warning README. They
   never ran as part of deploy, but they are the class of tool that bricked
   the device and should not sit next to sanctioned tools.
2. **Safety docs consolidated into this file** (device rules + audit).
3. **v2.1 upstream regressions rejected** during the feature port (below).
4. **setup.sh unlock modernized**: the dead `zwrt_bsp.usb set {mode:debug}`
   path replaced with the backup/restore route; the broken in-setup dropbear
   install (404 URL + unusable opkg) removed in favor of `zharden.sh`.

## 4. v2.1 features ported, and what was deliberately NOT ported

Ported (safe, gated): charge control (`zwrt_bsp.charger`, inverted semantics
handled), USB powerbank (`zwrt_bsp.powerbank set`), firmware WMS SMS translation
with readiness gating (no direct database writes), `/api/at/port`, WiFi dual UCI namespace
(`zte_mbb.wifi.*` + `wireless.zte_mbb.*`) with guest-time read.

**Rejected from v2.1 (safety regressions):**

| v2.1 change | Why rejected |
|---|---|
| kill-bloat list adds `zte_topsw_mc`, `zte_dm`, `zte_topsw_wms`, `zte_topsw_sleep_faw`, `zte_topsw_tr098db` | daemon.conf daemons — sync-barrier risk (see above); current list kept |
| unrestricted AT terminal (any `AT…` accepted) | current allowlist kept (`AT+CFUN`, `AT^…`, `AT+CMGD`, `AT$QCRMCALL` etc. blocked) |
| bind `0.0.0.0:9090`, no CORS pinning, no body limit, no `X-Confirm` on destructive ops | current hardening kept (LAN bind `192.168.0.1`, LAN-only CORS, 1 MiB body cap, `X-Confirm: true` required) |

The scheduler, DoH proxy and SMS forwarder have since been removed entirely —
they had no dashboard surface, and `main.rs` runs a one-shot migration that
undoes DoH's dnsmasq rewiring so a device that had it enabled does not come
back up forwarding DNS to a dead port.
| Tailscale module | skipped by owner decision (installs/supervises a daemon, downloads binaries) |

## 5. Secrets / sensitive data

- `back_parameter` (encrypted config backup), `adb-lock-investigation.md`
  (contains the backup-key suffix, IMEI and sticker credentials), `logs/`,
  `loopdebug-capture/` are **gitignored and never committed** — verified
  across all history. Keep it that way.
- No IMEI, passwords, session tokens or backup-key material in any committed
  file (scanned 2026-08-09).
- `scripts/research/` tools are env-parameterized (no embedded credentials).

## 6. Residual risks (accepted, documented)

- `zunlock.py` restore path: inherent to the unlock; mitigations in place.
- NCM gadget rebuild: runtime-only, recoverable by reboot.
- `zwrt_bsp.charger set` can stop charging; the charge-limit enforcer
  re-enables on charger unplug and on disable. Manual override is exposed
  via the API.
- `scripts/research/` remains runnable if deliberately invoked — that is the
  point of the quarantine README.
