"""Independent semantic verifier; shares archive safety, NOT patch/discovery code."""
from __future__ import annotations

import hashlib
import re

from archive_guard import (INNER, SIDECAR, TARGET, Archive, Rejected, metadata,
                           unpack)


def _digest(data: bytes) -> bytes:
    return hashlib.md5(data, usedforsecurity=False).hexdigest().encode("ascii")


def _check_sidecars(old: bytes, new: bytes, old_inner: bytes, new_inner: bytes) -> None:
    # Deliberately independent of patcher's sidecar_layout/md5 functions.
    match = re.fullmatch(rb"([0-9A-Fa-f]{32})(\r\n|\n)?", old)
    if match is None:
        raise Rejected("verify_md5_layout")
    prior = match[1]
    letters = bytes(c for c in prior if 65 <= c <= 70 or 97 <= c <= 102)
    if letters and not (letters.isupper() or letters.islower()):
        raise Rejected("verify_md5_case")
    if prior.lower() != _digest(old_inner):
        raise Rejected("verify_original_md5")
    digest = _digest(new_inner)
    if letters and letters.isupper():
        digest = digest.upper()
    if new != digest + (match[2] or b""):
        raise Rejected("verify_patched_md5")


def _check_rc(old: bytes, new: bytes) -> None:
    # Read the alleged insertion from NEW bytes, then independently discover the
    # actual unique path in OLD bytes. Never accept patcher's claimed path.
    first, separator, rest = old.partition(b"\n")
    if not separator or b"\0" in old:
        raise Rejected("verify_shebang")
    if first in {b"#!/bin/sh", b"#!/bin/sh\r"}:
        new_first, separator, new_rest = new.partition(b"\n")
        if not separator or new_first != first:
            raise Rejected("verify_first_line")
        inserted, separator, remaining = new_rest.partition(b"\n")
        if not separator or remaining != rest:
            raise Rejected("verify_rc_original_bytes")
    else:
        known_comment = b"# Put your custom commands here that should be executed once"
        if first not in {known_comment, known_comment + b"\r"}:
            raise Rejected("verify_shebang")
        inserted, separator, remaining = new.partition(b"\n")
        if not separator or remaining != old:
            raise Rejected("verify_rc_original_bytes")
    crlf = first.endswith(b"\r")
    if crlf:
        if not inserted.endswith(b"\r"):
            raise Rejected("verify_newline")
        inserted = inserted[:-1]
    elif inserted.endswith(b"\r"):
        raise Rejected("verify_newline")
    if not inserted.startswith(b"echo 1 > "):
        raise Rejected("verify_insertion")
    node = inserted[len(b"echo 1 > "):]
    parts = node.split(b"/")
    if len(parts) < 3 or parts[0] != b"" or parts[1] != b"sys" or parts[-1] != b"usb_op":
        raise Rejected("verify_usb_path")
    if any(p in {b"", b".", b".."} or re.fullmatch(rb"[A-Za-z0-9_.-]+", p) is None
           for p in parts[1:]):
        raise Rejected("verify_usb_path")
    discovered: set[bytes] = set()
    forbidden_boundary = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./$\\-"
    for token in re.finditer(rb"/(?:[A-Za-z0-9_.-]+/)+usb_op", old):
        before = old[token.start() - 1:token.start()] if token.start() else b""
        after = old[token.end():token.end() + 1]
        if before and before in forbidden_boundary:
            continue
        if after and after in forbidden_boundary:
            continue
        delimiters = b" \t\r\n=<>('\"`"
        if before and before not in delimiters:
            raise Rejected("verify_usb_literal")
        if before in {b"'", b'"', b"`"} and token.start() > 1:
            if old[token.start() - 2:token.start() - 1] not in delimiters:
                raise Rejected("verify_usb_literal")
        path = token.group()
        if not path.startswith(b"/sys/") or any(p in {b".", b".."} for p in path.split(b"/")[1:]):
            raise Rejected("verify_usb_path")
        discovered.add(path)
    if discovered != {node}:
        raise Rejected("verify_usb_discovery")
    for line in old.split(b"\n"):
        if re.match(rb"[ \t]*echo[ \t]+1[ \t]*>[ \t]*['\"]?" + re.escape(node) +
                    rb"['\"]?(?=[ \t;\r]|$)", line):
            raise Rejected("verify_already_patched")
    # The equality above proves original READ/flash-protection code is unchanged.
    expected_delta = len(b"echo 1 > ") + len(node) + (2 if crlf else 1)
    if len(new) - len(old) != expected_delta:
        raise Rejected("verify_rc_delta")


def _compare_members(old: Archive, new: Archive, changed: set[str]) -> None:
    if old.pax != new.pax or len(old.entries) != len(new.entries):
        raise Rejected("verify_membership")
    for before, after in zip(old.entries, new.entries):
        mutable = before.info.name in changed
        if metadata(before.info, omit_size=mutable) != metadata(after.info, omit_size=mutable):
            raise Rejected("verify_metadata_or_order")
        if mutable:
            if "size" in before.info.pax_headers or "size" in old.pax:
                raise Rejected("verify_mutable_pax_size")
        else:
            if hashlib.sha256(before.data).digest() != hashlib.sha256(after.data).digest():
                raise Rejected("verify_other_payload")
        if before.info.islnk() and before.info.linkname in changed:
            raise Rejected("verify_mutable_alias")


def verify_package(original: bytes, patched: bytes) -> dict[str, object]:
    """Verify both complete archives, returning only fixed sanitized metadata."""
    old_outer, new_outer = unpack(original), unpack(patched)
    _compare_members(old_outer, new_outer, {INNER, SIDECAR})
    oi, ni = old_outer.require_regular(INNER), new_outer.require_regular(INNER)
    om, nm = old_outer.require_regular(SIDECAR), new_outer.require_regular(SIDECAR)
    _check_sidecars(om.data, nm.data, oi.data, ni.data)
    old_inner, new_inner = unpack(oi.data), unpack(ni.data)
    _compare_members(old_inner, new_inner, {TARGET})
    old_rc, new_rc = old_inner.require_regular(TARGET), new_inner.require_regular(TARGET)
    _check_rc(old_rc.data, new_rc.data)
    return {
        "verified": True,
        "outer_members": len(old_outer.entries),
        "inner_members": len(old_inner.entries),
        "rc_added_lines": 1,
        "original_bytes": len(original),
        "patched_bytes": len(patched),
        "original_sha256": hashlib.sha256(original).hexdigest(),
        "patched_sha256": hashlib.sha256(patched).hexdigest(),
    }
