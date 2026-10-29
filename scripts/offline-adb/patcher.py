"""Strictly offline, byte-preserving minimal rc.local package patcher.

Input is a decrypted gzip/tar package, NOT an encrypted router backup.
No decryption, credential handling, networking, extraction, restore or reboot.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys

from archive_guard import (INNER, SIDECAR, TARGET, Rejected, md5, pack,
                           read_bounded, sidecar_layout, unpack)

# Capture only literal path characters. In particular ')' is never part of a node.
_NODE = re.compile(rb"(?<![A-Za-z0-9_./$\\-])(/(?:[A-Za-z0-9_.-]+/)+usb_op)(?![A-Za-z0-9_./$\\-])")


def patch_rc(original: bytes) -> bytes:
    if b"\x00" in original:
        raise Rejected("rc_nul")
    first_end = original.find(b"\n")
    if first_end < 0:
        raise Rejected("rc_shebang")
    first = original[:first_end + 1]
    has_shebang = first in {b"#!/bin/sh\n", b"#!/bin/sh\r\n"}
    stock_comment = b"# Put your custom commands here that should be executed once"
    if not has_shebang and first not in {stock_comment + b"\n", stock_comment + b"\r\n"}:
        raise Rejected("rc_shebang")
    nodes: set[bytes] = set()
    allowed_before = b" \t\r\n=<>('\"`"
    for match in _NODE.finditer(original):
        start = match.start(1)
        before = original[start - 1:start] if start else b""
        if before and before not in allowed_before:
            raise Rejected("usb_node_not_literal")
        if before in {b"'", b'"', b"`"} and start > 1:
            if original[start - 2:start - 1] not in allowed_before:
                raise Rejected("usb_node_not_literal")
        nodes.add(match.group(1))
    if len(nodes) != 1:
        raise Rejected("usb_node_not_unique")
    node = nodes.pop()
    if not node.startswith(b"/sys/") or any(
            part in {b"", b".", b".."} for part in node.split(b"/")[1:]):
        raise Rejected("usb_node_unsafe")
    already = re.compile(rb"(?m)^[ \t]*echo[ \t]+1[ \t]*>[ \t]*['\"]?" +
                         re.escape(node) + rb"['\"]?(?=[ \t;\r\n]|$)")
    if already.search(original):
        raise Rejected("already_patched")
    newline = b"\r\n" if first.endswith(b"\r\n") else b"\n"
    inserted = b"echo 1 > " + node + newline
    if has_shebang:
        return first + inserted + original[first_end + 1:]
    return inserted + original  # Exact known stock no-shebang prefix only.


def patch_package(original: bytes) -> bytes:
    """Return patched decrypted package; independently verify before returning."""
    outer = unpack(original)
    inner_entry = outer.require_regular(INNER)
    checksum_entry = outer.require_regular(SIDECAR)
    newline, uppercase = sidecar_layout(checksum_entry.data, inner_entry.data)
    inner = unpack(inner_entry.data)
    rc = inner.require_regular(TARGET)
    # A link alias to a changed payload would change a second logical file.
    for archive, changed in ((outer, {INNER, SIDECAR}), (inner, {TARGET})):
        if any(e.info.islnk() and e.info.linkname in changed for e in archive.entries):
            raise Rejected("mutable_hardlink_alias")
    new_inner = pack(inner, {TARGET: patch_rc(rc.data)})
    digest = md5(new_inner)
    new_outer = pack(outer, {INNER: new_inner,
                             SIDECAR: (digest.upper() if uppercase else digest) + newline})
    # Import lazily; verifier never imports or calls this patch implementation.
    from verify import verify_package
    verify_package(original, new_outer)
    return new_outer


class _Parser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        raise Rejected("cli_arguments")


def main(argv: list[str] | None = None) -> int:
    parser = _Parser(description="Offline decrypted-package patch/verify; never connects to a router.")
    sub = parser.add_subparsers(dest="command", required=True, parser_class=_Parser)
    patch = sub.add_parser("patch", help="create output exclusively; never overwrite")
    patch.add_argument("original")
    patch.add_argument("output")
    verify = sub.add_parser("verify", help="independently compare original and patched package")
    verify.add_argument("original")
    verify.add_argument("patched")
    try:
        args = parser.parse_args(argv)
        original = read_bounded(args.original)
        from verify import verify_package
        if args.command == "verify":
            report = verify_package(original, read_bounded(args.patched))
        else:
            result = patch_package(original)
            report = verify_package(original, result)
            # Do not follow an output symlink or overwrite ANY existing object.
            flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
            if hasattr(os, "O_NOFOLLOW"):
                flags |= os.O_NOFOLLOW
            fd = os.open(args.output, flags, 0o600)
            with os.fdopen(fd, "wb") as output:
                output.write(result)
                output.flush()
                os.fsync(output.fileno())
        print(json.dumps(report, sort_keys=True))
        return 0
    except (Rejected, OSError, ValueError, OverflowError):
        # Do not print archive paths, rc.local contents, diffs, credentials or
        # exception strings (which can contain attacker-controlled metadata).
        print("ERROR: offline package operation rejected", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
