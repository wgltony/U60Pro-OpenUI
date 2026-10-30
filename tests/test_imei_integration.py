"""Offline IMEI packaging and deployment exclusion; no helper/device execution.

The native shell runs against the explicit disposable runtime fixture. Its JSON
predicate uses the fixture's Python adapter, not the device Lua runtime.
"""
import fcntl
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ImeiDeploymentGuard(unittest.TestCase):
    def setUp(self):
        runtime = load(ROOT / 'tests/test_transaction_runtime.py', 'imei_runtime_fixture')
        self.fixture = runtime.RuntimeTransactions('test_running_services_quiesce_before_replace_and_restore_original_versions')
        self.fixture.setUp()
        self.root = self.fixture.root
        self.state = self.root / 'data/openui-imei'

    def tearDown(self):
        self.fixture.tearDown()

    def private_state(self, journal=None):
        self.state.mkdir(parents=True, mode=0o700)
        lock = self.state / 'controller.lock'
        lock.write_bytes(b'')
        lock.chmod(0o600)
        if journal is not None:
            path = self.state / 'current.json'
            path.write_text(json.dumps(journal))
            path.chmod(0o600)
        return lock

    def assert_untouched(self):
        self.assertFalse(self.fixture.transaction.exists())
        self.assertFalse((self.fixture.transaction.parent / 'active').exists())
        self.assertFalse((self.root / 'events.log').exists())

    def test_first_install_creates_same_lock_before_active_marker(self):
        self.fixture.action('begin')
        self.assertTrue((self.state / 'controller.lock').is_file())
        self.assertEqual(self.state.stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.state / 'controller.lock').stat().st_mode & 0o777, 0o600)
        script = (ROOT / 'scripts/device/transaction.sh').read_text()
        begin = script.split('begin)\n', 1)[1]
        self.assertLess(begin.index('imei_deployment_guard'), begin.index('mkdir -p "$base"'))

    def test_busy_controller_lock_rejects_begin_without_snapshot(self):
        with self.private_state().open('r+b') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.fixture.action('begin', success=False)
        self.assert_untouched()

    def test_unfinished_or_uncertain_journal_never_replaces_helpers(self):
        for journal in [
            {'schema': 1, 'stage': 'writing', 'uncertain': False},
            {'schema': 1, 'stage': 'reboot_to_verify', 'uncertain': False},
            {'schema': 1, 'stage': 'complete', 'uncertain': True},
            {'schema': 2, 'stage': 'complete', 'uncertain': False},
            {'schema': True, 'stage': 'complete', 'uncertain': False},
            {'schema': 1, 'stage': 'complete'},
        ]:
            with self.subTest(journal=journal):
                if not self.state.exists():
                    self.private_state()
                path = self.state / 'current.json'
                path.write_text(json.dumps(journal)); path.chmod(0o600)
                self.fixture.action('begin', success=False)
                self.assert_untouched()

    def test_complete_journal_allows_snapshot_without_touching_backup(self):
        self.private_state({'schema': 1, 'stage': 'complete', 'uncertain': False})
        before = (self.state / 'current.json').read_bytes()
        self.fixture.action('begin')
        self.assertEqual((self.state / 'current.json').read_bytes(), before)
        targets = (self.fixture.transaction / 'targets').read_text()
        self.assertIn('data/local/tmp/openui-imei-helpers', targets)
        self.assertNotIn('data/openui-imei', targets)

    def test_private_directory_mode_and_symlink_are_not_repaired_implicitly(self):
        self.private_state()
        self.state.chmod(0o755)
        self.fixture.action('begin', success=False)
        self.assert_untouched()
        self.state.chmod(0o700)
        (self.state / 'current.json').symlink_to(self.root / 'etc/rc.local')
        self.fixture.action('begin', success=False)
        self.assert_untouched()


class ImeiHostIntegration(unittest.TestCase):
    def test_helpers_validated_before_any_transport_and_dry_run_does_not_push(self):
        fixture = load(ROOT / 'tests/test_deploy_portability.py', 'imei_deploy_fixture')
        module = fixture.load_deploy()
        args = SimpleNamespace(agent=None, dashboard=None, harden=False, auto_adb=False,
                               bundle=None, gateway='192.168.0.1', adb_serial=None,
                               dry_run=True, identity_file=None, keep_credentials=False,
                               skip_unchanged=False, imei_dir='synthetic-package')
        validator = SimpleNamespace(validate=lambda _: {'zte_nv': b'synthetic', 'manifest.json': b'{}'})
        modem = fixture.FakeModem()
        with patch.object(module, 'load_script', return_value=validator), \
             patch.object(module, 'Transport', return_value=modem), \
             patch.object(sys, 'stdout', io.StringIO()):
            module.deploy(args)
        self.assertFalse(any('zte_nv ' in command or 'zte_config ' in command for command, _ in modem.commands))
        invalid = SimpleNamespace(validate=lambda _: (_ for _ in ()).throw(ValueError('bad manifest')))
        with patch.object(module, 'load_script', return_value=invalid), \
             patch.object(module, 'Transport', side_effect=AssertionError('contacted before validation')):
            with self.assertRaisesRegex(ValueError, 'bad manifest'):
                module.deploy(args)

    def test_install_requires_explicit_source_feature_build_before_any_commands(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            touched = directory / 'unexpected-command'
            for name in ('adb', 'ssh', 'curl', 'zig', 'mktemp'):
                stub = directory / name
                stub.write_text('#!/bin/sh\ntouch "$TOUCHED"\nexit 91\n')
                stub.chmod(0o700)
            result = subprocess.run(['bash', str(ROOT / 'install.sh'), '--with-imei'],
                                    cwd=ROOT, env={**os.environ, 'PATH': str(directory) + ':' + os.environ['PATH'], 'TOUCHED': str(touched)},
                                    capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('--from-source', result.stderr)
            self.assertFalse(touched.exists())

    def test_mock_imei_get_is_bare_unavailable_and_post_never_fakes_success(self):
        mock = load(ROOT / 'web-app/tools/mock_agent.py', 'imei_mock_fixture')
        sent = []
        class Request:
            path = '/api/imei'
            def _send(self, value, status=200): sent.append((status, value))
            def _body(self): return {'action': 'begin'}
        mock.Handler.do_GET(Request())
        self.assertEqual(sent[-1][1]['phase'], 'unsupported')
        self.assertEqual(sent[-1][1]['allowed'], [])
        self.assertNotIn('data', sent[-1][1])
        mock.Handler.do_POST(Request())
        self.assertEqual(sent[-1], (409, {'ok': False, 'error': 'helper_unavailable'}))


if __name__ == '__main__':
    unittest.main()
