#!/usr/bin/env python3
"""Validate locally built, pinned IMEI helpers without executing any helper.

This does not establish on-device ABI or write safety. Build these optional
components with tools/imei-native/build.py and retain the accompanying notices.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat

ROOT = Path(__file__).resolve().parents[1]
NATIVE = ROOT / 'tools' / 'imei-native'
NAMES = ('zte_nv', 'zte_config', 'zte_config_read')
FIXED = {
    'schema': 1,
    'profile': 'mu5250-b31-libdiag-v1',
    'source_commit': '2f7e24c49125ba06d375def56ea5f3e7c1f770aa',
    'platform': 'aarch64-linux-musl',
    'compiler': 'zig 0.14.1',
    'interpreter': '/lib/ld-musl-aarch64.so.1',
    'libdiag_sha256': 'c0fee4f76b302dd7db2a91dcd5bf1a3f771f674b4637f21468d864a789d00392',
    'router_sha256': '55c54f74aaa427940254a2f16c36771e675a80a002363e4f10b0dfcb604d9c6f',
    'musl_sha256': 'd9e8fd8847efd5f63c7ff6476d8654f187be2a48d59d5551eb7c3df9c70e01dd',
    'private_device_data_embedded': False,
}


def read_regular(path, limit):
    path = Path(path)
    st = path.lstat()
    if not stat.S_ISREG(st.st_mode) or st.st_nlink != 1 or not 0 < st.st_size <= limit:
        raise ValueError('IMEI component is not a bounded regular file')
    fd = os.open(path, os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0))
    with os.fdopen(fd, 'rb') as f:
        opened = os.fstat(f.fileno())
        data = f.read(limit + 1)
    if (st.st_dev, st.st_ino, st.st_size) != (opened.st_dev, opened.st_ino, len(data)):
        raise ValueError('IMEI component changed during validation')
    return data


def no_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('Duplicate IMEI manifest field')
        result[key] = value
    return result


def validate(directory):
    directory = Path(directory)
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError('IMEI helper directory must be a real directory')
    raw = read_regular(directory / 'manifest.json', 128 * 1024)
    manifest = json.loads(raw, object_pairs_hook=no_duplicate_keys)
    if not isinstance(manifest, dict) or any(type(manifest.get(k)) is not type(v) or manifest.get(k) != v for k, v in FIXED.items()):
        raise ValueError('IMEI helper manifest does not match the supported B31 profile')
    spec = importlib.util.spec_from_file_location('imei_native_metadata', NATIVE / 'build.py')
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    provenance, sources = native.verified_sources()
    if manifest.get('provenance_sha256') != hashlib.sha256((NATIVE / 'PROVENANCE.json').read_bytes()).hexdigest() or manifest.get('source_sha256') != sources:
        raise ValueError('IMEI helper provenance or source checksums do not match this checkout')
    if manifest.get('upstream') != {'repository': provenance['repository'], 'commit': provenance['commit'], 'license': 'MIT'}:
        raise ValueError('IMEI helper upstream identity mismatch')
    for field in ('helpers', 'binary_sizes', 'elf'):
        if not isinstance(manifest.get(field), dict) or set(manifest[field]) != set(NAMES):
            raise ValueError('IMEI helper set is incomplete or unexpected')
    prepared = {'manifest.json': raw}
    for name in NAMES:
        data = read_regular(directory / name, 1024 * 1024)
        if type(manifest['binary_sizes'][name]) is not int or manifest['binary_sizes'][name] != len(data) or manifest['helpers'][name] != hashlib.sha256(data).hexdigest():
            raise ValueError('IMEI helper checksum or size mismatch')
        if native.inspect_elf(data) != manifest['elf'][name]:
            raise ValueError('IMEI helper ELF metadata mismatch')
        prepared[name] = data
    # Notices are fixed local source files, never paths supplied by the manifest.
    for path in [NATIVE / 'LICENSE', NATIVE / 'LICENSES.md', *[NATIVE / 'licenses' / name for name in ('musl-COPYRIGHT.txt', 'zig-LICENSE.txt', 'TOOLCHAIN.json')]]:
        prepared['licenses/' + path.name] = read_regular(path, 512 * 1024)
    if not (NATIVE / 'licenses').is_dir() or len(prepared) < 8:
        raise ValueError('IMEI helper license notices are incomplete')
    return prepared


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    files = validate(args.directory)
    print(f'Validated {len(NAMES)} optional IMEI helpers and {len(files) - 4} notice files; no helper executed.')
