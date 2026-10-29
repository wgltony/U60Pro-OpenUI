# Enabling ADB on stock B31

English | [简体中文](ADB.zh.md)

How to re-enable USB ADB on a **ZTE U60 Pro (MU5250) running CN stock B31**.
`install.sh` does this automatically for a locked device; this page explains the
method and gives three paths, from fully automatic to fully manual.

> Only for a device **you own** whose model and firmware match. Read the
> [safety notes](SAFETY.md) first.

## Why it is needed

Firmware B28 and later (including B31) removed the USB-debug toggle
(`zwrt_bsp.usb.set` / `zwrt_bsp.usb.op`) from the web daemon, so there is no web
switch to turn ADB on. CN B27 and earlier still have it and can be enabled
directly, without the steps here.

What still works is the **config backup / restore** path:

- the device's parameter backup is an openssl-encrypted tar of the system config
  and startup scripts;
- the restore process runs as **root** and extracts whatever you give it.

So the flow is the one you remember: **export a backup → insert one line that
enables ADB at boot → restore it → the device reboots → ADB is on**.

## How it works (what gets inserted)

The startup script `etc/rc.local` reads a USB-mode node. The tool adds a single
**line** near the top of that script to switch USB to the debug composition
(adbd) at boot, leaving every original byte after it untouched:

```sh
echo 1 > /sys/class/android_usb/android0/usb_op
```

> That line is written into the **router's startup script**, not run in your
> computer's terminal. The sysfs path is read from the device's own stock
> `rc.local` (not hardcoded). Never write that node by hand at runtime — a live
> write can kill the USB gadget until a reboot.

The tool then repacks in the device's own format (inner tgz → recompute md5 →
outer tgz → re-encrypt) so it passes the restore-time checksum. The backup is
the one you exported seconds earlier, so nothing but that one line changes.

## The backup-key suffix

The backup's encryption password is `<device IMEI><suffix>`:

- the **IMEI** is per-device; the tool reads it from the device, you never type
  it (and should never paste it into chat or a command line);
- the **suffix** is a fixed string shared across this ZTE generation. At the
  request of the researchers who found it, it is **not published** (publishing it
  gets it patched out in the next firmware), and it is not in this repository.
  To obtain it:
  - ask the community;
  - or extract it yourself from a rooted same-generation (SDX75) ZTE MBB unit:
    the web server builds the backup password in memory, and the suffix is
    visible in the strings near its backup/restore code paths.

Provide it with `--suffix`, the `ZTE_BACKUP_SUFFIX` env var, or the interactive
(hidden) prompt. It never enters this repository.

## Path 1: let `install.sh` do it (easiest)

Most people use this. When it detects a locked device (no ADB, no SSH),
`install.sh` runs the whole unlock automatically and then installs OpenUI.

```sh
bash install.sh --dry-run     # check only, write nothing to the device
bash install.sh               # real run; shows what it will restore and asks first
```

It prompts for the router admin password and the backup-key suffix, runs the
unlock tool's own dry run, and only then restores. The device reboots; about
60–90 s later ADB is on and the install continues. See the
[install guide](INSTALL.md).

## Path 2: ADB only, with the `zunlock.py` script

To enable ADB without installing OpenUI yet, run the unlock script on its own:

```sh
python3 scripts/zunlock.py --dry-run          # full flow, just no upload (safe)
python3 scripts/zunlock.py                     # real run, confirms before upload
```

What it does:

1. logs into the web UI with the admin password;
2. requests and downloads a fresh parameter backup;
3. decrypts it (`openssl enc -d -des-ede3-cbc -md sha256`, password `<IMEI><suffix>`);
4. inserts the line above into `etc/rc.local` (after the shebang), preserving
   file modes and ownership;
5. repacks and re-encrypts in the device's format;
6. after confirmation, uploads it (`/cgi-bin/cgi-upload`, verifying the server's
   sha256) and triggers `device_restore_proc` — the device restores and reboots;
7. about 60–90 s later `adb devices` lists the unit and `adb shell` is a root shell.

Give the suffix with `--suffix` or `ZTE_BACKUP_SUFFIX`; `--keep-work DIR` keeps
the intermediate artifacts.

## Path 3: fully offline, restore it yourself (most cautious)

If you would rather the script not upload and trigger the restore for you, have
it produce only the **patched backup file** and do the restore yourself in the
stock web UI:

```sh
python3 scripts/zunlock.py --dry-run --keep-work ./unlock-work
```

- it still logs in, downloads and patches locally, but **does not upload, trigger
  a restore, or reboot**;
- the patched backup is written to `./unlock-work/back_parameter.patched`.

Then restore it yourself:

1. confirm the backup is from **this device, its current config, stock B31**;
2. in the stock web UI's parameter backup/restore page, restore
   `back_parameter.patched` **once**;
3. **the restore reboots the router and briefly drops the network.** Do not
   repeat the restore just because the page disconnects — wait for the reboot,
   then check.

This path separates "which line changes" from "when it is written back," so you
can inspect the patch and perform the restore by hand.

For a variant that never contacts the device at all — you export the backup
yourself and patch that local file offline — see
[scripts/offline-adb/](../scripts/offline-adb/README.md).

## Verify

Connect a data-capable USB cable, install Android platform-tools, then:

```sh
adb devices -l
```

Once the device shows up (replace the serial with the one actually listed):

```sh
adb -s LISTED_SERIAL shell id
```

- only a `device` state means ADB works;
- `unauthorized` needs authorization on the device; `offline` or an empty list
  points to the cable, driver or USB enumeration first, not necessarily a failed
  restore;
- judge root by what `id` returns, not by a passing file check.

## Safety

- The restore reboots the device and briefly drops connectivity (~90 s); keep
  power stable.
- **Never** write the `usb_op` node by hand at runtime (it can disable the USB
  gadget until a reboot).
- **Never** experiment with A/B slot switching (`abctl --set_active`); a
  mixed-slot boot can brick the unit.
- Keep an **unmodified original backup**; use only a backup you just exported
  from **this device**, never someone else's, an older firmware's, or one of
  unknown origin.
- ADB at boot exposes a debug interface after every reboot; connect only trusted
  computers and never expose the debug port to the public internet.
- After installing OpenUI, prefer the key-only SSH on port 2222; a reboot returns
  USB to normal tethering and turns ADB off.

For USB modes, restore and rollback after OpenUI is installed, see
[DEPLOYMENT.md](DEPLOYMENT.md).

## Credits

- [MlgmXyysd/openadb_MU5250](https://github.com/MlgmXyysd/openadb_MU5250): public
  research on enabling ADB on the MU5250.
- [jesther-ai/open-u60-pro](https://github.com/jesther-ai/open-u60-pro): the
  device access work this project builds on.
