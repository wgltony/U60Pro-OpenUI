#!/usr/bin/env python3
"""Reject device identity values, backup-key material and personal data in tracked files.

Personal names, SSIDs, accounts or hosts that must never be published go in
.git/info/personal-patterns, one case-insensitive literal per line ('#' starts a
comment). The file lives inside .git, so the list itself is never committed.
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

DEVICE_ID = re.compile(
    rb"(?i)\b(?:imei|imsi|iccid|sim_imei|sim_imsi|sim_iccid)\b"
    rb"[^\r\n]{0,80}?[\"']([0-9]{14,22})[\"']"
)
LITERAL_SUFFIX = re.compile(
    rb"(?im)\bZTE_BACKUP_SUFFIX\s*=\s*([\"'])([^\"'\r\n]+)\1"
)
PRIVATE_KEY = re.compile(rb"-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----")
TOKEN = re.compile(
    rb"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|"
    rb"AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{20,}|sk-[A-Za-z0-9]{20,}|"
    rb"sk-(?:proj|svcacct)-[A-Za-z0-9_-]{20,})"
)


# A real WireGuard private key (a 32-byte base64 value), not a placeholder.
WIREGUARD_KEY = re.compile(rb"(?im)^\s*PrivateKey\s*=\s*[A-Za-z0-9+/]{42}[AEIMQUYcgkosw048]=")
# Tailscale auth/API/OAuth keys: tskey-<kind>-<id>CNTRL-<secret>.
TAILSCALE_KEY = re.compile(rb"tskey-(?:auth|api|client|webhook)-[A-Za-z0-9]{6,}-[A-Za-z0-9]{16,}")
PERSONAL = ROOT / ".git/info/personal-patterns"


def personal_patterns() -> list[bytes]:
    try:
        lines = PERSONAL.read_bytes().splitlines()
    except FileNotFoundError:
        return []
    return [line.strip().lower() for line in lines if line.strip() and not line.lstrip().startswith(b"#")]


def candidate_files() -> list[Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
    )
    return [ROOT / item.decode() for item in result.stdout.split(b"\0") if item]


def main() -> int:
    failures: list[str] = []
    personal = personal_patterns()
    for path in candidate_files():
        try:
            data = path.read_bytes()
        except (FileNotFoundError, IsADirectoryError):
            continue

        if DEVICE_ID.search(data):
            failures.append(f"{path.relative_to(ROOT)}: hard-coded device identity")
        for match in LITERAL_SUFFIX.finditer(data):
            value = match.group(2).strip()
            if value and not value.startswith((b"$", b"<")):
                failures.append(f"{path.relative_to(ROOT)}: literal ZTE backup suffix")
                break
        if PRIVATE_KEY.search(data):
            failures.append(f"{path.relative_to(ROOT)}: private key material")
        if TOKEN.search(data):
            failures.append(f"{path.relative_to(ROOT)}: credential-like token")
        if WIREGUARD_KEY.search(data):
            failures.append(f"{path.relative_to(ROOT)}: WireGuard private key")
        if TAILSCALE_KEY.search(data):
            failures.append(f"{path.relative_to(ROOT)}: Tailscale key")
        lowered = data.lower()
        # Only the file is named: printing the match would publish it in CI logs.
        if any(pattern in lowered for pattern in personal):
            failures.append(f"{path.relative_to(ROOT)}: matches .git/info/personal-patterns")

    if failures:
        print("Device-secret audit failed:")
        for failure in failures:
            print(f"  {failure}")
        return 1

    extra = f", or any of {len(personal)} personal patterns" if personal else ""
    print(f"OK: no tracked device identities, ZTE backup suffixes, private keys, credential tokens, WireGuard or Tailscale keys{extra}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
