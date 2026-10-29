"""Offline synthetic tests. Static startup code only; NO real backups or secrets."""
from __future__ import annotations
import argparse
import copy
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from archive_guard import INNER, TARGET, unpack
from patcher import patch_package
from test_patcher import fixture, member

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('b31_offline_tool', HERE / '__main__.py')
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)
STOCK = (HERE / 'stock_rc.local.sample').read_bytes()
# The shipped tool keeps the real stock fingerprint (KNOWN_STOCK_RC_SHA256 in
# __main__.py) for use on a real device. These tests are fully synthetic, so they
# pin that guard to the sample above instead of shipping a real stock rc.local.
tool.KNOWN_STOCK_RC_SHA256 = hashlib.sha256(STOCK).hexdigest()
PASSWORD = 'synthetic-test-password-NOT-A-REAL-KEY'


def args(original, output):
    return argparse.Namespace(original=str(original), out_dir=str(output),
                              confirm_stock_b31=True)


class OfflineToolTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='u60-b31-synthetic-')
        self.root = Path(self.temp.name)
        self.plain = fixture(rc=STOCK)
        self.encrypted = tool.crypt(self.plain, PASSWORD, encrypt=True)
        self.original = self.root / 'synthetic-original.enc'
        self.original.write_bytes(self.encrypted)
        self.output = self.root / 'new-private-output'

    def tearDown(self):
        self.temp.cleanup()

    def test_stock_template_and_crypto_roundtrip(self):
        tool.stock_node_check(self.plain)
        tool.extra_archive_checks(self.plain)
        self.assertEqual(tool.crypt(self.encrypted, PASSWORD, encrypt=False), self.plain)
        self.assertNotEqual(tool.crypt(self.plain, PASSWORD, encrypt=True), self.encrypted)

    def test_prepare_verify_permissions_and_no_network(self):
        with patch('socket.socket', side_effect=AssertionError('network forbidden')):
            result = tool.prepare(args(self.original, self.output), PASSWORD)
            self.assertTrue(result['verified'])
            self.assertEqual(result['rc_added_lines'], 1)
            self.assertFalse(result['router_contacted'])
            self.assertEqual((self.output / 'back_parameter.original.enc').read_bytes(), self.encrypted)
            self.assertEqual(self.original.read_bytes(), self.encrypted)
            mode = stat.S_IMODE(self.output.stat().st_mode)
            self.assertEqual(mode, 0o700)
            for path in self.output.iterdir():
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            report = json.loads((self.output / 'verification.json').read_text())
            self.assertFalse(report['uploaded'])
            self.assertFalse(report['restored'])
            self.assertFalse(report['rebooted'])
            self.assertNotIn(PASSWORD, json.dumps(report))
            result = tool.verify(argparse.Namespace(original=str(self.original),
                                  patched=str(self.output / 'back_parameter.adb.enc')), PASSWORD)
            self.assertTrue(result['verified'])
            patched = tool.crypt((self.output / 'back_parameter.adb.enc').read_bytes(), PASSWORD, encrypt=False)
            rc = unpack(unpack(patched).require_regular(INNER).data).require_regular(TARGET).data
            self.assertEqual(rc, b'echo 1 > /sys/class/android_usb/android0/usb_op\n' + STOCK)

    def test_wrong_password_or_corruption_never_creates_output(self):
        with self.assertRaises((tool.ToolError, ValueError)):
            tool.prepare(args(self.original, self.output), 'wrong-synthetic-password')
        self.assertFalse(self.output.exists())
        self.original.write_bytes(self.encrypted[:-1])
        with self.assertRaises(tool.ToolError):
            tool.prepare(args(self.original, self.output), PASSWORD)
        self.assertFalse(self.output.exists())

    def test_confirmation_required_before_any_work(self):
        a = args(self.original, self.output)
        a.confirm_stock_b31 = False
        with patch.object(tool, 'crypt', side_effect=AssertionError('must not run')):
            with self.assertRaisesRegex(tool.ToolError, 'b31_confirmation_required'):
                tool.prepare(a, PASSWORD)
        self.assertFalse(self.output.exists())

    def test_existing_output_is_never_overwritten(self):
        self.output.mkdir()
        sentinel = self.output / 'keep'
        sentinel.write_bytes(b'KEEP')
        with self.assertRaises(FileExistsError):
            tool.prepare(args(self.original, self.output), PASSWORD)
        self.assertEqual(list(self.output.iterdir()), [sentinel])
        self.assertEqual(sentinel.read_bytes(), b'KEEP')

    def test_nonstock_and_already_enabled_rejected_by_both_commands(self):
        node = b'/sys/class/android_usb/android0/usb_op'
        variants = [STOCK + b'\n# changed', b'echo 1 > ' + node + b'\n' + STOCK,
                    STOCK.replace(node, node + b'"suffix"'),
                    STOCK.replace(node, node + b'`printf suffix`'),
                    STOCK.replace(node, node + b'*'),
                    b'# ' + node + b'\nexit 0\n']
        for rc in variants:
            with self.subTest(rc_bytes=len(rc)):
                bad = fixture(rc=rc)
                with self.assertRaisesRegex(tool.ToolError, 'stock_template_mismatch'):
                    tool.stock_node_check(bad)
        self.original.write_bytes(tool.crypt(fixture(rc=variants[0]), PASSWORD, encrypt=True))
        with self.assertRaisesRegex(tool.ToolError, 'stock_template_mismatch'):
            tool.prepare(args(self.original, self.output), PASSWORD)
        candidate = self.root / 'synthetic-candidate.enc'
        candidate.write_bytes(self.encrypted)
        with self.assertRaisesRegex(tool.ToolError, 'stock_template_mismatch'):
            tool.verify(argparse.Namespace(original=str(self.original), patched=str(candidate)), PASSWORD)
        self.assertFalse(self.output.exists())

    def test_rejected_link_chain(self):
        data = fixture(rc=STOCK, outer_extra=(
            member('extras/a', kind=tarfile.SYMTYPE, link='blob.bin'),
            member('extras/pivot', kind=tarfile.SYMTYPE, link='a/../../outside'),))
        unpack(data)  # lexical core accepts this; wrapper must fail closed
        with self.assertRaisesRegex(tool.ToolError, 'archive_variant'):
            tool.extra_archive_checks(data)

    def test_old_gnu_unmodelled_fields_rejected(self):
        data = fixture(rc=STOCK, outer_extra=(member('extras/gnu-probe', b'probe'),))
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w:', format=tarfile.GNU_FORMAT) as tf:
            for entry in unpack(data).entries:
                tf.addfile(copy.deepcopy(entry.info), io.BytesIO(entry.data) if entry.info.isreg() else None)
        raw = bytearray(stream.getvalue())
        cursor = 0
        found = False
        while any(raw[cursor:cursor + 512]):
            block = bytes(raw[cursor:cursor + 512])
            header = tarfile.TarInfo.frombuf(block, 'utf-8', 'surrogateescape')
            if header.name == 'extras/gnu-probe':
                block = bytearray(block)
                block[345:357] = b'00000000001\0'
                block[148:156] = b'        '
                block[148:156] = ('%06o\0 ' % sum(block)).encode()
                raw[cursor:cursor + 512] = block
                found = True
                break
            cursor += 512 + ((header.size + 511) // 512) * 512
        self.assertTrue(found)
        unsupported = gzip.compress(raw)
        unpack(unsupported)
        with self.assertRaisesRegex(tool.ToolError, 'archive_variant'):
            tool.extra_archive_checks(unsupported)

    def test_input_symlink_fifo_and_secret_permissions(self):
        link = self.root / 'link'
        link.symlink_to(self.original)
        with self.assertRaises(tool.ToolError):
            tool.read_regular(link, 100000)
        fifo = self.root / 'fifo'
        os.mkfifo(fifo)
        with self.assertRaisesRegex(tool.ToolError, 'input_not_regular'):
            tool.read_regular(fifo, 100)
        self.original.chmod(0o644)
        with self.assertRaisesRegex(tool.ToolError, 'secret_permissions'):
            tool.read_regular(self.original, 100000, secret=True)

    def test_failed_write_has_no_completion_receipt(self):
        original_writer = tool.write_new
        def fail_candidate(path, data):
            if path.name == 'back_parameter.adb.enc':
                raise OSError('synthetic disk error')
            return original_writer(path, data)
        with patch.object(tool, 'write_new', side_effect=fail_candidate):
            with self.assertRaises(OSError):
                tool.prepare(args(self.original, self.output), PASSWORD)
        self.assertFalse((self.output / 'verification.json').exists())

    def test_failed_persisted_verify_has_no_completion_receipt(self):
        original_verifier = tool.verify_package
        calls = 0
        def fail_second(*arguments):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise tool.ToolError('output_verification')
            return original_verifier(*arguments)
        with patch.object(tool, 'verify_package', side_effect=fail_second):
            with self.assertRaises(tool.ToolError):
                tool.prepare(args(self.original, self.output), PASSWORD)
        self.assertEqual(calls, 2)
        self.assertFalse((self.output / 'verification.json').exists())

    def test_getpass_warning_does_not_fall_back_to_echo(self):
        with patch.object(tool.sys.stdin, 'isatty', return_value=True), \
             patch.object(tool.getpass, 'getpass', side_effect=tool.getpass.GetPassWarning):
            with self.assertRaisesRegex(tool.ToolError, 'terminal_required'):
                tool.hidden_prompt('synthetic prompt')

    def test_complete_password_prompt_and_private_file(self):
        a = argparse.Namespace(password_prompt=True, password_file=None, suffix_file=None)
        with patch.object(tool, 'hidden_prompt', return_value=PASSWORD):
            self.assertEqual(tool.load_password(a), PASSWORD)
        secret = self.root / 'synthetic-key'
        secret.write_text(PASSWORD + '\n')
        secret.chmod(0o600)
        a.password_prompt, a.password_file = False, str(secret)
        self.assertEqual(tool.load_password(a), PASSWORD)

    def test_secrets_not_in_child_argv_or_environment(self):
        run = tool.subprocess.run
        captured = []
        def observe(command, **kw):
            self.assertNotIn(PASSWORD, repr(command))
            self.assertNotIn('env', kw)
            self.assertIn('-pass', command)
            self.assertTrue(command[-1].startswith('fd:'))
            self.assertEqual(len(kw['pass_fds']), 1)
            captured.append(command)
            return run(command, **kw)
        with patch.object(tool.subprocess, 'run', side_effect=observe):
            tool.crypt(self.plain, PASSWORD, encrypt=True)
        self.assertEqual(len(captured), 1)

    def test_password_pipe_when_standard_fds_closed(self):
        program = '''import fcntl, os, runpy, sys
sys.path.insert(0, sys.argv[1])
ns=runpy.run_path(sys.argv[1]+'/__main__.py',run_name='synthetic_fd_test')
report=fcntl.fcntl(1,fcntl.F_DUPFD,20)
for fd in (0,1,2): os.close(fd)
payload=b'synthetic payload'
try:
 c=ns['crypt'](payload,'synthetic-password',encrypt=True)
 assert ns['crypt'](c,'synthetic-password',encrypt=False)==payload
 os.write(report,b'PASS')
except BaseException:
 os.write(report,b'FAIL')
 os._exit(1)
'''
        result = subprocess.run([sys.executable, '-c', program, str(HERE)],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
        self.assertEqual((result.returncode, result.stdout), (0, b'PASS'))


if __name__ == '__main__':
    unittest.main()
