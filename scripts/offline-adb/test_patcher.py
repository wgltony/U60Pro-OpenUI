"""Synthetic archives ONLY. No router, credentials or real backup access."""
from __future__ import annotations

import copy
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import archive_guard as guard
from archive_guard import INNER, SIDECAR, TARGET, Rejected, metadata, pack, unpack
from patcher import patch_package, patch_rc
from verify import verify_package

HERE = Path(__file__).resolve().parent
NODE = b"/sys/class/android_usb/android0/usb_op"
RC = (b"#!/bin/sh\n# synthetic fixture, not a router export\n"
      b"flag=$(cat " + NODE + b")\n"
      b'if [ "$flag" = 0 ]; then echo protected; fi\nexit 0')

STOCK_COMMENT = b"# Put your custom commands here that should be executed once"
STOCK_RC = STOCK_COMMENT + b"\n" + RC.partition(b"\n")[2]


def member(name, data=b"", kind=tarfile.REGTYPE, *, link="", pax=None):
    info = tarfile.TarInfo(name)
    info.type = kind
    info.size = len(data) if kind in {tarfile.REGTYPE, tarfile.AREGTYPE} else 0
    info.mode = 0o751 if name == TARGET else 0o640
    info.uid, info.gid = 17, 29
    info.uname, info.gname = "fixture-user", "fixture-group"
    info.mtime = 1700000000.125
    info.linkname = link
    info.pax_headers = {"fixture.annotation": "preserve", **(pax or {})}
    return info, data


def archive(entries, global_pax=None):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w:", format=tarfile.PAX_FORMAT,
                      pax_headers=global_pax) as output:
        for info, data in entries:
            output.addfile(info, io.BytesIO(data) if info.isreg() else None)
    return gzip.compress(stream.getvalue(), mtime=123)


def fixture(rc=RC, *, inner_extra=(), outer_extra=(), ending=b"\n", uppercase=False,
            rc_kind=tarfile.REGTYPE, rc_pax=None, sidecar=None, global_pax=None):
    base_inner = [member("etc", kind=tarfile.DIRTYPE),
                  member(TARGET, rc, rc_kind,
                         link="etc/unchanged" if rc_kind == tarfile.LNKTYPE else "",
                         pax=rc_pax),
                  member("etc/unchanged", b"\x00\xffUNCHANGED\r\n"),
                  *inner_extra]
    inner = archive(base_inner, global_pax)
    digest = hashlib.md5(inner).hexdigest().encode()
    if uppercase:
        digest = digest.upper()
    checksum = sidecar if sidecar is not None else digest + ending
    return archive([member("tmp", kind=tarfile.DIRTYPE), member(INNER, inner),
                    member(SIDECAR, checksum),
                    member("extras", kind=tarfile.DIRTYPE),
                    member("extras/blob.bin", b"\x00\x01\xfe\xffall bytes\r\n"),
                    member("extras/link", kind=tarfile.SYMTYPE, link="blob.bin"),
                    member("extras/hard", kind=tarfile.LNKTYPE, link="extras/blob.bin"),
                    *outer_extra], global_pax)


def rewrite_outer(blob, *, inner=None, checksum=None, other=None):
    old = unpack(blob)
    changes = dict(other or {})
    if inner is not None:
        changes[INNER] = inner
        changes[SIDECAR] = hashlib.md5(inner).hexdigest().encode() + b"\n"
    if checksum is not None:
        changes[SIDECAR] = checksum
    return pack(old, changes)


class PatcherTests(unittest.TestCase):
    def assert_rejected(self, operation, *args):
        with self.assertRaises(Rejected):
            operation(*args)

    def test_minimal_preserves_bytes_members_order_metadata_and_links(self):
        original = fixture(global_pax={"fixture.global": "unchanged"})
        patched = patch_package(original)
        report = verify_package(original, patched)
        self.assertTrue(report["verified"])
        old, new = unpack(original), unpack(patched)
        self.assertEqual([e.info.name for e in old.entries], [e.info.name for e in new.entries])
        for before, after in zip(old.entries, new.entries):
            self.assertEqual(metadata(before.info, omit_size=True),
                             metadata(after.info, omit_size=True))
            if before.info.name not in {INNER, SIDECAR}:
                self.assertEqual(before.data, after.data)
                self.assertEqual(metadata(before.info), metadata(after.info))
        new_rc = unpack(new.require_regular(INNER).data).require_regular(TARGET).data
        self.assertEqual(new_rc, RC[:10] + b"echo 1 > " + NODE + b"\n" + RC[10:])
        self.assertIn(b"flag=$(cat " + NODE + b")", new_rc)
        self.assertNotIn(b"usb_op)", new_rc.split(b"\n")[1])
        self.assertFalse(new_rc.endswith(b"\n"))  # preserve missing final newline

    def test_exact_stock_comment_prefix_prepends_one_line(self):
        for newline in (b"\n", b"\r\n"):
            stock_rc = STOCK_RC.replace(b"\n", newline)
            original = fixture(rc=stock_rc)
            patched = patch_package(original)
            verify_package(original, patched)
            new_rc = unpack(unpack(patched).require_regular(INNER).data).require_regular(TARGET).data
            self.assertEqual(new_rc, b"echo 1 > " + NODE + newline + stock_rc)
            self.assertEqual(new_rc.partition(b"\n")[2], stock_rc)
            self.assertIn(b"flag=$(cat " + NODE + b")", new_rc)

    def test_stock_comment_tamper_reordering_missing_line_rejected(self):
        original = fixture(rc=STOCK_RC)
        good = patch_package(original)
        inner = unpack(unpack(good).require_regular(INNER).data)
        rc = inner.require_regular(TARGET).data
        insertion = b"echo 1 > " + NODE + b"\n"
        for bad in (rc.replace(b"protected", b"modified"),
                    STOCK_COMMENT + b"\n" + insertion + STOCK_RC.partition(b"\n")[2],
                    STOCK_RC, insertion + STOCK_RC + b"\necho extra\n",
                    insertion.replace(b"\n", b"\r\n") + STOCK_RC):
            bad_inner = pack(inner, {TARGET: bad})
            self.assert_rejected(verify_package, original, rewrite_outer(good, inner=bad_inner))

    def test_unknown_no_shebang_prefixes_rejected_by_both_implementations(self):
        for prefix in (b"# arbitrary comment\n", b" " + STOCK_COMMENT + b"\n",
                       STOCK_COMMENT + b" \n", b"\n" + STOCK_COMMENT + b"\n",
                       STOCK_COMMENT.replace(b"once", b"twice") + b"\n", b""):
            unknown = prefix + RC.partition(b"\n")[2]
            original = fixture(rc=unknown)
            self.assert_rejected(patch_package, original)
            inside = unpack(unpack(original).require_regular(INNER).data)
            alleged = pack(inside, {TARGET: b"echo 1 > " + NODE + b"\n" + unknown})
            self.assert_rejected(verify_package, original, rewrite_outer(original, inner=alleged))
        self.assert_rejected(patch_package, fixture(rc=STOCK_COMMENT))
        self.assert_rejected(patch_package, fixture(rc=STOCK_COMMENT + b"\nexit 0\n"))

    def test_usb_node_must_be_sysfs_for_shebang_and_stock_branches(self):
        for old_rc in (RC, STOCK_RC):
            for outside in (b"/tmp/test/usb_op", b"/sys-other/test/usb_op"):
                bad_rc = old_rc.replace(NODE, outside)
                original = fixture(rc=bad_rc)
                self.assert_rejected(patch_package, original)
                old_inner = unpack(unpack(original).require_regular(INNER).data)
                insertion = b"echo 1 > " + outside + b"\n"
                if bad_rc.startswith(b"#!"):
                    first, _, rest = bad_rc.partition(b"\n")
                    alleged_rc = first + b"\n" + insertion + rest
                else:
                    alleged_rc = insertion + bad_rc
                alleged_inner = pack(old_inner, {TARGET: alleged_rc})
                self.assert_rejected(verify_package, original,
                                     rewrite_outer(original, inner=alleged_inner))

    def test_crlf_exact_and_non_utf8_original_bytes(self):
        old_rc = RC.replace(b"\n", b"\r\n") + b"\r\n# \xff\xfe\r\n"
        original = fixture(rc=old_rc, ending=b"\r\n")
        patched = patch_package(original)
        new_rc = unpack(unpack(patched).require_regular(INNER).data).require_regular(TARGET).data
        first, _, rest = old_rc.partition(b"\n")
        self.assertEqual(new_rc, first + b"\n" + b"echo 1 > " + NODE + b"\r\n" + rest)
        verify_package(original, patched)

    def test_all_digest_layouts_and_case(self):
        for ending in (b"", b"\n", b"\r\n"):
            for upper in (False, True):
                with self.subTest(ending=ending, upper=upper):
                    original = fixture(ending=ending, uppercase=upper)
                    patched = patch_package(original)
                    verify_package(original, patched)
                    checksum = unpack(patched).require_regular(SIDECAR).data
                    self.assertEqual(len(checksum), 32 + len(ending))
                    self.assertTrue(checksum.endswith(ending))
                    self.assertEqual(checksum[:32], checksum[:32].upper() if upper
                                     else checksum[:32].lower())

    def test_nonhardcoded_path_and_repeated_same_node(self):
        rc = RC.replace(NODE, b"/sys/devices/platform/usb-switch.2/usb_op")
        rc += b"\n# same node /sys/devices/platform/usb-switch.2/usb_op\n"
        original = fixture(rc=rc)
        patched = patch_package(original)
        verify_package(original, patched)
        self.assertIn(b"echo 1 > /sys/devices/platform/usb-switch.2/usb_op\n", patch_rc(rc))

    def test_unique_node_rejected(self):
        self.assert_rejected(patch_package, fixture(rc=RC + b"\ncat /other/node/usb_op\n"))
        self.assert_rejected(patch_package, fixture(rc=RC.replace(b"usb_op", b"not_usb")))

    def test_path_boundaries_and_traversal_rejected(self):
        for node in (b"/sys/../usb_op", b"/sys/./usb_op", b"/sys//usb_op",
                     b"/sys/a/usb_op.evil", b"/sys/a/usb_op/child", b"/sys/a$/usb_op",
                     b"/sys/a\\/usb_op", b"/sys/a/usb_op$tail"):
            with self.subTest(node=node):
                self.assert_rejected(patch_package, fixture(rc=RC.replace(NODE, node)))

    def test_dynamic_path_suffix_not_mistaken_for_literal_node(self):
        for node in (b"/sys/$(choose)/android0/usb_op", b'/sys/"dynamic"/node/usb_op'):
            self.assert_rejected(patch_package, fixture(rc=RC.replace(NODE, node)))

    def test_shebang_and_nul_rejected(self):
        for bad in (RC.replace(b"#!/bin/sh", b"#!/bin/bash"), b"\xef\xbb\xbf" + RC,
                    RC.replace(b"#!/bin/sh", b"#!/bin/sh -e"), b"#!/bin/sh",
                    RC + b"\0"):
            self.assert_rejected(patch_package, fixture(rc=bad))

    def test_existing_payload_rejected_anywhere(self):
        for payload in (b"echo 1 > " + NODE, b" echo 1>" + NODE,
                        b"echo 1 > '" + NODE + b"'", b'echo 1 > "' + NODE + b'"; exit 0'):
            self.assert_rejected(patch_package, fixture(rc=RC + b"\n" + payload + b"\n"))
        once = patch_package(fixture())
        self.assert_rejected(patch_package, once)

    def test_md5_unknown_layout_mismatch_rejected(self):
        for bad in (b"0" * 32 + b"\n", b"not-md5\n", b"a" * 32 + b"  filename\n",
                    b" " + b"a" * 32, b"a" * 32 + b"\n\n", b"aA" * 16):
            self.assert_rejected(patch_package, fixture(sidecar=bad))

    def test_duplicate_names_each_layer_and_directory_alias(self):
        for original in (fixture(inner_extra=[member(TARGET, RC)]),
                         fixture(outer_extra=[member(INNER, b"duplicate")]),
                         fixture(outer_extra=[member("extras/", kind=tarfile.DIRTYPE)])):
            self.assert_rejected(patch_package, original)

    def test_unsafe_names_each_layer(self):
        for name in ("/absolute", "../escape", "etc/../escape", "etc/./alias",
                     "etc//alias", "C:/absolute", "etc\\bad", "bad\nname"):
            with self.subTest(name=name):
                self.assert_rejected(patch_package, fixture(inner_extra=[member(name, b"bad")]))
                self.assert_rejected(patch_package, fixture(outer_extra=[member(name, b"bad")]))

    def test_target_hardlink_and_mutable_alias_rejected(self):
        self.assert_rejected(patch_package, fixture(rc_kind=tarfile.LNKTYPE))
        self.assert_rejected(patch_package, fixture(inner_extra=[
            member("etc/alias", kind=tarfile.LNKTYPE, link=TARGET)]))
        self.assert_rejected(patch_package, fixture(outer_extra=[
            member("tmp/alias", kind=tarfile.LNKTYPE, link=INNER)]))

    def test_required_outer_files_must_be_regular_and_present(self):
        original = unpack(fixture())
        for required in (INNER, SIDECAR):
            for kind in (tarfile.DIRTYPE, tarfile.SYMTYPE, tarfile.LNKTYPE):
                entries = [(copy.copy(e.info), e.data) for e in original.entries]
                for info, _ in entries:
                    if info.name == required:
                        info.type, info.size = kind, 0
                        info.linkname = "extras/blob.bin" if kind != tarfile.DIRTYPE else ""
                self.assert_rejected(patch_package, archive(entries))
            entries = [(e.info, e.data) for e in original.entries if e.info.name != required]
            self.assert_rejected(patch_package, archive(entries))

    def test_links_unsafe_or_missing_rejected(self):
        for kind, link in ((tarfile.SYMTYPE, "../../outside"), (tarfile.SYMTYPE, "/absolute"),
                           (tarfile.LNKTYPE, "missing"), (tarfile.LNKTYPE, "../../outside")):
            self.assert_rejected(patch_package, fixture(outer_extra=[
                member("extras/invalid-link", kind=kind, link=link)]))

    def test_stock_relative_symlinks_preserved_exactly(self):
        names = ("etc/odhcp6c.user.d/zte_dhcpv6_info.script",
                 "etc/udhcpc.user.d/zte_dhcpv4_info.script")
        targets = ("../../sbin/zte_dhcpv6_info.script",
                   "../../sbin/zte_dhcpv4_info.script")
        extras = [member("etc/odhcp6c.user.d", kind=tarfile.DIRTYPE),
                  member("etc/udhcpc.user.d", kind=tarfile.DIRTYPE),
                  member("sbin", kind=tarfile.DIRTYPE)]
        for name, target in zip(names, targets):
            extras.append(member(name, kind=tarfile.SYMTYPE, link=target))
            extras.append(member(target[len("../../"):], b"synthetic script only\n"))
        original = fixture(inner_extra=extras)
        patched = patch_package(original)
        verify_package(original, patched)
        before = unpack(unpack(original).require_regular(INNER).data)
        after = unpack(unpack(patched).require_regular(INNER).data)
        for name, target in zip(names, targets):
            old = next(e for e in before.entries if e.info.name == name)
            new = next(e for e in after.entries if e.info.name == name)
            self.assertEqual(old.info.linkname, target)
            self.assertEqual(new.info.linkname, target)
            self.assertEqual(metadata(old.info), metadata(new.info))

    def test_relative_symlink_escape_components_and_ancestor_rejected(self):
        # Parent depth is two: exactly two .. components remain inside root.
        good = fixture(inner_extra=[member("etc/sub/link", kind=tarfile.SYMTYPE,
                                          link="../../safe")])
        verify_package(good, patch_package(good))
        for target in ("../../../escape", ".././unsafe", "../bad//empty", r"..\bad", "",
                       "/absolute", "../../safe/../../escape", "../bad\nname"):
            self.assert_rejected(patch_package, fixture(inner_extra=[
                member("etc/sub/link", kind=tarfile.SYMTYPE, link=target)]))
        # A safe symlink may never become an entry-name ancestor for insertion.
        self.assert_rejected(patch_package, fixture(inner_extra=[
            member("etc/sub/alias", kind=tarfile.SYMTYPE, link="../../safe"),
            member("etc/sub/alias/child", b"not allowed")]))
        # PAX linkpath retains strict root-relative path checks, not a blanket
        # relaxation: no member context is available during PAX preflight.
        self.assert_rejected(patch_package, fixture(inner_extra=[
            member("etc/sub/pax-link", kind=tarfile.SYMTYPE, link="../../safe",
                   pax={"linkpath": "../../safe"})]))
        # Hardlinks remain archive-root-relative; .. is never allowed.
        self.assert_rejected(patch_package, fixture(inner_extra=[
            member("etc/sub/hard", kind=tarfile.LNKTYPE, link="../../etc/unchanged")]))

    def test_special_sparse_members_rejected(self):
        for kind in (tarfile.CHRTYPE, tarfile.BLKTYPE, tarfile.FIFOTYPE, b"Z", tarfile.GNUTYPE_SPARSE):
            self.assert_rejected(patch_package, fixture(outer_extra=[member("special", kind=kind)]))
        for pax in ({"GNU.sparse.map": "0,1"}, {"SCHILY.realsize": "1"},
                    {"SCHILY.filetype": "sparse"}):
            self.assert_rejected(patch_package, fixture(rc_pax=pax))

    def test_pax_safe_path_metadata_preserved(self):
        original = fixture(rc_pax={"path": TARGET, "comment": "keep original metadata"},
                           outer_extra=[member("stored-name", b"pax payload",
                                               pax={"path": "extras/effective-name"})])
        patched = patch_package(original)
        verify_package(original, patched)
        old, new = unpack(original), unpack(patched)
        self.assertEqual(old.entries[-1].info.name, "extras/effective-name")
        self.assertEqual(metadata(old.entries[-1].info), metadata(new.entries[-1].info))

    def test_pax_path_linkpath_size_overrides_rejected(self):
        self.assert_rejected(patch_package, fixture(rc_pax={"path": "../../escape"}))
        self.assert_rejected(patch_package, fixture(rc_pax={"size": str(len(RC))}))
        self.assert_rejected(patch_package, fixture(outer_extra=[member(
            "extras/pax-link", kind=tarfile.SYMTYPE, link="blob.bin",
            pax={"linkpath": "/outside"})]))
        self.assert_rejected(patch_package, fixture(global_pax={"path": "../bad"}))

    def test_non_directory_ancestors_rejected(self):
        for kind in (tarfile.REGTYPE, tarfile.SYMTYPE, tarfile.LNKTYPE):
            ancestor = member("shadow", kind=kind,
                              link="extras/blob.bin" if kind != tarfile.REGTYPE else "")
            self.assert_rejected(patch_package, fixture(outer_extra=[
                ancestor, member("shadow/nested", b"unexpected")]))

    def test_unchanged_pax_size_and_gnu_names_preserved(self):
        original = fixture(outer_extra=[member("extras/sized", b"abc", pax={"size": "3"})])
        verify_package(original, patch_package(original))
        # No invented PAX path key for an original GNU long-name member.
        info, data = member("extras/" + "long-" * 40, b"long name payload")
        info.pax_headers, info.mtime = {}, 1700000000
        original_outer = unpack(fixture())
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w:", format=tarfile.GNU_FORMAT) as out:
            for entry in original_outer.entries:
                entry.info.pax_headers = {}
                entry.info.mtime = int(entry.info.mtime)
                out.addfile(entry.info, io.BytesIO(entry.data) if entry.info.isreg() else None)
            out.addfile(info, io.BytesIO(data))
        original = gzip.compress(stream.getvalue())
        verify_package(original, patch_package(original))

    def test_physical_extensions_pax_framing_and_conflicting_sizes(self):
        # Refuse extension recursion before handing anything to tarfile's parser.
        info = tarfile.TarInfo("././@LongLink")
        info.type, info.size = tarfile.GNUTYPE_LONGNAME, 2
        extended = info.tobuf(format=tarfile.GNU_FORMAT) + b"x\0" + b"\0" * 510
        self.assert_rejected(unpack, gzip.compress(extended * 9 + b"\0" * 1024))
        for payload in (b"0 key=value\n", b"999 key=value\n", b"x key=value\n"):
            info = tarfile.TarInfo("pax")
            info.type, info.size = tarfile.XHDTYPE, len(payload)
            raw = info.tobuf() + payload + b"\0" * ((-len(payload)) % 512) + b"\0" * 1024
            self.assert_rejected(unpack, gzip.compress(raw))
        self.assert_rejected(patch_package, fixture(rc_pax={"size": str(len(RC) + 1)}))

    def test_gzip_corruption_and_truncation(self):
        original = fixture()
        for blob in (original[:-1], original[:20], original + b"junk", b"not gzip",
                     original[:-8] + b"\0" * 8):
            self.assert_rejected(patch_package, blob)
        broken = bytearray(original)
        broken[len(broken) // 2] ^= 0xFF
        self.assert_rejected(patch_package, bytes(broken))

    def test_hidden_tar_and_incomplete_framing_rejected(self):
        raw = gzip.decompress(fixture())
        self.assert_rejected(patch_package, gzip.compress(raw + b"nonzero trailer"))
        self.assert_rejected(patch_package, gzip.compress(raw + b"x" * 512))
        self.assert_rejected(patch_package, gzip.compress(raw[:-1]))
        # Keep the valid first archive but append another complete tar.
        self.assert_rejected(patch_package, gzip.compress(raw + raw))

    def test_bounds(self):
        original = fixture()
        for constant, value in (("MAX_COMPRESSED", len(original) - 1), ("MAX_TAR", 512),
                                ("MAX_FILE", 1), ("MAX_PAYLOAD", 1), ("MAX_MEMBERS", 1)):
            with self.subTest(constant=constant), patch.object(guard, constant, value):
                self.assert_rejected(patch_package, original)

    def test_independent_verifier_rejects_other_payload_metadata_order_and_members(self):
        original = fixture()
        good = patch_package(original)
        self.assert_rejected(verify_package, original, rewrite_outer(
            good, other={"extras/blob.bin": b"changed"}))
        old = unpack(good)
        entries = [(copy.copy(e.info), e.data) for e in old.entries]
        entries[-1][0].mode ^= 0o100
        self.assert_rejected(verify_package, original, archive(entries))
        self.assert_rejected(verify_package, original, archive(entries[::-1]))
        self.assert_rejected(verify_package, original, archive(entries[:-1]))

    def test_independent_verifier_rejects_inner_changes_and_wrong_insertion(self):
        original = fixture()
        good = patch_package(original)
        inside = unpack(unpack(good).require_regular(INNER).data)
        rc = inside.require_regular(TARGET).data
        for replacement in (rc.replace(b"echo 1", b"echo 2", 1),
                            rc.replace(NODE, b"/sys/wrong/usb_op", 1),
                            rc.replace(b"protected", b"modified"),
                            rc.replace(b"\n", b"\r\n", 1),
                            rc.replace(b"echo 1 > " + NODE + b"\n", b"", 1),
                            rc + b"\necho extra\n"):
            bad_inner = pack(inside, {TARGET: replacement})
            self.assert_rejected(verify_package, original, rewrite_outer(good, inner=bad_inner))
        changed_inner = pack(inside, {"etc/unchanged": b"changed"})
        self.assert_rejected(verify_package, original, rewrite_outer(good, inner=changed_inner))
        self.assert_rejected(verify_package, original, original)

    def test_verifier_does_not_call_patcher(self):
        original = fixture()
        good = patch_package(original)
        with patch("patcher.patch_rc", side_effect=AssertionError("must not call patcher")), \
             patch("patcher.patch_package", side_effect=AssertionError("must not call patcher")):
            self.assertTrue(verify_package(original, good)["verified"])

    def test_independent_verifier_rejects_stale_digest(self):
        original = fixture()
        good = patch_package(original)
        old_digest = unpack(original).require_regular(SIDECAR).data
        self.assert_rejected(verify_package, original, rewrite_outer(good, checksum=old_digest))

    def test_host_shell_syntax_only_synthetic_fixture(self):
        for synthetic_original in (RC, STOCK_RC):
            synthetic_patched = patch_rc(synthetic_original)
            result = subprocess.run(["/bin/sh", "-n"], input=synthetic_patched,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b"")

    def test_cli_exclusive_create_verify_and_sanitized_output(self):
        # All temporary files are synthetic and under the authorized new subtree.
        with tempfile.TemporaryDirectory(dir=HERE) as directory:
            root = Path(directory)
            original, output = root / "synthetic-input.tgz", root / "synthetic-output.tgz"
            original.write_bytes(fixture())
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
            command = [sys.executable, str(HERE / "patcher.py")]
            result = subprocess.run(command + ["patch", str(original), str(output)],
                                    capture_output=True, env=env, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            report = json.loads(result.stdout)
            self.assertTrue(report["verified"])
            self.assertNotIn(NODE, result.stdout)
            self.assertNotIn(b"fixture-user", result.stdout)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            saved = output.read_bytes()
            repeated = subprocess.run(command + ["patch", str(original), str(output)],
                                      capture_output=True, env=env, check=False)
            self.assertEqual(repeated.returncode, 2)
            self.assertEqual(repeated.stdout, b"")
            self.assertEqual(output.read_bytes(), saved)
            verified = subprocess.run(command + ["verify", str(original), str(output)],
                                      capture_output=True, env=env, check=False)
            self.assertEqual(verified.returncode, 0, verified.stderr)
            # Existing output symlink is not followed and its target is untouched.
            link = root / "existing-symlink"
            link.symlink_to(original)
            refused = subprocess.run(command + ["patch", str(original), str(link)],
                                     capture_output=True, env=env, check=False)
            self.assertEqual(refused.returncode, 2)
            self.assertEqual(original.read_bytes(), fixture())
            with self.assertRaises((Rejected, OSError)):
                guard.read_bounded(str(link))
            self.assert_rejected(guard.read_bounded, str(root))
            fifo = root / "synthetic-fifo"
            os.mkfifo(fifo, 0o600)
            self.assert_rejected(guard.read_bounded, str(fifo))


if __name__ == "__main__":
    unittest.main()
