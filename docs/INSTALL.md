# Install guide — from a stock U60 Pro to OpenUI in one command

[简体中文](INSTALL.zh.md) | English

This guide takes a ZTE U60 Pro from stock firmware to the full OpenUI stack: web
dashboard, touchscreen UI (DevUI), Clash / WireGuard / Tailscale, Wi-Fi relay, eSIM,
USB LAN and the router log. `install.sh` does the work; this page explains each step and what
to do afterwards.

## Before you start

**Tested on exactly this device:**

| Model | ZTE U60 Pro (MU5250) |
| --- | --- |
| Hardware | `MU5250_HW1.0` |
| Firmware | `CN_ZTE_MU5250V1.0.0B31` |

Other firmware or hardware revisions are untested. Read [SAFETY.md](SAFETY.md) once.
In short:

- Enabling ADB on a locked device restores a patched copy of your own settings backup
  and reboots the router (about 90 s without internet). Your settings are kept.
- The install turns off automatic firmware updates (an update could remove the
  unlock), adds SSH on port 2222 (key only) and returns USB to normal tethering after
  the next reboot.
- Every deployment keeps a snapshot and attempts recovery on observed failures.
  New snapshots cover the current UCI/owned-sidecar lifecycle contract; old snapshots
  retain legacy coverage only. This is not guaranteed exact recovery after every
  failure or power loss. See [rollback limits](DEPLOYMENT.md#rollback-scope-and-limits).
- Personal settings are not uploaded to project servers. Router-local configuration
  holds passwords, networks, subscriptions and identities; the computer also retains
  private encrypted original recovery backups and identity records. Standalone
  `zunlock.py --dry-run` / `--keep-work` can keep decrypted configuration. The main
  installer cleans its temporary work on normal success/failure, but retains encrypted
  recovery backups separately. Never publish either; do not assume a stock backup
  includes Tailscale state. See [privacy details](DEPLOYMENT.md#privacy-and-recovery-material).

**You need:**

- A Mac or Linux computer. A first install from Windows is untested; for updates,
  double-click `deploy-windows.cmd`.
- **Locked B31 first install: a USB DATA cable and network access to the router.**
  Wi-Fi is useful for stock-web access; Wi-Fi alone cannot provide USB ADB after
  restore. Existing root-SSH updates may use Wi-Fi only.
- Tools: `python3`, `openssl`, `adb`, `curl`, `ssh`.
  - macOS: `brew install python openssl android-platform-tools`
  - Debian/Ubuntu: `sudo apt-get install python3 openssl adb curl openssh-client`
- For a locked device only (firmware B28 and later, including B31):
  - the router's **admin password** (the login of `http://192.168.0.1`);
  - the **backup-key suffix** for this device family. It is deliberately not
    published. **Obtain it before downloading installation payloads**; ask the fork
    [support/issues channel](https://github.com/wgltony/U60Pro-OpenUI/issues) for
    current acquisition guidance, without posting secrets. If unavailable, stop. See
    [ADB.md](ADB.md#the-backup-key-suffix). How ADB enabling works, step by step
    (automatic, script, or fully manual): [ADB.md](ADB.md).

Simple `strings` extraction does **not** obtain the known B31 suffix. Offline
patching tools also require it and do not solve acquisition. No private author
environment file is provided or required. Source review, mocks and CI are not
current clean-stock B31/cold-boot, physical eSIM or Windows-first-install qualification.
The fork has no qualified desktop distribution; inherited GUI/setup tutorials
redirect to this `install.sh` flow, not upstream desktop packages.

## 1. Get the code

```sh
git clone https://github.com/wgltony/U60Pro-OpenUI.git
cd U60Pro-OpenUI
```

`install.sh` downloads the matching release files from the repository you cloned.

## 2. Connect the router

For locked first install, attach the USB DATA cable **and** establish network
access to the stock web UI (Wi-Fi may be used). Confirm this before any restore;
ADB normally is absent until after reboot. For an SSH-enabled update, Wi-Fi alone
is sufficient. From the repository root, check:

```sh
adb devices
```

- A device listed as `device`: ADB already works, the unlock is skipped.
- Nothing listed: normal for a locked router. `install.sh` will unlock it.

## 3. Try it first (no device writes)

```sh
bash install.sh --dry-run
```

It checks your computer, downloads and verifies every file, and, when ADB already
works, checks the router's identity and free space. On a locked router it runs the
unlock's own dry run and stops before restoring anything. This still creates local backup/work material;
a passed dry run does not prove a subsequent restore or cold boot will succeed.

## 4. Install

```sh
bash install.sh                    # add --with-tailscale for Tailscale (~60 MB)
```

You are asked for:

1. **A password for the web dashboard.** Pick a new one; it protects
   `http://192.168.0.1:8080` and the agent API.
2. Only on a locked router: the **router admin password** and the **backup-key
   suffix**. The script shows what it is about to restore and asks before doing it;
   confirm the USB DATA cable is connected before the restore. The router then
   reboots; USB enumeration/cable/drivers must work for ADB to become reachable.

Before restore, interactive `install.sh` requires `YES` confirming the USB DATA
cable is attached to this computer. Noninteractive locked first install must
explicitly pass `--acknowledge-usb`; the option records your acknowledgement,
not electrical cable detection. An empty `adb devices` list is not proof of a
cable. Use `--adb-serial SERIAL` to preserve a specific selection through ADB
waiting and deployment. Do not acknowledge unless the required cable is connected.

Then, in one deployment:

| Installed | Where |
| --- | --- |
| Agent (API, proxy, relay, USB, Wi-Fi, logs) | `/data/zte-agent`, port 9090 |
| Web dashboard and its server | `http://192.168.0.1:8080` |
| Touchscreen UI | `/data/bin/openui-screen` |
| Clash core (pinned build) and geo data | `/data/bin/mihomo`, `/data/share/mihomo/` |
| eSIM tool (lpac) | `/data/bin/lpac` |
| WireGuard tool | `/data/bin/wg` |
| Relay band guard | `/data/bin/relay-band-guard.sh` |
| Tailscale (with `--with-tailscale`) | `/data/bin/tailscale*`, service `openui-tailscale` |
| SSH (key only) | port 2222, key `~/.ssh/id_ed25519` (created if missing) |

The router's own web UI on port 80 is not touched. Everything starts again after a
reboot.

Prefer to build everything yourself? Add `--from-source` (needs Rust with the
`aarch64-unknown-linux-musl` target, **cargo-zigbuild**, Zig 0.14.1, Python 3,
Node.js `^20.19.0 || >=22.12.0` and npm; the agent uses `cargo zigbuild`, not
an extra musl cross-GCC linker; the Clash core also needs
Go or Docker, and is left out without them).

## 5. Touchscreen check

At the end the script offers a supervised check of the touchscreen UI (about three
minutes): it takes over the screen and tests recovery from idle, crashes and hangs.
Keep the router in view. Until it has passed, the dashboard's **Open DevUI screen**
button stays disabled. To run it later:

```sh
python3 scripts/verify-screen.py --gateway 192.168.0.1 --ssh-key ~/.ssh/id_ed25519 --if-needed
```

## 6. Reboot

Reboot the router when convenient. USB goes back to normal tethering and ADB turns
off; manage the router over SSH (`ssh -p 2222 root@192.168.0.1`) from then on.

## After installing

Open `http://192.168.0.1:8080` and sign in with the password you chose.

- **Proxy** — choose WireGuard, Clash or Tailscale; only one carries traffic at a
  time. Each has a setup guide at the top of its tab:
  - *Clash*: the core and geo data are already installed (**Advanced → Core** updates
    them later). Add your provider's Clash / mihomo subscription URL, choose devices,
    turn it on.
  - *WireGuard*: import the `.conf` file from your VPN provider, choose devices, turn
    it on.
  - *Tailscale* (installed with `--with-tailscale`): sign in with the link shown on
    the page (open it on your phone or computer) or an auth key. Then either turn on
    **remote management** — your tailnet reaches this router and its LAN, alongside
    Clash or WireGuard; approve the `192.168.0.0/24` route once in the Tailscale admin
    console — or use **Tailscale mode** with an exit node.
- **Wi-Fi relay (无线中继)** — use an existing Wi-Fi as the internet uplink. To avoid
  observed AP/STA conflicts, this fork's guard pauses the hotspot on the upstream's
  band while relaying and retains the other band. This is the current policy, not an
  absolute claim about all firmware or hardware capabilities.
  Prepare opposite-band Wi-Fi or USB backup management **before**
  enabling it; do not assume clients roam automatically. The guard commits UCI
  and keeps `/data/local/tmp/relay-band-guard.disabled` across reboot for later
  restoration. See [relay limits](WIFI-UPLINK.md#band-guard-and-management-access).
- **Network → Clients** — connected devices; block or unblock Wi-Fi devices (also on
  the touchscreen).
- **Overview** — USB LAN switch (share the connection over USB).
- **System → Tools → Router logs** — relay, Tailscale and blocklist events, kept on
  the router (about 1 MiB).
- **DevUI** — open the touchscreen UI, choose its menu, startup page and the
  power-button shortcut. See the README's DevUI section.

**Wi-Fi region (optional).** The CN firmware uses China's channel rules. Outside China,
set your country code over SSH and reboot (replace `US`):

```sh
ssh -p 2222 root@192.168.0.1 'uci set wireless.wifi0.country=US; uci set wireless.wifi1.country=US; uci commit wireless'
```

## Updating

```sh
git pull
bash install.sh --update            # keeps the password and PIN, skips unchanged parts
```

`--update` needs SSH on port 2222 or ADB, and never unlocks or re-hardens.

## Troubleshooting

| Problem | What to do |
| --- | --- |
| `The touchscreen is running OpenUI` | Tap **Stock UI (原厂界面)** on the touchscreen and run again, or add `--skip-screen`. |
| `No release found` | The repository has no release yet: add `--from-source`, or `--repo OWNER/NAME`. |
| No ADB three minutes after the unlock | Replug the USB-C cable (use a data cable) and run `adb devices`; then run `bash install.sh` again — it continues where ADB works. |
| Unlock fails | Check the admin password and suffix; run `bash install.sh --dry-run` again. Nothing is restored before its checks pass. |
| Dashboard not reachable after a reboot | `ssh -p 2222 root@192.168.0.1 sh /data/local/tmp/start_dashboard.sh` |
| A deployment stopped half way | It attempts snapshot recovery; if incomplete, it prints a recovery command (`sh /data/local/tmp/open-u60-transaction.sh restore …`). Run that command on the router. |
| Something misbehaves later | **System → Tools → Router logs**, or `/data/local/tmp/openui-logs/` over SSH. |

More detail: [DEPLOYMENT.md](DEPLOYMENT.md) (unlock and deployment internals),
[SCREEN.md](SCREEN.md), [WIFI-UPLINK.md](WIFI-UPLINK.md), [CLASH.md](CLASH.md),
[WIREGUARD.md](WIREGUARD.md).

## For contributors: keep personal data out

`scripts/check-device-secrets.py` (also run in CI) rejects device identities, private
keys, tokens, WireGuard and Tailscale keys. List your own names, SSIDs, accounts and
hosts in `.git/info/personal-patterns` (one per line; the file lives inside `.git` and
is never committed) and add it as a pre-commit hook:

```sh
printf '#!/bin/sh\nexec python3 scripts/check-device-secrets.py\n' > .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit
```
