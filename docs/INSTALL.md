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
- Every deployment keeps a snapshot and restores it by itself when a step fails.
- Nothing personal is stored in this repository. Your passwords, Wi-Fi networks,
  subscriptions and Tailscale identity stay on your router.

**You need:**

- A Mac or Linux computer. A first install from Windows is untested; for updates,
  double-click `deploy-windows.cmd`.
- A USB-C data cable, or the computer on the router's Wi-Fi.
- Tools: `python3`, `openssl`, `adb`, `curl`, `ssh`.
  - macOS: `brew install python openssl android-platform-tools`
  - Debian/Ubuntu: `sudo apt-get install python3 openssl adb curl openssh-client`
- For a locked device only (firmware B28 and later, including B31):
  - the router's **admin password** (the login of `http://192.168.0.1`);
  - the **backup-key suffix** for this device family. It is deliberately not
    published, at the request of the people who found it; ask the community. See
    [ADB.md](ADB.md#the-backup-key-suffix). How ADB enabling works, step by step
    (automatic, script, or fully manual): [ADB.md](ADB.md).

## 1. Get the code

```sh
git clone https://github.com/wgltony/U60Pro-OpenUI.git
cd U60Pro-OpenUI
```

`install.sh` downloads the matching release files from the repository you cloned.

## 2. Connect the router

Plug it into the computer with USB-C, or join its Wi-Fi. Check:

```sh
adb devices
```

- A device listed as `device`: ADB already works, the unlock is skipped.
- Nothing listed: normal for a locked router. `install.sh` will unlock it.

## 3. Try it first (writes nothing)

```sh
bash install.sh --dry-run
```

It checks your computer, downloads and verifies every file, and, when ADB already
works, checks the router's identity and free space. On a locked router it runs the
unlock's own dry run and stops before restoring anything.

## 4. Install

```sh
bash install.sh                    # add --with-tailscale for Tailscale (~60 MB)
```

You are asked for:

1. **A password for the web dashboard.** Pick a new one; it protects
   `http://192.168.0.1:8080` and the agent API.
2. Only on a locked router: the **router admin password** and the **backup-key
   suffix**. The script shows what it is about to restore and asks before doing it;
   the router then reboots and comes back with ADB.

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
`aarch64-unknown-linux-musl` target, Zig 0.14.1 and Node.js 22; the Clash core also needs
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
- **Wi-Fi relay (无线中继)** — use an existing Wi-Fi as the internet uplink. The
  radio can carry the upstream and the hotspot on different bands only: while the
  relay is connected, the hotspot on the upstream's band pauses and the other band
  stays on (both use the same name, so most devices move over by themselves).
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
| A deployment stopped half way | It restores the previous state by itself; if it could not, it printed a recovery command (`sh /data/local/tmp/open-u60-transaction.sh restore …`). Run that command on the router. |
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
