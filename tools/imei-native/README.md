# Guarded MU5250 B31 native helpers

**Development/offline build only. No live helper execution, DIAG session,
IMEI/EFS write, router reboot, or modem reset was performed for this integration.**
A successful build or synthetic test is not proof that a real modem accepts a
write, persists it across reboots, or transmits the intended network-side PEI.

## Provenance and scope

The five files in [src/](src/) and fourteen upstream test files are unchanged
from [SadykovIV/ZTE_U60Pro at the pinned commit](https://github.com/SadykovIV/ZTE_U60Pro/tree/2f7e24c49125ba06d375def56ea5f3e7c1f770aa/MacIMEI/DeviceHelpers).
[PROVENANCE.json](PROVENANCE.json) records their exact upstream paths and hashes,
plus the complete MIT license and reference documentation/build scripts. The
build verifies the pinned provenance hash and every vendored byte before doing
any compilation. No eSIM/ES10/QMI module or vendor binary is included. See
[LICENSES.md](LICENSES.md) for distribution notices.

The channel is the vendor `/usr/lib/libdiag.so.1`, not ubus `set_imei`, AT,
SPC/auth, a UCI display override, or raw partition writes. The helper does not
reboot the router or manage the outer, explicitly confirmed two-reboot workflow.
The shared Rust controller owns model/firmware/library/router/helper gates,
private backups/plans, serialization, consent, recovery and final verification.
Do not expose the native CLI as arbitrary HTTP commands or execute a helper for
`--version`, on page entry, in status polling, or as an installation smoke test.
Even a read opens an MPSS diagnostic session and temporarily changes logging
mode; it is not completely passive.

## Reproducible offline build

Requires Python 3 and an existing **Zig 0.14.1** installation. No download,
package installation, SDK sysroot, device library copy, or device connection is
needed:

```sh
python3 tools/imei-native/build.py --zig /absolute/path/to/zig
python3 tools/imei-native/build.py --check
```

`ZIG=/absolute/path/to/zig` is the equivalent compiler selection. No target,
profile, source hash, or device-path override is accepted. Output is fixed to
`tools/imei-native/build/`; that directory and all host-test outputs are ignored
by git. Do not commit the generated binaries.

The build uses Zig's native `build-exe` driver with `-target
aarch64-linux-musl`, `-mcpu generic`, `-dynamic`, `-fPIE`, and the exact interpreter
`/lib/ld-musl-aarch64.so.1`. It retains C11 warnings-as-errors, stack protection,
RELRO/NOW, stripped debug data, a fixed source epoch and normalized source paths.
**Zig 0.14.1's `cc` compatibility driver produced static PIE in our offline
check even with `-dynamic`; that output was rejected, not packaged.**

The Python ELF parser independently requires ELF64 little-endian, AArch64 machine
183, ET_DYN, exactly the expected PT_INTERP, a valid PT_DYNAMIC, and
DT_NEEDED `libc.so` only. Static/static-PIE, a foreign loader, unexpected library,
RPATH/RUNPATH, malformed table, or mismatching output hash fails the build/check.
Generated helpers use the installed vendor musl loader/libc at runtime; no Zig
musl shared library is deployed. This proves link metadata, not native ABI or
real-device behavior. Two successive builds produced identical helpers and
manifest bytes during offline testing.

## Packaging contract

The generated `build/manifest.json` has schema 1, profile
`mu5250-b31-libdiag-v1`, `source_commit`, `platform`, `interpreter`, the expected
`router_sha256`, `libdiag_sha256`, `musl_sha256`, pinned `provenance_sha256`, all
vendored `source_sha256` entries, binary sizes and ELF metadata. **`helpers` is
an object mapping exactly `zte_nv`, `zte_config`, `zte_config_read` to their
binary SHA-256 strings**, not an array. `build.py --check` validates source,
manifest, bytes and ELF without loading or executing the helpers.

The installer/controller use only:

```
/data/local/tmp/openui-imei-helpers/zte_nv
/data/local/tmp/openui-imei-helpers/zte_config
/data/local/tmp/openui-imei-helpers/zte_config_read
/data/local/tmp/openui-imei-helpers/manifest.json
```

Ship [LICENSE](LICENSE), [LICENSES.md](LICENSES.md) and the retained toolchain
notices with redistributed helpers. An absent/mismatching component must produce
an explicit unavailable/profile error, never a shell, AT, generic NV or UCI
fallback. `manifest.json` is build metadata, not a signature or a new source of
runtime authority: the controller must pin trusted hashes/profile constants and
secure all installation ancestors and files.

[PROFILE.json](PROFILE.json) distinguishes the public source claim from hashes
supplied by the parent after read-only stock CN B31 file checks. These hashes
are not an OEM attestation and are not write-capability evidence. No private
identifier, configuration dump, credential or target value is included.

## Preserved native safeguards and limits

- Indexed NV550 reads/writes are fixed to subscriptions 0/1; legacy access is
  read-only. These indices are **NV records, not asserted physical SIM slots**.
  Only exact NV_NOTACTIVE_S=5 can mean inactive NV1; no record is created.
- Strict HDLC/CRC, request header/index/length and NV status checks; no auth,
  guessed commands, arbitrary NV indices, transport fallback, or guessed ABI.
- Valid 15-digit BCD/Luhn records, exact original topology, legacy alias and all
  128 bytes are checked. Target changes preserve the other 119 bytes. Luhn is
  error detection, not proof of lawful assignment. The controller adds all-zero,
  unchanged-operation, consent and profile restrictions.
- NV plan is exactly 256/512 bytes. EFS config plan is exactly 30,146 bytes with
  independently validated original/candidate B31 layouts and the sole ID102
  flag/CRC change. Unknown file layouts/modes remain unsupported; in particular
  do not broaden support to an unverified `0x8d10` mode.
- Plan files must be absolute, regular, single-link, NOFOLLOW, exact-size and
  unchanged during reading. Native plan I/O does **not** verify owner, 0600 mode
  or ancestor security: the controller must enforce those in private 0700
  transaction directories and never send raw records/plans to the frontend.
- Lost ACK, unknown whole records, topology drift, uncertain EFS siblings and
  failed release must not become success. Recovery only restores known
  original/target records, never unknown content. Native rollback is bounded;
  it is not permission for an outer automatic retry/reboot.
- Normal success requires owned-session release and DeInit. Some upstream early
  session-open/ownership-error exits call DeInit without checking its result;
  this integration does not silently modify that audited code. Such an exit is
  failure with cleanup uncertain, never a successful check or an automatic
  replay authorization. Any behavioral cleanup fix requires independent tests
  and an explicit provenance change.
- Production stdout may contain private full records/config or vendor-library
  output. Capture privately with a bounded process/output limit, parse exact
  schemas, and never log or echo raw stdout/stderr. Only validated public status
  may leave the controller. The separate upstream readonly-support build strips
  raw-value export and writes, but is not an installed production component.

## Offline tests

```sh
python3 tools/imei-native/tests/run_all.py
```

This runs the unchanged upstream host-Clang ASan/UBSan suites, the separate
readonly-support fake-DIAG suites, and local provenance/ELF/manifest tests. Device
mains are either renamed and not called or are compiled with fake DIAG shims;
no real libdiag is loaded. Test identifiers and records are **synthetic**. Test
logs remain in the ignored `tests/.build/` directory.

Coverage includes malformed CRC/header/index responses, exact plan sizes,
BCD/Luhn/tail/distinct-target validation, inactive NV1 vs transport errors,
single/dual topology, lost ACK, guarded inverse restore, unknown-record refusal,
35 config transaction scenarios, strict EFS structure/mode/cleanup handling,
read-only write refusal and output privacy. Local tests reject source/provenance
and binary tampering, malformed/static/wrong-architecture ELF, foreign loaders,
extra dependencies and rpaths. None substitutes for separately authorized
native read, write or reboot verification.
