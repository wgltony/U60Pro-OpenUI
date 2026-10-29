"""Bounded, extraction-free archive primitives; no router or crypto operations."""
from __future__ import annotations

import copy
import gzip
import hashlib
import io
import os
import re
import stat
import tarfile
import zlib
from dataclasses import dataclass

INNER = "tmp/back_parameter_r1.tgz"
SIDECAR = "tmp/back_parameter_r.md5"
TARGET = "etc/rc.local"
MAX_COMPRESSED = 32 * 1024 * 1024
MAX_TAR = 128 * 1024 * 1024
MAX_FILE = 32 * 1024 * 1024
MAX_PAYLOAD = 64 * 1024 * 1024
MAX_MEMBERS = 4096
MAX_HEADER_METADATA = 1024 * 1024
MAX_EXTENSION_CHAIN = 8
ALLOWED_TYPES = {tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE,
                 tarfile.SYMTYPE, tarfile.LNKTYPE}


class Rejected(ValueError):
    """A fixed diagnostic code, never archive bytes or attacker-supplied names."""


@dataclass
class Entry:
    info: tarfile.TarInfo
    data: bytes


@dataclass
class Archive:
    entries: list[Entry]
    pax: dict[str, str]

    def require_regular(self, name: str) -> Entry:
        found = [e for e in self.entries if e.info.name == name]
        if len(found) != 1 or found[0].info.type not in {tarfile.REGTYPE, tarfile.AREGTYPE}:
            raise Rejected("required_regular_member")
        return found[0]


def safe_name(name: str) -> str:
    # Reject aliases rather than normalizing them. A trailing slash is allowed
    # only as the conventional directory spelling; the caller checks its type.
    if not name or name.startswith("/") or "\\" in name:
        raise Rejected("unsafe_path")
    if any(ord(c) < 32 or ord(c) == 127 or 0xD800 <= ord(c) <= 0xDFFF for c in name):
        raise Rejected("unsafe_path")
    clean = name[:-1] if name.endswith("/") else name
    parts = clean.split("/")
    if any(p in {"", ".", ".."} for p in parts) or ":" in parts[0]:
        raise Rejected("unsafe_path")
    return clean


def safe_symlink(member_name: str, target: str) -> None:
    """Allow relative .. only when resolution stays within the virtual root.

    Validate, never rewrite the original linkname. Entry names and hardlinks
    deliberately continue to use the stricter safe_name rule.
    """
    if not target or target.startswith("/") or "\\" in target:
        raise Rejected("unsafe_symlink")
    if any(ord(c) < 32 or ord(c) == 127 or 0xD800 <= ord(c) <= 0xDFFF for c in target):
        raise Rejected("unsafe_symlink")
    parts = target.split("/")
    if any(p in {"", "."} for p in parts) or ":" in parts[0]:
        raise Rejected("unsafe_symlink")
    stack = safe_name(member_name).split("/")[:-1]
    for component in parts:
        if component == "..":
            if not stack:
                raise Rejected("symlink_escapes_root")
            stack.pop()
        else:
            stack.append(component)


def _validate_pax(pax: dict[str, str]) -> None:
    if any(k.startswith("GNU.sparse") or k in {"SCHILY.realsize", "SCHILY.filetype"}
           for k in pax):
        raise Rejected("sparse_or_special_pax")
    if "path" in pax:
        safe_name(pax["path"])
    if "linkpath" in pax:
        safe_name(pax["linkpath"])


def _pax_records(data: bytes) -> dict[str, str]:
    """Validate framing before tarfile can interpret sparse/size extensions."""
    records: dict[str, str] = {}
    cursor = 0
    while cursor < len(data):
        separator = data.find(b" ", cursor, cursor + 12)
        if separator < 0:
            raise Rejected("pax_record")
        prefix = data[cursor:separator]
        if re.fullmatch(rb"[1-9][0-9]{0,9}", prefix) is None:
            raise Rejected("pax_record")
        length = int(prefix)
        end = cursor + length
        if end > len(data) or end <= separator + 2 or data[end - 1:end] != b"\n":
            raise Rejected("pax_record")
        key, equals, value = data[separator + 1:end - 1].partition(b"=")
        if not equals or not key or b"\0" in key or b"\n" in key:
            raise Rejected("pax_record")
        try:
            decoded_key, decoded_value = key.decode("utf-8"), value.decode("utf-8")
        except UnicodeError:
            raise Rejected("pax_encoding") from None
        if decoded_key in records:
            raise Rejected("pax_duplicate_key")
        records[decoded_key] = decoded_value
        cursor = end
    _validate_pax(records)
    if "size" in records:
        size = records["size"]
        if re.fullmatch(r"[0-9]{1,10}", size) is None or int(size) > MAX_FILE:
            raise Rejected("pax_size")
    return records


def _preflight(raw: bytes) -> None:
    # tarfile recursively consumes extension chains, and sparse metadata may be
    # expanded before yielding a member. Bound/reject those at physical headers.
    cursor = physical = logical = chain = 0
    global_pax: dict[str, str] = {}
    local_pax: dict[str, str] = {}
    extension_types = {tarfile.XHDTYPE, tarfile.XGLTYPE,
                       tarfile.GNUTYPE_LONGNAME, tarfile.GNUTYPE_LONGLINK}
    while cursor + 512 <= len(raw):
        block = raw[cursor:cursor + 512]
        if not any(block):
            if chain or cursor + 1024 > len(raw) or any(raw[cursor:]):
                raise Rejected("tar_trailer")
            return
        physical += 1
        if physical > 3 * MAX_MEMBERS + 16:
            raise Rejected("physical_member_limit")
        header = tarfile.TarInfo.frombuf(block, "utf-8", "surrogateescape")
        size = header.size
        if size < 0 or size > MAX_FILE:
            raise Rejected("file_limit")
        payload_start = cursor + 512
        cursor = payload_start + ((size + 511) // 512) * 512
        if cursor > len(raw):
            raise Rejected("tar_truncated_member")
        if header.type in extension_types:
            chain += 1
            if chain > MAX_EXTENSION_CHAIN or size > MAX_HEADER_METADATA:
                raise Rejected("extension_limit")
            if header.type in {tarfile.XHDTYPE, tarfile.XGLTYPE}:
                records = _pax_records(raw[payload_start:payload_start + size])
                if header.type == tarfile.XGLTYPE:
                    global_pax.update(records)
                else:
                    if local_pax:
                        raise Rejected("pax_multiple_local")
                    local_pax = records
            continue
        logical += 1
        if logical > MAX_MEMBERS:
            raise Rejected("member_limit")
        if header.type not in ALLOWED_TYPES:
            raise Rejected("unsupported_member")
        # In-range PAX sizes equal to physical sizes are accepted for unchanged
        # files. Conflicting physical/PAX lengths are rejected, never guessed.
        effective_pax = {**global_pax, **local_pax}
        if "size" in effective_pax and int(effective_pax["size"]) != size:
            raise Rejected("pax_size_override")
        local_pax = {}
        chain = 0
    raise Rejected("tar_missing_end")


def unpack(blob: bytes) -> Archive:
    """Read a bounded gzip/tar in memory; never extract or follow links."""
    if not isinstance(blob, bytes) or not blob or len(blob) > MAX_COMPRESSED:
        raise Rejected("compressed_limit")
    try:
        with gzip.GzipFile(fileobj=io.BytesIO(blob), mode="rb") as stream:
            raw = stream.read(MAX_TAR + 1)
        if len(raw) > MAX_TAR:
            raise Rejected("tar_limit")
        # Require a complete tar end marker and no hidden nonzero trailer.
        if len(raw) % tarfile.BLOCKSIZE or len(raw) < 2 * tarfile.BLOCKSIZE:
            raise Rejected("tar_framing")
        _preflight(raw)
        entries: list[Entry] = []
        seen: set[str] = set()
        total = 0
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:", errorlevel=2) as archive:
            for member in archive:
                if len(entries) >= MAX_MEMBERS:
                    raise Rejected("member_limit")
                name = safe_name(member.name)
                if name in seen:
                    raise Rejected("duplicate_member")
                seen.add(name)
                if member.name.endswith("/") and member.type != tarfile.DIRTYPE:
                    raise Rejected("unsafe_path")
                if member.type not in ALLOWED_TYPES or member.sparse is not None:
                    raise Rejected("unsupported_member")
                _validate_pax(member.pax_headers)
                if member.type == tarfile.SYMTYPE:
                    safe_symlink(member.name, member.linkname)
                elif member.type == tarfile.LNKTYPE:
                    safe_name(member.linkname)
                elif member.linkname:
                    raise Rejected("unexpected_linkname")
                if member.size < 0 or member.size > MAX_FILE:
                    raise Rejected("file_limit")
                if not member.isreg() and member.size:
                    raise Rejected("special_payload")
                total += member.size
                if total > MAX_PAYLOAD:
                    raise Rejected("payload_limit")
                if member.isreg():
                    stream = archive.extractfile(member)
                    if stream is None:
                        raise Rejected("missing_payload")
                    data = stream.read(MAX_FILE + 1)
                    if len(data) != member.size:
                        raise Rejected("payload_size")
                else:
                    data = b""
                entries.append(Entry(copy.copy(member), data))
            pax = dict(archive.pax_headers)
            _validate_pax(pax)
            # TarFile.offset points immediately after the final member's data,
            # before the two zero blocks. Reject appended/concatenated tar data.
            end = archive.offset
            if len(raw) < end + 1024 or any(raw[end:]):
                raise Rejected("tar_trailer")
        by_name = {e.info.name.rstrip("/"): e for e in entries}
        for entry in entries:
            components = entry.info.name.rstrip("/").split("/")
            for count in range(1, len(components)):
                parent = by_name.get("/".join(components[:count]))
                if parent is not None and parent.info.type != tarfile.DIRTYPE:
                    raise Rejected("non_directory_ancestor")
            if entry.info.type == tarfile.LNKTYPE:
                dest = by_name.get(entry.info.linkname.rstrip("/"))
                if dest is None or not dest.info.isreg():
                    raise Rejected("hardlink_destination")
        return Archive(entries, pax)
    except Rejected:
        raise
    except (OSError, EOFError, ValueError, tarfile.TarError, OverflowError, zlib.error):
        raise Rejected("invalid_archive") from None


def metadata(info: tarfile.TarInfo, *, omit_size: bool = False) -> tuple:
    values = (info.name, info.mode, info.uid, info.gid, info.uname, info.gname,
              info.mtime, info.type, info.linkname, info.devmajor, info.devminor,
              tuple(sorted(info.pax_headers.items())))
    return values if omit_size else values + (info.size,)


def pack(archive: Archive, replacements: dict[str, bytes]) -> bytes:
    """Re-serialize without extracting; preserve effective member metadata/order."""
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w:", format=tarfile.PAX_FORMAT,
                      pax_headers=dict(archive.pax)) as output:
        for entry in archive.entries:
            member = copy.copy(entry.info)
            member.pax_headers = dict(entry.info.pax_headers)
            data = replacements.get(member.name, entry.data)
            if member.name in replacements:
                # Changing a PAX size record would violate exact metadata retention.
                if "size" in member.pax_headers or "size" in archive.pax:
                    raise Rejected("mutable_pax_size")
                member.size = len(data)
            if raw.tell() + len(data) + 4096 > MAX_TAR:
                raise Rejected("output_tar_limit")
            # GNU long-name records avoid inventing PAX keys for original GNU
            # members; existing PAX dictionaries still use PAX serialization.
            output.format = tarfile.PAX_FORMAT if member.pax_headers else tarfile.GNU_FORMAT
            output.addfile(member, io.BytesIO(data) if member.isreg() else None)
    if raw.tell() > MAX_TAR:
        raise Rejected("output_tar_limit")
    result = gzip.compress(raw.getvalue(), mtime=0)
    if len(result) > MAX_COMPRESSED:
        raise Rejected("output_compressed_limit")
    return result


def md5(blob: bytes) -> bytes:
    return hashlib.md5(blob, usedforsecurity=False).hexdigest().encode("ascii")


def sidecar_layout(data: bytes, inner: bytes) -> tuple[bytes, bool]:
    """Only bare hex digests, optionally LF/CRLF; never invent filename layouts."""
    matched = re.fullmatch(rb"([0-9a-fA-F]{32})(\r\n|\n)?", data)
    if matched is None:
        raise Rejected("md5_layout")
    digest, newline = matched.group(1), matched.group(2) or b""
    letters = bytes(c for c in digest if chr(c).isalpha())
    if letters and not (letters.islower() or letters.isupper()):
        raise Rejected("md5_case_layout")
    if digest.lower() != md5(inner):
        raise Rejected("md5_mismatch")
    return newline, bool(letters and letters.isupper())


def read_bounded(path: str) -> bytes:
    """CLI boundary; caller supplies a local, already-decrypted package."""
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags)
    try:
        details = os.fstat(fd)
        if not stat.S_ISREG(details.st_mode):
            raise Rejected("input_not_regular")
        if details.st_size > MAX_COMPRESSED:
            raise Rejected("compressed_limit")
        with os.fdopen(fd, "rb", closefd=False) as stream:
            blob = stream.read(MAX_COMPRESSED + 1)
    finally:
        os.close(fd)
    if len(blob) > MAX_COMPRESSED:
        raise Rejected("compressed_limit")
    return blob
