# Deployment — current fork install, update and recovery

Use **`install.sh` from this repository root**. Start with the
[install guide](INSTALL.md) / [中文教程](INSTALL.zh.md) and [safety notes](SAFETY.md).
All host commands below assume the repository root; device-shell commands are
explicitly labelled. Do not `source` device startup scripts to read credentials.

## Supported entrance (not the inherited GUI/setup tutorial)

```sh
bash install.sh --dry-run
bash install.sh                 # optional: --with-tailscale
# An already installed device:
bash install.sh --update
```

The old `setup.sh` and desktop-installer tutorials are superseded by this flow.
Do not use upstream desktop packages: they install a different product and can
overwrite this fork's additions. This fork has **no qualified desktop distribution**;
installer source and CI/package checks do not establish a supported native GUI
release. [Installer development notes](../installer/README.md) are not an end-user
installation recommendation.

## Before downloading payloads

- Target only `MU5250_HW1.0` with `CN_ZTE_MU5250V1.0.0B31`; other combinations
  are unqualified. Prior individual-device observations are not a new clean-stock
  installation qualification of every current release.
- A locked B31 first installation requires a **USB DATA cable** attached to this
  computer **and network access** to the stock web UI. Wi-Fi is useful for that
  network connection; Wi-Fi alone cannot supply post-restore USB ADB. Keep stable
  power and allow the restore/reboot to finish once. Confirm the USB requirement
  before accepting the restore: interactive runs require `YES`, noninteractive
  locked first install requires explicit `--acknowledge-usb`. This is owner
  acknowledgement, not cable detection; an empty ADB list proves nothing.
  `--adb-serial` is preserved through waiting and deployment. An update over working root SSH on port 2222 may
  use Wi-Fi only; `--update` never unlocks a device.
- For a locked device, obtain the admin password and **backup-key suffix before
  downloading installation payloads**. The suffix is an external prerequisite,
  not supplied by this repository. Ask via this fork's
  [support/issues channel](https://github.com/wgltony/U60Pro-OpenUI/issues) for the
  currently available acquisition guidance; do not post secrets or backups there.
  Availability is not guaranteed. If it cannot be obtained, stop before install.
- Simple `strings` extraction does **not** recover the known B31 suffix. Offline
  patching tools also require an already obtained suffix; they do not discover it.
  See [ADB details](ADB.md#the-backup-key-suffix) and the dated
  [B31 investigation](reference/suffix-recovery-b31.md).
- Host tools: Python 3, OpenSSL, Android platform-tools (`adb`), curl, OpenSSH,
  ssh-keygen and tar. Git is needed to clone. Default installation downloads
  verified release payloads; it does not require a Rust/Node build toolchain.

## What changes

The locked-device bootstrap exports a fresh backup, patches only its ADB boot
line, and restores it after confirmation. It changes device settings and reboots;
a successful dry run validates preparation, **not** restore success or cold boot.
For ADB-only/manual restore details use [ADB.md](ADB.md), not an old setup recipe.

The main installation stages and verifies the agent, dashboard, screen, lpac,
Clash core/geo data, WireGuard tool and relay guard; Tailscale is optional.
Hardening installs key-only Dropbear on port 2222 and an isolated dashboard server
on port 8080, removes the temporary ADB boot line, and disables FOTA auto-update.
The stock UI on port 80 is kept separate. Verify SSH before a controlled reboot:
USB should return to stock tethering (observed B31 RNDIS, `PID_1403`), not ADB.

| Service | Location |
| --- | --- |
| Agent | `/data/zte-agent`, LAN port 9090 |
| Dashboard | `/data/bin/dashboard-uhttpd`, `/data/www.current`, port 8080 |
| SSH | `/data/bin/dropbear`, port 2222; `/data/dropbear/` key copy |
| Startup | `/data/local/tmp/start_*.sh`, called from `/etc/rc.local` |
| Relay guard | `/data/bin/relay-band-guard.sh` |
| Optional Tailscale | `/etc/init.d/openui-tailscale`, `/data/bin/tailscale*` |

OpenUI's own optional Tailscale service is not a stock boot-critical service.
Do not add firewall boot hooks or disable vendor sync-barrier daemons. The relay
guard intentionally commits same-band hotspot changes to UCI and keeps a restore
marker on `/data`; see [Wi-Fi relay](WIFI-UPLINK.md). Use opposite-band Wi-Fi or
USB as a backup management path before enabling relay.

## Source builds and advanced deployment

`bash install.sh --from-source` uses **`cargo zigbuild`**, not a musl GCC cross
linker. Install Rust with `aarch64-unknown-linux-musl`, **cargo-zigbuild**, Zig
0.14.1, Python 3, Node.js `^20.19.0 || >=22.12.0` and npm. Go or Docker is needed
to build the Clash core (without either, that core is omitted). See the
[README source commands](../README.en.md#updating).

Advanced component deployments use `python3 scripts/deploy-components.py` from
the root. `--keep-credentials` preserves the installed password/PIN without
executing the startup script; `--skip-unchanged` omits identical components.
Use `--dry-run` first and explicit local artifacts. Legacy wrappers are not the
recommended full-stack install entrance. Windows `deploy-windows.cmd` updates an
already installed SSH-enabled device; **Windows clean-stock first install is
unqualified**. Neither CI nor mock screenshots establish USB driver compatibility.

## Rollback scope and limits

A deployment checks space, locks the device and keeps a private snapshot under
`/data/local/tmp/open-u60-transactions/<id>`. It attempts automatic recovery on
observed deployment/verification failures and prints the exact same-device
recovery command if the connection is lost:

```sh
# On the same router, using the exact id/fingerprint printed by the deployer:
sh /data/local/tmp/open-u60-transaction.sh restore <transaction-id> <identity-fingerprint>
```

Retain snapshots until checked. New snapshots capture FOTA/wireless UCI files, the band-guard restore marker
and original running state of owned Tailscale/guard sidecars. Activation stops
owned old sidecars; recovery stops new sidecars before restoring files/config,
then restarts only those previously running. Ambiguous ownership refuses a kill.
Old snapshots retain **legacy coverage only**, not retroactive capture of missing
state. Driver/CSA state and immediate management-channel continuity are not
guaranteed to return instantly.
The lifecycle regression suite uses an explicit simulated proc/procd/UCI backend;
this is not native B31 or power-loss acceptance. FOTA recovery currently accepts
only an empty-object setter acknowledgement plus the saved UCI value. An unknown
or error reply keeps recovery **pending**, while restoration of other verified
management services is still attempted. Do not treat an unrecognized native
response as successful recovery; its schema needs a separately authorized,
read-only investigation before widening the accepted protocol.
This is deployment recovery, not firmware/partition rollback, not an undo of the
unlock restore, and not a guarantee of exact recovery after every future failure,
power loss or storage damage. Pending/ambiguous records require inspection before
another deployment. See [operations rollback](OPERATIONS.md#rollback).

## Privacy and recovery material

No project-server upload of router configuration is part of this flow. Router
local settings contain your passwords, subscriptions and identities; configure
your own accounts rather than copying an author's environment. The computer also
retains **private encrypted original recovery backups** under
`~/.local/share/open-u60-pro/recovery`, including an identity record. Encryption
does not make these safe to publish. Do not assume a stock backup includes
OpenUI/Tailscale state on `/data`.

Standalone `zunlock.py --dry-run` and `--keep-work` can leave decrypted configuration
archives and patched artifacts in their work directories. Protect them and remove
unneeded material deliberately. The main `install.sh` cleans its temporary work
directory on normal exit (success or failure); original encrypted recovery backups
are retained separately. Abrupt termination can prevent cleanup. Never attach
backups, work directories, SSH keys or suffixes to public issues. This precaution
does not imply a confirmed leak.

## Firmware updates are not routine recovery

FOTA can replace startup hooks, SSH configuration and the suffix; do not enable
it as a troubleshooting shortcut. On the observed B31 preserve list,
`/etc/rc.local` and `/etc/dropbear` were absent while `/data` persisted. Future
firmware may differ: read its real `/sbin/backup_fota.list`, retain private recovery
material and qualify the new build separately. If the suffix rotates, no working
B31 plain-string extraction recipe is known. Stop and seek support rather than
promising re-unlock. Historical evidence: [B31 status](B31-UNLOCK-STATUS.md).

## Validation boundary

Source review, synthetic/offline tests, mocks, hash checks and CI prove their
specific contracts, not a fresh-stock end-to-end hardware qualification. A current
clean-stock B31 restore → USB ADB → SSH/Web → cold-boot → update/failure-recovery
run still needs a separately authorized spare-device acceptance test. Physical
eSIM/card/carrier flows, relay band/DFS recovery, DevUI supervision and Windows
first installation/USB drivers need their own hardware evidence. Do not reset or
flash a working router just to fill these gaps.
