#!/usr/bin/env python3
"""Extract tailscale and tailscaled from the pinned official static arm64 package."""
import argparse
import hashlib
import io
from pathlib import Path
import tarfile
import urllib.request

VERSION = '1.104.2'
URL = f'https://pkgs.tailscale.com/stable/tailscale_{VERSION}_arm64.tgz'
SHA256 = '5776aaa901b5ef1916e66be5bb54b9f481cd399c9c28db123a8cab620efa237b'
LIMIT = 128 * 1024 * 1024

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', default='build/tailscale', help='directory for tailscale and tailscaled')
    args = parser.parse_args()
    with urllib.request.urlopen(URL, timeout=60) as response:
        package = response.read(LIMIT + 1)
    if len(package) > LIMIT or hashlib.sha256(package).hexdigest() != SHA256:
        raise SystemExit('Tailscale package checksum mismatch')
    out = Path(args.output)
    out.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(package), mode='r:gz') as archive:
        for name in ('tailscale', 'tailscaled'):
            binary = archive.extractfile(f'tailscale_{VERSION}_arm64/{name}').read()
            path = out / name
            path.write_bytes(binary)
            path.chmod(0o700)
    print(f'Checksum verified; extracted Tailscale {VERSION} to {out}')

if __name__ == '__main__':
    main()
