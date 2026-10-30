#!/usr/bin/env python3
"""Offline, pinned-Zig build; never run the generated device executables."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
BUILD = ROOT / "build"
NAMES = ("zte_nv", "zte_config", "zte_config_read")
SOURCE_COMMIT = "2f7e24c49125ba06d375def56ea5f3e7c1f770aa"
PROVENANCE_SHA256 = "b01b1241f2d6a66930df6bfc28773136dedaf4932e4eb6d8a8837728c4365b7c"
PROFILE = "mu5250-b31-libdiag-v1"
ROUTER_SHA256 = "55c54f74aaa427940254a2f16c36771e675a80a002363e4f10b0dfcb604d9c6f"
LIBDIAG_SHA256 = "c0fee4f76b302dd7db2a91dcd5bf1a3f771f674b4637f21468d864a789d00392"
MUSL_SHA256 = "d9e8fd8847efd5f63c7ff6476d8654f187be2a48d59d5551eb7c3df9c70e01dd"
INTERPRETER = "/lib/ld-musl-aarch64.so.1"
ZIG_VERSION = "0.14.1"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def verified_sources():
    """Verify every byte of the reviewed upstream files, including tests/license."""
    data = (ROOT / "PROVENANCE.json").read_bytes()
    if sha256(data) != PROVENANCE_SHA256:
        raise ValueError("pinned provenance hash mismatch")
    provenance = json.loads(data)
    if provenance["commit"] != SOURCE_COMMIT or provenance["schema"] != 1:
        raise ValueError("unsupported provenance")
    result = {}
    for entry in provenance["files"]:
        relative = Path(entry["path"])
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("invalid provenance path")
        path = ROOT / relative
        if path.is_symlink() or not path.is_file():
            raise ValueError("missing/nonregular vendor source: " + entry["path"])
        if sha256(path.read_bytes()) != entry["sha256"]:
            raise ValueError("vendor source hash mismatch: " + entry["path"])
        result[entry["path"]] = entry["sha256"]
    return provenance, result


def inspect_elf(data):
    """Check ELF metadata only. Never load or execute foreign code."""
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("expected ELF64 little-endian")
    header = struct.unpack_from("<16sHHIQQQIHHHHHH", data)
    if header[1] != 3 or header[2] != 183 or header[3] != 1:
        raise ValueError("expected AArch64 ET_DYN executable")
    offset, entry_size, count = header[5], header[9], header[10]
    if entry_size != 56 or not 1 <= count <= 64 or offset + count * 56 > len(data):
        raise ValueError("invalid program headers")
    headers = [struct.unpack_from("<IIQQQQQQ", data, offset + i * 56) for i in range(count)]
    for ph in headers:
        if ph[2] + ph[5] > len(data):
            raise ValueError("program header outside file")
    interpreters = [data[ph[2]:ph[2] + ph[5]] for ph in headers if ph[0] == 3]
    if interpreters != [INTERPRETER.encode() + b"\x00"]:
        raise ValueError("wrong or missing dynamic musl interpreter")
    dynamic = [ph for ph in headers if ph[0] == 2]
    if len(dynamic) != 1 or dynamic[0][5] % 16:
        raise ValueError("missing/malformed dynamic section (static build rejected)")
    ph = dynamic[0]
    tags = []
    terminated = False
    for pos in range(ph[2], ph[2] + ph[5], 16):
        tag, value = struct.unpack_from("<qQ", data, pos)
        if tag == 0:
            terminated = True
            break
        if tag in (15, 29):
            raise ValueError("unexpected RPATH/RUNPATH")
        tags.append((tag, value))
    if not terminated:
        raise ValueError("unterminated dynamic section")
    strtabs = [value for tag, value in tags if tag == 5]
    sizes = [value for tag, value in tags if tag == 10]
    if len(strtabs) != 1 or len(sizes) != 1 or sizes[0] > len(data):
        raise ValueError("invalid dynamic string table")
    mappings = [ph for ph in headers if ph[0] == 1 and ph[3] <= strtabs[0]
                and strtabs[0] + sizes[0] <= ph[3] + ph[5]]
    if len(mappings) != 1:
        raise ValueError("unmapped dynamic string table")
    start = mappings[0][2] + strtabs[0] - mappings[0][3]
    strings = data[start:start + sizes[0]]
    needed = []
    for tag, value in tags:
        if tag != 1:
            continue
        if value >= len(strings):
            raise ValueError("invalid DT_NEEDED offset")
        end = strings.find(b"\x00", value)
        if end < 0:
            raise ValueError("unterminated DT_NEEDED")
        needed.append(strings[value:end].decode("ascii"))
    if needed != ["libc.so"]:
        raise ValueError("unexpected dynamic dependencies: " + repr(needed))
    # Fixed libdiag remains a dlopen dependency: the vendor musl loader must own libc.
    return {"elf_class": 64, "elf_machine": 183, "elf_type": "ET_DYN",
            "interpreter": INTERPRETER, "needed": needed}


def check_build():
    """Validate current outputs against source provenance and their manifest."""
    _, sources = verified_sources()
    if BUILD.is_symlink() or not BUILD.is_dir():
        raise ValueError("build directory must be a real directory")
    path = BUILD / "manifest.json"
    if path.is_symlink():
        raise ValueError("manifest must not be a symlink")
    manifest = json.loads(path.read_bytes())
    expected = {"schema": 1, "profile": PROFILE, "source_commit": SOURCE_COMMIT,
                "router_sha256": ROUTER_SHA256, "libdiag_sha256": LIBDIAG_SHA256,
                "musl_sha256": MUSL_SHA256,
                "platform": "aarch64-linux-musl", "interpreter": INTERPRETER,
                "provenance_sha256": PROVENANCE_SHA256, "source_sha256": sources,
                "compiler": "zig " + ZIG_VERSION, "private_device_data_embedded": False}
    for key, value in expected.items():
        if manifest.get(key) != value:
            raise ValueError("manifest metadata mismatch: " + key)
    if set(manifest.get("helpers", {})) != set(NAMES):
        raise ValueError("manifest helper set mismatch")
    for name in NAMES:
        binary = BUILD / name
        if binary.is_symlink() or not binary.is_file():
            raise ValueError("missing/nonregular helper: " + name)
        data = binary.read_bytes()
        if sha256(data) != manifest["helpers"][name]:
            raise ValueError("binary hash mismatch: " + name)
        if len(data) != manifest["binary_sizes"][name]:
            raise ValueError("binary size mismatch: " + name)
        if inspect_elf(data) != manifest["elf"][name]:
            raise ValueError("ELF metadata mismatch: " + name)
    return manifest


def build(zig):
    provenance, sources = verified_sources()
    compiler = shutil.which(zig)
    if not compiler:
        raise ValueError("Zig not found; use --zig /absolute/path/to/zig")
    version = subprocess.check_output([compiler, "version"], text=True, timeout=10).strip()
    if version != ZIG_VERSION:
        raise ValueError("Zig " + ZIG_VERSION + " required; found " + version)
    if BUILD.is_symlink() or (BUILD.exists() and not BUILD.is_dir()):
        raise ValueError("build directory must be a real directory")
    BUILD.mkdir(exist_ok=True)
    for name in (*NAMES, "manifest.json"):
        destination = BUILD / name
        if destination.is_symlink() or (destination.exists() and not destination.is_file()):
            raise ValueError("invalid output path: " + name)
    env = dict(os.environ, SOURCE_DATE_EPOCH=str(provenance["source_date_epoch"]),
               LC_ALL="C", TZ="UTC", ZIG_GLOBAL_CACHE_DIR=str(BUILD / ".zig-global-cache"),
               ZIG_LOCAL_CACHE_DIR=str(BUILD / ".zig-cache"))
    helpers, sizes, elf = {}, {}, {}
    with tempfile.TemporaryDirectory(prefix=".stage-", dir=BUILD) as stage:
        stage = Path(stage)
        for name in NAMES:
            output = stage / name
            # Zig 0.14.1's cc compatibility driver silently selects static PIE
            # for this musl target even with -dynamic. The native driver honors it.
            command = [compiler, "build-exe", "-target", "aarch64-linux-musl", "-mcpu", "generic",
                       "-dynamic", "--dynamic-linker", INTERPRETER, "-fPIE", "-fstrip",
                       "-OReleaseSmall", "-z", "relro", "-z", "now", "--build-id=none",
                       "-lc", "-ldl", "-lpthread", "-femit-bin=" + str(output),
                       "-cflags", "-std=c11", "-Os", "-Wall", "-Wextra", "-Werror",
                       "-fstack-protector-strong", "-pthread",
                       "-ffile-prefix-map=" + str(ROOT) + "=tools/imei-native",
                       "-fmacro-prefix-map=" + str(ROOT) + "=tools/imei-native",
                       "--", "src/" + name + ".c"]
            subprocess.run(command, cwd=ROOT, env=env, check=True, timeout=300)
            data = output.read_bytes()
            elf[name] = inspect_elf(data)
            helpers[name], sizes[name] = sha256(data), len(data)
        manifest = {"schema": 1, "profile": PROFILE, "source_commit": SOURCE_COMMIT,
                    "router_sha256": ROUTER_SHA256, "libdiag_sha256": LIBDIAG_SHA256,
                "musl_sha256": MUSL_SHA256,
                    "platform": "aarch64-linux-musl", "interpreter": INTERPRETER,
                    "compiler": "zig " + version,
                    "upstream": {"repository": provenance["repository"], "commit": SOURCE_COMMIT,
                                 "license": provenance["license"]},
                    "provenance_sha256": PROVENANCE_SHA256, "source_sha256": sources,
                    "private_device_data_embedded": False, "helpers": helpers,
                    "binary_sizes": sizes, "elf": elf}
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        for name in NAMES:
            os.chmod(stage / name, 0o755)
            os.replace(stage / name, BUILD / name)
        os.replace(stage / "manifest.json", BUILD / "manifest.json")
    check_build()
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zig", default=os.environ.get("ZIG", "zig"), help="Zig 0.14.1 executable")
    parser.add_argument("--check", action="store_true", help="verify existing outputs without compiling")
    args = parser.parse_args()
    try:
        manifest = check_build() if args.check else build(args.zig)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, "native build failed: " + str(error) + "\n")
    print(json.dumps({"profile": PROFILE, "manifest": "build/manifest.json",
                      "helpers": manifest["helpers"], "executed_device_helpers": False}, sort_keys=True))


if __name__ == "__main__":
    main()
