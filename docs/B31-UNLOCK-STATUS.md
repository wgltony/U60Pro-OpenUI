# B31 unlock — current status and qualification limits

Use the current [install.sh guide](INSTALL.md) / [中文教程](INSTALL.zh.md), not an
old GUI or `setup.sh` tutorial. The [2026-10-09/10 status record](archive/b31-unlock-status-2026-10-09.md)
is archived as history: its “unconfirmed” conclusions, private-author environment
assumptions and old commands are not current prerequisites or instructions.

## What is known

- The web USB-debug toggle was removed in CN B28 and is unavailable on B31.
- A 2026-10-10 record reports an individual CN B31 unlock, agent/SSH/dashboard
  deployment and reboot-persistence check. That observation does not qualify every
  current release on a clean-stock device, other firmware or hardware.
- The current source implements the fresh-config backup → one-line ADB boot patch
  → confirmed restore flow. Use it only on your own matching `MU5250_HW1.0` /
  `CN_ZTE_MU5250V1.0.0B31` device. A dry run checks preparation/decryption, not
  restore success, USB enumeration or cold boot.
- Locked first install needs a **USB DATA cable and network access** to the router.
  Wi-Fi is useful for network access but cannot replace post-restore USB ADB.
  Existing root-SSH updates may use Wi-Fi only. Before restore, interactive
  `install.sh` requires `YES`, and noninteractive runs require explicit
  `--acknowledge-usb`; an empty ADB list is not evidence of a connected cable.
  An explicit `--adb-serial` selection is kept for ADB waiting/deployment.

## External prerequisite: obtain the suffix first

The backup-key suffix is deliberately not published or embedded in the repository.
No private author environment file is needed or provided. You must obtain the
appropriate suffix through an external support/community channel **before
installation payload downloads**. The fork's
[support/issues channel](https://github.com/wgltony/U60Pro-OpenUI/issues) can be used
to ask for current acquisition guidance, not to post suffixes, identities or
backups. Neither availability nor a response is guaranteed; stop if unavailable.
Use the hidden prompt or `ZTE_BACKUP_SUFFIX`; never paste the value into an issue.

The known B31 suffix is **not recoverable by simple `strings` extraction** from
`/usr/bin/zte_web`; the investigation also found no contiguous/single-byte-XOR
value in the examined candidates. This is not a substitute suffix-acquisition
recipe. [Dated investigation](reference/suffix-recovery-b31.md). Offline patching
tools require the suffix too and do not solve obtaining it. Rotation in a future
firmware may leave no known re-unlock path.

## Privacy, recovery and remaining acceptance work

The computer retains private encrypted original recovery backups and identity
records. Standalone unlock dry runs/kept work can contain decrypted configuration;
the main installer's temporary cleanup is a separate mechanism. Router-local
configuration is not the only private material. Do not share these directories;
no leak is asserted and native backup coverage of Tailscale state is not assumed.
See [privacy details](DEPLOYMENT.md#privacy-and-recovery-material).

New transaction snapshots have the current helper's UCI/owned-sidecar recovery
coverage; older snapshots only cover legacy state. Neither those snapshots nor a
passing mock/CI run guarantee exact recovery after every failure, firmware update
or power interruption. See [Deployment](DEPLOYMENT.md#rollback-scope-and-limits).

Still requires separate authorization and hardware evidence: a current clean-stock
B31 restore → USB ADB → SSH/Web → cold-boot → update/failure-recovery sequence,
physical eSIM/card/carrier flows, relay band/DFS recovery, DevUI supervision and
Windows first-install/driver qualification. Do not reset or flash the working
development router to obtain that evidence.
