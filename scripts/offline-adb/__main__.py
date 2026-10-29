#!/usr/bin/env python3
"""Offline-only stock MU5250 B31 ADB backup tool; Python 3.10+, macOS/Linux.

Never contacts a router, uploads, restores, reboots, or executes archive members.
The caller must supply a freshly exported backup from the same stock B31 unit.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import getpass
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import warnings

from archive_guard import INNER, TARGET, MAX_COMPRESSED, MAX_TAR, Rejected, unpack
from patcher import patch_package
from verify import verify_package

MAX_ENCRYPTED = MAX_COMPRESSED + 32
MAX_SECRET_FILE = 4096
# Fingerprint of the exact stock template independently retained after the
# successful factory-reset B31 unlock, with ONLY our first echo line removed.
# This is intentionally NOT a generic rc.local injector for unknown variants.
KNOWN_STOCK_RC_SHA256 = '21ab5b3c49c4b9b96e6fb69f5e73ea6dcd6ceb840ea5a2bf34d8da5570bc0280'
STOCK_NODE = b'/sys/class/android_usb/android0/usb_op'


class ToolError(ValueError):
    pass


def read_regular(path: str | Path, limit: int, *, secret: bool = False) -> bytes:
    """Bounded reads, no final-component symlink/FIFO; reject loose key-file modes."""
    path = os.fspath(path)
    if os.path.islink(path):
        raise ToolError('input_symlink')
    flags = os.O_RDONLY | getattr(os, 'O_NONBLOCK', 0) | getattr(os, 'O_NOFOLLOW', 0)
    fd = os.open(path, flags)
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            raise ToolError('input_not_regular')
        if before.st_size > limit:
            raise ToolError('input_size')
        if secret and os.name == 'posix' and before.st_mode & 0o077:
            raise ToolError('secret_permissions')
        with os.fdopen(fd, 'rb', closefd=False) as stream:
            data = stream.read(limit + 1)
        after = os.fstat(fd)
        if len(data) > limit or len(data) != before.st_size:
            raise ToolError('input_size')
        if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (
                after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            raise ToolError('input_changed')
        return data
    finally:
        os.close(fd)


def validate_password(password: str) -> str:
    if not password or any(c in password for c in '\0\r\n'):
        raise ToolError('bad_secret')
    if len(password.encode('utf-8')) > 1024:
        raise ToolError('bad_secret')
    return password


def hidden_prompt(prompt: str) -> str:
    if not sys.stdin.isatty():
        raise ToolError('terminal_required')
    with warnings.catch_warnings():
        warnings.simplefilter('error', getpass.GetPassWarning)
        try:
            return getpass.getpass(prompt)
        except getpass.GetPassWarning:
            raise ToolError('terminal_required') from None


def load_password(args: argparse.Namespace) -> str:
    if args.password_prompt:
        return validate_password(hidden_prompt('完整备份解密口令（不回显，不是网页管理密码）: '))
    if args.password_file:
        value = read_regular(args.password_file, MAX_SECRET_FILE, secret=True).decode('utf-8')
        # Permit one conventional file-ending newline, not arbitrary whitespace.
        if value.endswith('\r\n'):
            value = value[:-2]
        elif value.endswith('\n'):
            value = value[:-1]
        return validate_password(value)
    if args.suffix_file:
        document = json.loads(read_regular(args.suffix_file, MAX_SECRET_FILE, secret=True))
        if not isinstance(document, dict) or not isinstance(document.get('suffix'), str):
            raise ToolError('bad_suffix_file')
        suffix = validate_password(document['suffix'])
    else:
        suffix = validate_password(hidden_prompt('备份加密后缀（不回显，不是网页管理密码）: '))
    imei = hidden_prompt('当前这台路由器的 15 位 IMEI（不回显）: ')
    if re.fullmatch(r'[0-9]{15}', imei) is None:
        raise ToolError('bad_imei')
    return validate_password(imei + suffix)


def check_encrypted(data: bytes) -> None:
    if len(data) < 24 or len(data) > MAX_ENCRYPTED or data[:8] != b'Salted__' or (len(data) - 16) % 8:
        raise ToolError('encrypted_format')


def crypt(data: bytes, password: str, *, encrypt: bool) -> bytes:
    """OpenSSL-compatible 3DES-CBC/SHA256; secrets via inherited pipe, never argv/env.

    No plaintext files are written. Only OpenSSL's fixed operation is executed.
    A separate pipe carries the password while stdin carries the binary payload.
    """
    if os.name != 'posix':
        raise ToolError('posix_required')
    program = shutil.which('openssl')
    if program is None:
        raise ToolError('openssl_missing')
    if encrypt:
        if not data or len(data) > MAX_COMPRESSED:
            raise ToolError('input_size')
    else:
        check_encrypted(data)
    secret = (validate_password(password) + '\n').encode('utf-8')
    read_fd, write_fd = os.pipe()
    try:
        if read_fd < 3:
            # Keep password input distinct even if an automated caller closed
            # stdin/stdout/stderr before starting us.
            import fcntl
            safe_fd = fcntl.fcntl(read_fd, fcntl.F_DUPFD, 3)
            os.set_inheritable(safe_fd, False)
            os.close(read_fd)
            read_fd = safe_fd
        try:
            if os.write(write_fd, secret) != len(secret):
                raise ToolError('secret_pipe')
        finally:
            os.close(write_fd)
        result = subprocess.run(
            [program, 'enc', '-des-ede3-cbc', '-e' if encrypt else '-d',
             '-md', 'sha256', '-pass', f'fd:{read_fd}'],
            input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            pass_fds=(read_fd,), timeout=45, check=False)
    finally:
        os.close(read_fd)
    if result.returncode:
        # Never echo OpenSSL stderr: diagnostics are not needed to expose secrets.
        raise ToolError('crypto_failed')
    if encrypt:
        check_encrypted(result.stdout)
    elif not result.stdout or len(result.stdout) > MAX_COMPRESSED:
        raise ToolError('input_size')
    return result.stdout


def extra_archive_checks(blob: bytes) -> None:
    """Reject unsupported physical GNU fields and cross-link target traversal.

    This is NOT a general malicious-backup/code audit. Inputs must be freshly
    exported, trusted stock backups; members are never extracted or executed.
    """
    outer = unpack(blob)
    inner_blob = outer.require_regular(INNER).data
    inner = unpack(inner_blob)
    for compressed, archive in ((blob, outer), (inner_blob, inner)):
        with gzip.GzipFile(fileobj=io.BytesIO(compressed), mode='rb') as stream:
            raw = stream.read(MAX_TAR + 1)
        if len(raw) > MAX_TAR:
            raise ToolError('archive_variant')
        cursor = 0
        while cursor + 512 <= len(raw) and any(raw[cursor:cursor + 512]):
            block = raw[cursor:cursor + 512]
            # Python/native tar interpretations can disagree on old-GNU fields.
            if block[257:265] == tarfile.GNU_MAGIC and any(block[345:500]):
                raise ToolError('archive_variant')
            header = tarfile.TarInfo.frombuf(block, 'utf-8', 'surrogateescape')
            cursor += 512 + ((header.size + 511) // 512) * 512
        by_name = {entry.info.name.rstrip('/'): entry for entry in archive.entries}
        for entry in archive.entries:
            if not (entry.info.issym() or entry.info.islnk()):
                continue
            parent = entry.info.name.rpartition('/')[0]
            stack = parent.split('/') if entry.info.issym() and parent else []
            for component in entry.info.linkname.split('/'):
                if component == '..':
                    if not stack:
                        raise ToolError('archive_variant')
                    stack.pop()
                else:
                    stack.append(component)
                    crossed = by_name.get('/'.join(stack))
                    if crossed is not None and (crossed.info.issym() or crossed.info.islnk()):
                        raise ToolError('archive_variant')


def stock_node_check(blob: bytes) -> None:
    rc = unpack(unpack(blob).require_regular(INNER).data).require_regular(TARGET).data
    if hashlib.sha256(rc).hexdigest() != KNOWN_STOCK_RC_SHA256:
        raise ToolError('stock_template_mismatch')
    # Every usb_op reference must be a bare argument to cat, not a comment,
    # concatenated quote/glob token, dynamic expansion, or another node.
    references = [match.start() for match in re.finditer(rb'usb_op', rc)]
    pattern = (rb'(?:^|[ \t(`])cat[ \t]+(' + re.escape(STOCK_NODE)
               + rb')(?=$|[ \t\r\n)`;])')
    reads = [match.start(1) + len(STOCK_NODE) - len(b'usb_op')
             for match in re.finditer(pattern, rc, flags=re.MULTILINE)]
    if not references or references != reads:
        raise ToolError('stock_template_mismatch')


def shell_syntax(blob: bytes) -> None:
    rc = unpack(unpack(blob).require_regular(INNER).data).require_regular(TARGET).data
    program = shutil.which('sh')
    if program is None:
        raise ToolError('shell_missing')
    result = subprocess.run([program, '-n'], input=rc, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=10, check=False)
    if result.returncode:
        raise ToolError('shell_syntax')


def write_new(path: Path, data: bytes) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, 'O_NOFOLLOW', 0)
    fd = os.open(path, flags, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def prepare(args: argparse.Namespace, password: str) -> dict:
    if not args.confirm_stock_b31:
        raise ToolError('b31_confirmation_required')
    original = read_regular(args.original, MAX_ENCRYPTED)
    old_plain = crypt(original, password, encrypt=False)
    extra_archive_checks(old_plain)
    stock_node_check(old_plain)
    new_plain = patch_package(old_plain)
    extra_archive_checks(new_plain)
    proof = verify_package(old_plain, new_plain)
    shell_syntax(old_plain)
    shell_syntax(new_plain)
    candidate = crypt(new_plain, password, encrypt=True)
    if crypt(candidate, password, encrypt=False) != new_plain:
        raise ToolError('crypto_roundtrip')
    # Preserve the source exactly; reject a changed file before making outputs.
    if read_regular(args.original, MAX_ENCRYPTED) != original:
        raise ToolError('input_changed')
    report = {
        'tool': 'stock-mu5250-b31-adb-offline', 'version': 1,
        'created_utc': datetime.now(timezone.utc).isoformat(),
        'firmware_check': 'user-confirmed stock B31; not independently detectable from backup',
        'original_encrypted_sha256': hashlib.sha256(original).hexdigest(),
        'patched_encrypted_sha256': hashlib.sha256(candidate).hexdigest(),
        'proof': proof, 'shell_syntax_checked': True, 'crypto_roundtrip_verified': True,
        'network_access': False, 'uploaded': False, 'restored': False, 'rebooted': False,
        'warning': 'Manual native restore auto-reboots; use only on same unit/current configuration.'
    }
    # Exclusive new directory; never overwrite an original, output, or symlink.
    directory = Path(args.out_dir)
    directory.mkdir(mode=0o700)
    write_new(directory / 'back_parameter.original.enc', original)
    write_new(directory / 'back_parameter.adb.enc', candidate)
    persisted_original = read_regular(directory / 'back_parameter.original.enc', MAX_ENCRYPTED)
    persisted_candidate = read_regular(directory / 'back_parameter.adb.enc', MAX_ENCRYPTED)
    if persisted_original != original or persisted_candidate != candidate:
        raise ToolError('output_verification')
    verify_package(crypt(persisted_original, password, encrypt=False),
                   crypt(persisted_candidate, password, encrypt=False))
    # Completion receipt is created LAST. Any failure before this leaves an
    # incomplete directory that must not be used for native restore.
    write_new(directory / 'verification.json', (json.dumps(report, ensure_ascii=False, indent=2) + '\n').encode())
    return {'verified': True, 'rc_added_lines': proof['rc_added_lines'],
            'output_directory': str(directory.resolve()),
            'file_to_restore': 'back_parameter.adb.enc', 'router_contacted': False,
            'manual_restore_auto_reboots': True}


def verify(args: argparse.Namespace, password: str) -> dict:
    original = read_regular(args.original, MAX_ENCRYPTED)
    patched = read_regular(args.patched, MAX_ENCRYPTED)
    old_plain = crypt(original, password, encrypt=False)
    extra_archive_checks(old_plain)
    stock_node_check(old_plain)
    new_plain = crypt(patched, password, encrypt=False)
    extra_archive_checks(new_plain)
    proof = verify_package(old_plain, new_plain)
    shell_syntax(old_plain)
    shell_syntax(new_plain)
    return {'verified': True, 'rc_added_lines': proof['rc_added_lines'],
            'original_encrypted_sha256': hashlib.sha256(original).hexdigest(),
            'patched_encrypted_sha256': hashlib.sha256(patched).hexdigest(),
            'router_contacted': False}


MESSAGES = {
    'b31_confirmation_required': '请先核对型号 MU5250、原生国行 B31，并添加 --confirm-stock-b31。',
    'bad_imei': 'IMEI 必须是当前设备的 15 位数字；未生成输出。',
    'stock_template_mismatch': 'rc.local 不是已验证的原生 B31 模板（或已解锁/已修改），拒绝操作；不要替换成旧模板强行通过。',
    'archive_variant': '归档包含未支持的物理字段或链接链，拒绝操作；不要绕过检查恢复。',
    'bad_secret': '解密口令/后缀无效；不要使用网页密码代替备份解密口令。',
    'bad_suffix_file': '后缀文件须为包含字符串 suffix 的 JSON 文件。',
    'secret_permissions': '密钥文件权限过宽；请先执行 chmod 600 密钥文件。',
    'terminal_required': '需要交互终端输入隐私信息；自动化可用 --password-file。',
    'encrypted_format': '不是受支持的 Salted__ 3DES 加密备份；未生成输出。',
    'crypto_failed': '解密/加密失败：口令不对、文件损坏或 OpenSSL 不支持；未通过校验。',
    'already_patched': '备份已经包含开启 ADB 的行；拒绝重复注入，请勿恢复旧备份。',
    'verify_already_patched': '原备份已开启 ADB；此工具不用于重复修改。',
    'posix_required': '此版本支持 macOS/Linux；Windows 未适配，不要直接用于恢复。',
    'openssl_missing': '未找到 openssl；需要本地 Python 3.10+、OpenSSL 和 sh。',
    'shell_missing': '未找到 sh，无法进行只解析不执行的 shell 语法检查。',
    'shell_syntax': '原始或候选 rc.local 未通过语法检查；拒绝生成恢复文件。',
    'input_symlink': '拒绝读取符号链接；请使用常规文件。',
    'input_not_regular': '输入必须是常规文件，不能是目录、管道或设备。',
    'input_size': '输入/输出大小不符合安全限制。',
    'input_changed': '处理期间原始输入发生变化；操作已停止。',
    'crypto_roundtrip': '加密往返校验失败；操作已停止。',
    'output_verification': '写入后校验失败；不要使用输出。',
}


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description='MU5250 原生 B31 ADB 离线备份工具；绝不连接路由器或自动恢复。')
    sub = p.add_subparsers(dest='command', required=True)
    for name in ('prepare', 'verify'):
        command = sub.add_parser(name)
        command.add_argument('original', help='当前同一台原生 B31 导出的加密参数备份')
        if name == 'prepare':
            command.add_argument('--out-dir', required=True, help='必须是尚不存在的新目录')
            command.add_argument('--confirm-stock-b31', action='store_true', help='本人已核对原生国行 MU5250 B31')
        else:
            command.add_argument('patched', help='待独立复核的加密候选文件')
        secret = command.add_mutually_exclusive_group()
        secret.add_argument('--suffix-file', help='私有 JSON 文件：suffix；仍会隐藏提示输入 IMEI')
        secret.add_argument('--password-file', help='私有 UTF-8 文件：完整备份解密口令（不是网页密码）')
        secret.add_argument('--password-prompt', action='store_true', help='隐藏输入完整备份解密口令，不另外输入 IMEI/后缀')
    return p


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    os.umask(0o077)
    try:
        if args.command == 'prepare' and not args.confirm_stock_b31:
            raise ToolError('b31_confirmation_required')
        password = load_password(args)
        report = prepare(args, password) if args.command == 'prepare' else verify(args, password)
        print(json.dumps(report, ensure_ascii=False, sort_keys=True))
        return 0
    except FileExistsError:
        print('ERROR: 输出目录/文件已存在，未覆盖。请换用新的 --out-dir。', file=sys.stderr)
    except (Rejected, ToolError) as exc:
        code = str(exc)
        message = MESSAGES.get(code, '备份结构或校验不符合已验证格式；已拒绝操作。')
        print('ERROR: ' + message, file=sys.stderr)
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
        print('ERROR: 本地文件、口令格式或外部工具检查失败；不要使用未通过校验的输出。', file=sys.stderr)
    except (KeyboardInterrupt, EOFError):
        print('ERROR: 操作已取消；未连接路由器。', file=sys.stderr)
    return 2


if __name__ == '__main__':
    raise SystemExit(main())
