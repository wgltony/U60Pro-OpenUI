# Safety — how to not brick the U60 Pro

Hard-won rules for this device. Read before running anything against the
modem with ADB/SSH access. A July 2026 brick incident motivates these boundaries;
following them reduces risk, but does not guarantee recovery from every failure.
Use [install.sh](INSTALL.md), not inherited GUI/setup tutorials.

## Golden rules

1. **Do not alter stock boot-critical services.** OpenUI startup uses
   `/etc/rc.local`; do not add firewall includes or hooks to vendor boot services.
   A blocking hook can wedge the device before recovery interfaces exist
   (`firewall.zte_recovery`, withdrawn in commit `a41f89c`, is the example not to
   reintroduce). The project-owned optional `/etc/init.d/openui-tailscale`
   service and relay-band-guard started by the agent startup script are intentional,
   not permission to disable vendor services. Documented Wi-Fi UCI writes are
   allowed; a blanket claim that OpenUI never writes UCI/init.d would be false.
2. **Never disable a daemon listed in `zte_topsw_daemon.conf` via
   `/etc/init.d/<name> disable`** — see the sync barrier below.
3. **Stay out of partitions.** No `dd`, `mtd`, `fw_setenv`, `/dev/block`,
   QFPROM/fuse writes, `abctl --set_active` (mixed-slot boots can brick).
   There is no recovery from these without a flasher.
4. **Config backup/restore is the only sanctioned privileged path**
   (`scripts/zunlock.py`, `scripts/zbackup.py`). Always `--dry-run` first;
   restore runs as root and extracts whatever it validates.
5. **Exploit/injection tooling lives in `scripts/research/` and stays
   there.** It is quarantined for a reason — see its README.
6. **FOTA auto-update stays off** (`zharden.sh` step 4). A surprise firmware
   update wipes rc.local hooks and can change the rules under the agent.
7. **Never write the USB composition node (`usb_op`) live** — set it via
   rc.local + reboot. Live writes (`0` or `2` after `1`) kill the gadget
   until the next reboot.

## CRITICAL: ZTE daemon sync barrier

`zte_topsw_daemon` is the master daemon. It reads
`/etc/config/zte_topsw_daemon.conf` and **waits for ALL listed daemons to
register** before releasing the boot sequence.

If you disable a daemon.conf daemon via init.d, the device will:

- stick on the ZTE boot logo (UI never renders)
- never connect WAN (mobile data call never initiated)
- leave the touchscreen dead (mtdev2tuio bridge never starts)

…while all other daemons appear to run fine, which makes the root cause
very hard to diagnose.

**Daemons in daemon.conf (NEVER disable via init.d, NEVER kill casually):**

```
zte_topsw_mc, zte_router, zte_topsw_data, zte_topsw_nwinfo,
zte_topsw_mdm, zte_topsw_sleep_faw, zte_topsw_apn, zte_topsw_wms,
zte_topsw_key, zte_topsw_led, zte_topsw_tr098db, zte_dm,
zte_topsw_fota_result, zte_topsw_devui, zte_topsw_wlan, zte_smart_manage
```

**Optional candidates observed outside daemon.conf (not a universal safe list):**

```
zte_topsw_diag, zte_topsw_samba, zte_topsw_nfc, zte_topsw_get_brand,
zte_topsw_jwxk_query, zte_topsw_tr069_sub, zte_mqtt_sdk_st,
zte_topsw_dua, zte-topsw-tunnel
```

The agent's `kill-bloat` endpoint (`agent/src/system.rs` `SAFE_OPTIONAL_DAEMONS`)
contains only the safe list and subtracts the live daemon.conf barrier at
runtime. An unreadable or empty barrier blocks the operation. Do not add daemon.conf names to it
(killing `zte_topsw_wms` breaks SMS, `zte_topsw_sleep_faw` breaks wakelocks,
and procd will respawn them anyway).

Do not edit the barrier configuration to bypass this rule during installation.
Firmware-specific service changes require separate investigation and authorization.

### Overlay whiteouts

`/etc/init.d/<name> disable` creates **whiteout character devices** in
`/zteoverlay/etc-upper_a/rc.d/` that silently delete the ROM symlinks.
Invisible in normal `ls /etc/rc.d/`, persistent across reboots.

- Check: `ls -la /zteoverlay/etc-upper_a/rc.d/ | grep '^c'`
- Fix: `rm /zteoverlay/etc-upper_a/rc.d/<whiteout_file>`

### Verify sync status after boot

```sh
ubus call zwrt_topsw_daemon.sync get_sync_info '{}'
# Should return: "noSyncModuleName": "sync success"
```

## Recovery commands

**Display stuck on logo:**

```sh
sh /usr/bin/mtdev2tuio.sh                                      # touchscreen bridge
kill -9 $(pidof zte_topsw_devui); /usr/bin/zte_topsw_devui &   # restart UI
```

**WAN not connecting:**

```sh
ubus call zwrt_qcmap_cli set_qcliiface '{"source_module":"zte_topsw_data","type":1,"enable":1,"sub_id":1}'
ubus call zwrt_qcmap_cli set_qcliiface '{"source_module":"zte_topsw_data","type":2,"enable":1,"sub_id":1}'
```

**Check data call status:**

```sh
ubus call zwrt_data get_wwaniface '{"source_module":"zte_topsw_data","cid":1}'
# Look for: "enable": 1, "connect_status": "connected"
```

## Firmware behavior gotchas

- **Airplane mode bug**: `nwinfo_set_mode ONLINE` does NOT recover the modem
  from low-power mode. Only fix: reboot.
- **Charge policy inversion**: `zwrt_bsp.charger set
  {"direct_power_supply_mode":"enable"}` STOPS charging; `"disable"` STARTS
  it. The agent's charge-control code (`agent/src/charge_policy.rs`) already
  accounts for this — don't "fix" the inversion.
- **procd respawn**: `kill -9` on a procd service may respawn it. Use
  `/etc/init.d/<name> stop` instead.
- **rc.local discipline**: stock rc.local contains a flash-protect block that
  READS `usb_op` — never delete that block (`sed '/usb_op/d'` breaks the
  script's syntax and kills ALL rc.local actions next boot). Always `sh -n
  /etc/rc.local` after any edit.
- **rc.local truncation by host quoting** (observed 2026-10-10, CN B31): a host
  command such as `adb shell "grep -cF "echo 1 >" /etc/rc.local"` loses its
  inner quotes in transit, so the *device* shell receives
  `grep -cF echo 1 > /etc/rc.local` — an unquoted redirection that **truncates
  rc.local to 0 bytes**, destroying both the boot hooks and the flash-protect
  block. Recovery: the deployment transaction snapshot keeps a pre-write copy at
  `/data/local/tmp/open-u60-transactions/<id>/before/etc_rc.local`. Prevention:
  never inline-quote device commands through a host shell — push a script and
  run it. See [OPERATIONS.md](OPERATIONS.md) §7.1.
- **IMEI is QFPROM-fused** — hardware-locked, not modifiable. Don't try.
- **eSIM**: the CN unit has no usable built-in eUICC. Removable eUICC cards are
  managed with lpac ([ESIM.md](ESIM.md)) through QMI UIM and DMS requests only, never
  AT commands. A switch takes the radio to low power, powers the SIM off and on and
  sets the radio online again; firmware daemons are never restarted. When the
  firmware keeps the previous profile, OpenUI offers a restart instead.


## Current deployment and runtime boundaries

The current `install.sh` flow stages the agent, dashboard, screen, lpac, Clash,
WireGuard tools, relay guard and optional Tailscale; it uses the shared transaction
helper. Hardening installs key-only SSH, the isolated dashboard listener, guarded
`/etc/rc.local` lines and disables FOTA. See [Deployment](DEPLOYMENT.md) for paths,
rollback coverage/legacy limits and source prerequisites, not an old setup recipe.

The relay guard deliberately pauses the same-band hotspot, commits
`wireless.<radio>.disabled` to UCI and retains
`/data/local/tmp/relay-band-guard.disabled` across reboot. Keep opposite-band Wi-Fi
or USB management available; do not rely on a same-band client roaming by itself.
The marker needs the guard to run for restoration (after its boot grace period);
a reboot without the guard is not a promise that all relay changes disappear.

Personal configuration stays local rather than being uploaded to project servers,
but **both router and computer contain private material**. Encrypted original
recovery backups and identity metadata remain on the computer; standalone dry runs
and `--keep-work` may also keep decrypted configuration. Main-installer temporary
cleanup is separate from retained encrypted recovery backups. Never publish these
materials; do not assume native backups contain Tailscale state. No leak is being
asserted. See [privacy and recovery material](DEPLOYMENT.md#privacy-and-recovery-material).

Source review, synthetic tests, mocks and CI are not clean-stock B31/cold-boot,
physical eSIM or Windows first-install qualification. Installation/restoration has
risk; use a separately authorized spare device for hardware acceptance testing.

## Known-good ubus surface

`zte-script-ng.js` (repo root) is the community-vetted reference of ubus
calls that are safe on this firmware: `zte_nwinfo_api` (netinfo/netselect/
bandlock/cell lock), `zwrt_wlan set` (txpower/country/maxassoc), `uci get`,
and read-only status objects. New agent features should prefer these objects
and methods; anything outside that surface deserves extra scrutiny.

---

## Historical audits

The [2026-08-09 audit](archive/safety-audit-2026-08-09.md) is retained as history,
not a current component inventory or proof that installation cannot fail.

## WireGuard gateway extension

The user-requested gateway adds managed runtime policy routing and firewall chains,
not a system-service or partition change. See [WIREGUARD.md](WIREGUARD.md) for
reserved resources, IPv4-only forwarding/IPv6 blocking, the transition guard,
boot/firewall-reload limitations and recovery. The existing agent starts the
feature from its current boot path; no additional init.d/firewall hooks are used.

## Wi-Fi relay

The relay adds `ou-sta0`, agent-owned wpa_supplicant/udhcpc, firewall chains
`OUUP_*`, table `10431` and rules `10429`–`10431`. Those forwarding resources are
runtime state, but saved relay settings and the band guard's committed UCI change
and `/data` restore marker are persistent. The same-band AP stays paused while
associated; disassociation between networks does not immediately give it back.
Inbound upstream traffic is filtered except replies and DHCP offers. Guard-based
restoration, boot grace and backup management are described in
[WIFI-UPLINK.md](WIFI-UPLINK.md).

## Temporary screen ownership

The optional `openui-screen` supervisor may stop `zte_topsw_devui` only after
verified boot synchronization and only for a panel session the owner opened: from
the webpage, with the two-finger hold or four presses on the device, or once per
boot when opening at startup is chosen. It never disables the init service. It
records brightness and touch-service state before stopping UI, and restores on
exit, on idle when the stock UI is the chosen idle action, after fifteen seconds
without agent contact, or on display-child failure. With the default idle action
the session stays, backlight off, until the owner leaves DevUI. The panel is never
left without a UI: the firmware has been seen to reboot itself (reason code 1185)
a few minutes after its UI stopped with nothing in its place
([faying/zte-u60-pro-mu5250-touch-ui](https://github.com/faying/zte-u60-pro-mu5250-touch-ui),
`src/uid.c`). The factory UI resumes its own dim/sleep policy.
No legacy SETCRTC or debugfs image_dump writes are permitted. See [SCREEN.md](SCREEN.md).

## eSIM profile switching

The agent's QMI requests (`agent/src/qmi.rs`) are limited to the UIM service (logical
channels to the eUICC's ISD-R, APDUs on them, slot and card status, SIM power off/on)
and the DMS operating mode. The switch cleanup attempts to power the card back on and return a previously
online radio to online after a failure; it cannot guarantee recovery for every
card, carrier or hardware fault. A restart may be needed to regain mobile service. No firmware daemon is stopped or
restarted, and no reboot happens without the owner choosing it. See [ESIM.md](ESIM.md).
