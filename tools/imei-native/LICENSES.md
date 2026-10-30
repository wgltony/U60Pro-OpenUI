# Third-party license and scope notice

The independent native DeviceHelpers and their synthetic C tests are vendored,
byte-for-byte, from **SadykovIV/ZTE_U60Pro**, commit
`2f7e24c49125ba06d375def56ea5f3e7c1f770aa`:

https://github.com/SadykovIV/ZTE_U60Pro/tree/2f7e24c49125ba06d375def56ea5f3e7c1f770aa/MacIMEI/DeviceHelpers

The full upstream MIT license is retained in [LICENSE](LICENSE), including:

- Copyright (c) 2026 ZTE_U60Pro contributors
- Copyright (c) 2026 David Klasens
- Portions derived from jesther-ai/open-u60-pro:
  Copyright (c) 2025-present Jesther Silvestre

[PROVENANCE.json](PROVENANCE.json) records every included upstream path and its
SHA-256. All five files in `src/`, the fourteen original test files, and the
reference build scripts/documentation in `upstream/` are unchanged. No eSIM,
ES10, QMI, backup-tool modules, binary helper, vendor firmware, vendor library,
credential, device identifier, or private device dump is included. The build
outputs only our three executables and their metadata; it does not redistribute
`libdiag.so.1` or a router binary.

The local build/metadata tests and documentation are integration additions. A
future source change must be explicitly reviewed and documented; do not silently
regenerate the pinned provenance to bless a changed guard or protocol.

The pinned Zig 0.14.1 toolchain supplies musl startup objects and compiler
runtime support. Its original notices are also retained in
[licenses/zig-LICENSE.txt](licenses/zig-LICENSE.txt) and
[licenses/musl-COPYRIGHT.txt](licenses/musl-COPYRIGHT.txt); their original toolchain paths
and hashes are in [licenses/TOOLCHAIN.json](licenses/TOOLCHAIN.json). Package
these notices together with this notice and the upstream MIT license when
redistributing the generated helpers. Neither the Zig compiler nor a musl shared
library is included in the deployment payload.
