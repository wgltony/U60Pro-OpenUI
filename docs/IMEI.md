# Protected IMEI / NV transactions

[简体中文及完整操作说明](IMEI.zh.md)

**Development feature; not verified on a real modem.** Offline tests and cross-builds do not establish the device ABI, persistent writes, successful restoration or carrier acceptance. No production helper, real IMEI write or reboot was executed during development. Use only on devices you own or are explicitly authorized to service, in accordance with applicable law and carrier terms; never impersonate another device.

## Scope

OpenUI's signal details and DevUI's signal block open the same authenticated, serialized transaction controller. This is not a display-only setter or an unverified AT command. The native DIAG/NV/EFS helpers are derived from the pinned MIT-licensed [reference](../tools/imei-native/README.md); there is no generic NV editor, AT fallback, partition or fuse writer.

The production adapter checks the fixed MU5250 B31 firmware identity, native-library hashes, helper manifest and record/config layouts. Unknown profiles fail closed. Record 0/1 are native record indices, not a promise about physical SIM slots. Secondary editing is disabled unless the secondary record is positively identified.

Targets must be 15 ASCII digits, nonzero and Luhn-valid. Original data must also pass strict validation. Keeping one record is allowed; duplicate/finally unchanged targets are not. A target may not equal the *other* record's original, including swaps: those plans can produce a duplicate crash hybrid that strict recovery would reject. Unknown bytes, malformed originals and corrupt backups are not guessed or repaired.

## Opt-in installation

The published v1.2.2 assets do not contain this feature. The development source has an explicit optional-helper path:

```sh
# Review only first; run from this feature's source checkout.
bash install.sh --from-source --with-imei --update --dry-run
# Installation is a separate owner decision, not an action executed in development.
bash install.sh --from-source --with-imei --update
```

The existing [source-build prerequisites](INSTALL.md) apply, including Zig 0.14.1 and cargo-zigbuild. A dry run may download/build host files; it is not a device ABI check. Helpers can also be built and checked on the host without execution:

```sh
python3 tools/imei-native/build.py
python3 scripts/prepare-imei-helpers.py tools/imei-native/build
```

Manifest checks, licenses and pinned source are described in the [native-tool documentation](../tools/imei-native/README.md). Never substitute an unrelated executable. Deployment and IMEI operations share a durable exclusion lock; pending IMEI recovery blocks installation, update and deployment rollback.

## Explicit sequence

1. **CHECK** performs the explicitly requested read-only native capability check. Opening or polling the page does not probe DIAG or start writes.
2. Enter/keep targets and request a revision-bound, short-lived **preview**.
3. Read the risks and provide a separate **second confirmation**. Original records, original/candidate configuration and a verified private manifest are made durable before configuration mutation.
4. Explicitly request the **first reboot** when allowed. Management connectivity will be lost temporarily.
5. Reconnect and explicitly **continue** the same operation. The controller revalidates identity, boot, records, backups and configuration, then applies the confirmed plan and restores the temporary configuration.
6. Explicitly request the **second reboot**.
7. Reconnect and explicitly **verify**. Only native-record and API readback agreement, restored configuration and the required boot boundary count as success. A queued ACK or HTTP success does not.

Restoration may require additional reboots. There is no automatic reboot, startup resume, page-load write, timeout retry or automatic generation of a secondary number. Client freshness is based on local receipt/elapsed time, not synchronized router/browser wall clocks; stale revisions, expired previews and uncertain results still fail closed.

## Restore default IMEI

The first successful CHECK, while no operation has ever existed, records the observed records once in root-private `/data/openui-imei/default.json`, bound to the modem's cid. It is never rewritten: after any operation the live records may no longer be the default, so a default that was not recorded by then is never guessed. When the live records differ from it, **Restore default IMEI** (screen `uimei restore_default <revision>`, API `{"action":"restore_default","revision":N}`) builds an ordinary preview whose targets are that default, only for records that differ. It writes nothing; Begin, both confirmations and the two reboots still apply. A missing, foreign or unsafe default file only hides the option.

## Recovery and bounded native rollback

Crashes and ambiguous results require a refresh and explicit state-appropriate recovery, not replaying an old request as a new operation. The durable journal binds identity, operation and backup integrity. Known per-record original/target hybrids can be explicitly restored; unknown state is refused. Old unsafe cross-slot plans are rejected without deleting their original backup files.

**Important behavior:** within one explicitly confirmed native `--apply-plan`, the pinned helper may make bounded reverse writes of known originals on some acknowledgement/interruption failures. This is intentional native transaction behavior, not zero additional writes on failure. The outer controller still requires explicit recovery and never retries across crashes, loops indefinitely or reboots automatically.

The audited B31 live configuration modes `0x8d00` and `0x8d20` may change through native recreation/boot. Only that narrow class is compatible; exact preview-source and backup binding remain strict, and other modes are not accepted generically.

## Private data and validation limits

Backups remain under root-private `/data/openui-imei/operations/<operation_id>/`; the current journal is `/data/openui-imei/current.json`. These contain sensitive identifiers and records. Restrictive permissions, checksums, exclusive locking and durable writes are **not encryption**. Do not publish or manually edit them; retain verified private host recovery copies via your trusted encrypted management channel. Losing device storage can still lose backups.

Targets and operation logs are masked. Observed original values may be shown in the authenticated interface; temporary inputs are cleared on exit/terminal state. Root administrators, the browser and the device remain trust boundaries.

Development validation covers synthetic transactions/crashes, fake-DIAG native tests, manifest integrity, API/IPC syntax, both UI previews, builds, deployment exclusion and existing-feature regressions. Real library ABI, DIAG initialization side effects, NV persistence, real restoration, physical power-loss recovery and operator acceptance remain **unverified** and require a separately authorized device acceptance procedure.
