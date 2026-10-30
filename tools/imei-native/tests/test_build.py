#!/usr/bin/env python3
"""Build metadata/provenance tests only; never execute an ELF/device helper."""
import importlib.util
import json
from pathlib import Path
import shutil
import struct
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("native_build", ROOT / "build.py")
build = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build)


def synthetic_elf():
    """Minimal synthetic ELF container; not executable machine code."""
    data = bytearray(512)
    ident = b"\x7fELF\x02\x01\x01" + bytes(9)
    struct.pack_into("<16sHHIQQQIHHHHHH", data, 0, ident, 3, 183, 1, 0,
                     64, 0, 0, 64, 56, 3, 0, 0, 0)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 4, 0, 0x10000, 0, 512, 512, 4096)
    interpreter = build.INTERPRETER.encode() + b"\0"
    struct.pack_into("<IIQQQQQQ", data, 120, 3, 4, 256, 0x10100, 0,
                     len(interpreter), len(interpreter), 1)
    struct.pack_into("<IIQQQQQQ", data, 176, 2, 6, 320, 0x10140, 0, 80, 80, 8)
    data[256:256 + len(interpreter)] = interpreter
    struct.pack_into("<qQqQqQqQqQ", data, 320, 1, 1, 5, 0x10000 + 416, 10, 9, 0, 0, 0, 0)
    data[416:425] = b"\0libc.so\0"
    return data


class BuildTests(unittest.TestCase):
    def test_pinned_sources(self):
        provenance, sources = build.verified_sources()
        self.assertEqual(provenance["commit"], build.SOURCE_COMMIT)
        self.assertEqual(len(sources), 23)
        self.assertEqual(set(path for path in sources if path.startswith("src/")),
                         {"src/" + name + ".c" for name in build.NAMES}
                         | {"src/plan_io.h", "src/support_config.h"})

    def test_provenance_and_source_tampering_refused(self):
        with tempfile.TemporaryDirectory(prefix="native-source-test-") as directory:
            root = Path(directory)
            provenance = (ROOT / "PROVENANCE.json").read_bytes()
            (root / "PROVENANCE.json").write_bytes(provenance + b" ")
            with patch.object(build, "ROOT", root):
                with self.assertRaisesRegex(ValueError, "provenance hash mismatch"):
                    build.verified_sources()
            (root / "PROVENANCE.json").write_bytes(provenance)
            for entry in json.loads(provenance)["files"]:
                target = root / entry["path"]
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / entry["path"], target)
            (root / "src/zte_nv.c").write_bytes(b"synthetic tampered source")
            with patch.object(build, "ROOT", root):
                with self.assertRaisesRegex(ValueError, "vendor source hash mismatch"):
                    build.verified_sources()

    def test_synthetic_dynamic_musl_elf(self):
        result = build.inspect_elf(synthetic_elf())
        self.assertEqual(result["elf_machine"], 183)
        self.assertEqual(result["needed"], ["libc.so"])
        self.assertEqual(result["interpreter"], build.INTERPRETER)

    def test_bad_headers_architecture_and_static_refused(self):
        for offset, fmt, value in ((0, "<I", 0), (16, "<H", 2), (18, "<H", 62),
                                   (54, "<H", 0), (56, "<H", 65535),
                                   (120, "<I", 0), (176, "<I", 0)):
            with self.subTest(offset=offset, value=value):
                data = synthetic_elf()
                struct.pack_into(fmt, data, offset, value)
                with self.assertRaises(ValueError):
                    build.inspect_elf(data)
        with self.assertRaises(ValueError):
            build.inspect_elf(b"\x7fELF")

    def test_foreign_loader_dependency_rpath_and_bounds_refused(self):
        mutations = ((256, b"/bad"), (417, b"evil.so"))
        for offset, value in mutations:
            data = synthetic_elf()
            data[offset:offset + len(value)] = value
            with self.assertRaises(ValueError):
                build.inspect_elf(data)
        for offset, fmt, value in ((320, "<q", 15), (320, "<q", 29),
                                   (344, "<Q", 0xFFFF), (328, "<Q", 9),
                                   (360, "<q", 1), (368, "<q", 1)):
            with self.subTest(offset=offset, value=value):
                data = synthetic_elf()
                struct.pack_into(fmt, data, offset, value)
                with self.assertRaises(ValueError):
                    build.inspect_elf(data)

    def test_missing_file_and_symlink_build_directory_refused(self):
        with tempfile.TemporaryDirectory(prefix="native-directory-test-") as directory:
            root = Path(directory)
            actual = root / "actual"
            actual.mkdir()
            (root / "link").symlink_to(actual, target_is_directory=True)
            (root / "file").write_text("synthetic file, not a directory")
            for name in ("missing", "link", "file"):
                with self.subTest(name=name), patch.object(build, "BUILD", root / name):
                    with self.assertRaisesRegex(ValueError, "build directory must be a real directory"):
                        build.check_build()

    def test_actual_outputs_when_present_metadata_only(self):
        if not (ROOT / "build/manifest.json").exists():
            self.skipTest("cross build not generated")
        manifest = build.check_build()
        self.assertEqual(set(manifest["helpers"]), set(build.NAMES))
        with tempfile.TemporaryDirectory(prefix="native-manifest-test-") as directory:
            target = Path(directory)
            for name in (*build.NAMES, "manifest.json"):
                shutil.copyfile(ROOT / "build" / name, target / name)
            data = json.loads((target / "manifest.json").read_bytes())
            data["helpers"]["zte_nv"] = "0" * 64
            (target / "manifest.json").write_text(json.dumps(data))
            with patch.object(build, "BUILD", target):
                with self.assertRaisesRegex(ValueError, "binary hash mismatch"):
                    build.check_build()
            shutil.copyfile(ROOT / "build/manifest.json", target / "manifest.json")
            (target / "zte_nv").write_bytes(b"synthetic-not-an-executable")
            with patch.object(build, "BUILD", target):
                with self.assertRaisesRegex(ValueError, "binary hash mismatch"):
                    build.check_build()


if __name__ == "__main__":
    unittest.main(verbosity=2)
