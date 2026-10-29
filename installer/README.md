# Desktop installer source — development reference only

**This fork has no qualified desktop distribution.** Do not download upstream
Open U60 Pro Installer packages to install this fork: they target a different
product and can overwrite its additions. An inherited GUI, CI package checks or
wireframes do not establish native GUI availability or clean-stock qualification.

For installation or updates use **`bash install.sh` from the repository root**:
[install guide](../docs/INSTALL.md) / [中文教程](../docs/INSTALL.zh.md).
Old GUI/setup tutorials redirect there. Locked B31 first install needs a **USB
DATA cable and network access** to the router, plus the admin password and an
externally obtained backup-key suffix **before installation payload downloads**.
The main shell flow requires interactive `YES` confirmation of the cable, or
explicit `--acknowledge-usb` in noninteractive locked-first-install use. This is
not cable detection; an empty ADB list proves nothing. `--adb-serial` selection
is retained through waiting/deployment. Existing root-SSH updates may use Wi-Fi only. Ask the fork's
[support channel](https://github.com/wgltony/U60Pro-OpenUI/issues) for acquisition
guidance without posting secrets. Simple B31 `strings` extraction and offline
patching tools do not solve obtaining the suffix.

The Tauri/React source is retained for developers. Any future native release must
validate this fork's complete component/repository/version contract and physical
install/recovery paths before being recommended. The commands below build/test
source, not a published or tested desktop installation recommendation. Do not run
modem actions merely to test the UI.

## Safety, recovery and privacy

Read [current safety notes](../docs/SAFETY.md) and
[rollback scope](../docs/DEPLOYMENT.md#rollback-scope-and-limits). Recovery uses
captured state, not a guarantee that all future failures restore exactly; old
snapshots retain only legacy coverage. Unlock restore is not firmware rollback.

Router-local configuration and computer-side recovery material are private.
Encrypted original backups and identity records may be retained by recovery tools;
standalone dry-run/kept work can contain decrypted configuration. The main shell
installer's normal-exit temporary cleanup does not remove retained encrypted
recovery backups. Native app directories, if used in development, also need
private handling. Never publish these files or SSH keys, and do not assume stock
parameter backups contain Tailscale identity. This warning does not assert a leak.

## Validation boundary

Source/unit tests, mock UI/wireframe captures and Windows/macOS CI package/startup
checks are not clean-stock B31 restore, USB enumeration, cold-boot or full rollback
qualification. Physical eSIM/card/carrier operations, relay-band/DFS recovery,
DevUI supervision and Windows first-install USB drivers require separate hardware
evidence. No upstream hardware result certifies this fork's native distribution.

## Build and test

Developer prerequisites: Node.js `^20.19.0 || >=22.12.0`, Rust and the Tauri
platform build prerequisites. From the repository root:

```sh
python3 scripts/prepare-installer-tools.py darwin  # use windows on Windows
npm ci --prefix installer
npm run build --prefix installer
npm test --prefix installer
cargo test -p mu5250-installer -p process-runner --locked
cargo clippy -p mu5250-installer -p process-runner --all-targets --locked -- -D warnings
npm run tauri --prefix installer -- build
```

A native executable launched with `--startup-check <report.json>` performs the
package startup diagnostic and exits; modem commands are disabled in that mode.

The manual installer workflow checks out its explicit `release_tag` for both
checks and builds. A blank tag produces build-only artifacts. Tagged production
releases include checksums for every published download.

## Optional release signing

Unsigned/ad-hoc builds remain supported and must be labelled in release notes.
To enable Apple Developer ID signing and notarization, configure GitHub secrets
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`,
`APPLE_PASSWORD` (app-specific password), and `APPLE_TEAM_ID`. Configured Developer
ID builds require complete notarization credentials and pass Gatekeeper checks.

For certificate-based Windows signing, configure `WINDOWS_CERTIFICATE` (base64
PFX) and `WINDOWS_CERTIFICATE_PASSWORD`. The workflow imports it temporarily,
configures SHA-256 Authenticode signing with timestamping, verifies the setup
signature and removes temporary credentials. Certificates requiring a remote
signing service or hardware token need that provider's integration separately.
Do not put certificates or passwords in source or issue comments.

## Wireframes

[View the v1.1 installer wireframes](../docs/INSTALLER-WIREFRAMES.md). They render
the actual React interface through a separate sample-only bridge, which is not
included in the desktop bundle.

To regenerate, run `npm run wireframes --prefix installer` in one terminal, then
`npm run capture:wireframes --prefix installer` in another. Install Playwright's
Chromium with `npx --prefix installer playwright install chromium` first.
