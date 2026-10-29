# Offline ADB backup patcher (stock B31)

An alternative to `scripts/zunlock.py` for enabling ADB on a stock
**MU5250 / U60 Pro, CN B31** device. See [docs/ADB.md](../../docs/ADB.md) /
[docs/ADB.zh.md](../../docs/ADB.zh.md) for the full method and safety notes.

Where `zunlock.py` logs into the router, patches and (optionally) restores for
you, this tool is **strictly offline**: it never contacts the device, uploads,
restores or reboots. You export a backup yourself, patch it here, inspect it,
and restore it yourself in the stock web UI. Use it when you want to do the
restore by hand and keep every network step out of the tool.

It makes exactly one change — prepending a single boot-time line that brings up
the adbd USB composition — and independently re-verifies that nothing else in
the backup changed before writing anything.

## Requirements

- macOS or Linux, Python 3.10+, `openssl` and `sh` on `PATH`.
- A parameter backup you **just exported from this same stock B31 device** via
  the stock web UI (an encrypted `back_parameter` file).
- The backup-key suffix for this device family (see
  [docs/ADB.md](../../docs/ADB.md#the-backup-key-suffix)); it is not in this repo.

Put the suffix in a private file, mode `600`, that you keep outside the repo:

```json
{ "suffix": "<the suffix>" }
```

## Use

```sh
# 1. Patch the exported backup offline. --out-dir must not already exist.
python3 scripts/offline-adb prepare /path/to/back_parameter \
    --out-dir ./prepared --confirm-stock-b31 --suffix-file /path/to/suffix.private.json
#    You are then prompted (hidden) for the device's 15-digit IMEI.
#    Instead of --suffix-file you may use --password-prompt and enter the full
#    backup password yourself (IMEI + suffix); the IMEI is never a CLI argument.

# 2. Independently re-check the result.
python3 scripts/offline-adb verify ./prepared/back_parameter.original.enc \
    ./prepared/back_parameter.adb.enc --suffix-file /path/to/suffix.private.json
```

A successful run exits 0, prints `"verified": true`, and leaves:

```
prepared/
  back_parameter.original.enc   an exact copy of your export
  back_parameter.adb.enc        the ONLY file to restore
  verification.json             the completion receipt (written last)
```

Then restore `back_parameter.adb.enc` **once** in the stock web UI's parameter
restore page. The stock restore reboots the router and briefly drops the
network; wait for it rather than restoring again. Verify with `adb devices -l`.

The low-level patch/verify primitives also run on an already-decrypted package
(no password, for auditing): `python3 scripts/offline-adb/patcher.py patch IN OUT`
and `python3 scripts/offline-adb/patcher.py verify IN OUT`.

## Safety and scope

- Only for a device **you own** that is genuinely stock CN B31; `--confirm-stock-b31`
  is your attestation, not a detection. A non-stock, already-unlocked or modified
  backup is rejected rather than forced through.
- Use only a backup you just exported from **this** device; restoring a stale or
  foreign backup can roll your settings back.
- The tool never overwrites an existing file or output directory, follows no
  symlinks, requires a `600` secret file, and keeps secrets out of argv and the
  environment. On any error it writes no usable output and prints a fixed code.
- Keep your original unmodified backup, and keep power stable during the restore.

## Tests

Fully synthetic — no device, backup, suffix or IMEI. The real stock fingerprint
(`KNOWN_STOCK_RC_SHA256` in `__main__.py`) guards real-device use; the tests pin
it to the synthetic `stock_rc.local.sample` instead of shipping a real stock file.

```sh
python3 -m unittest discover -s scripts/offline-adb -t scripts/offline-adb -v
```
