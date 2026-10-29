"""Offline regressions for install-audit fixes; every device/network/build tool is fake."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch


ROOT = Path(__file__).resolve().parents[1]
FORK = 'wgltony/U60Pro-OpenUI'
BASH = shutil.which('bash')


def load(name):
    sys.path.insert(0, str(ROOT / 'scripts'))
    try:
        spec = importlib.util.spec_from_file_location(name.replace('-', '_'), ROOT / 'scripts' / f'{name}.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    finally:
        sys.path.remove(str(ROOT / 'scripts'))


# Runs only in a disposable fixture checkout with a closed PATH and private HOME.
TOOL_STUB = r'''import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['AUDIT_LOG'], 'a') as log:
    log.write(json.dumps([name, *args]) + '\n')
if name == 'ssh': sys.exit(1)
if name == 'git':
    print(os.environ.get('MOCK_ORIGIN', 'https://github.com/wgltony/U60Pro-OpenUI.git'))
elif name == 'adb':
    if args == ['devices']: print('List of devices attached\n' + os.environ.get('MOCK_ADB_DEVICES', ''))
    elif args[-1:] == ['get-state']: sys.exit(1)
    else: sys.exit('Unexpected mock ADB command')
elif name == 'cargo':
    if args == ['zigbuild', '--version']:
        if os.environ.get('MOCK_NO_ZIGBUILD'): sys.exit(1)
        print('cargo-zigbuild 0.20.0')
    else:
        path = pathlib.Path('target/aarch64-unknown-linux-musl/release/zte-agent')
        path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b'mock agent')
elif name == 'rustup': print(os.environ.get('MOCK_TARGET', 'aarch64-unknown-linux-musl'))
elif name == 'zig': print(os.environ.get('MOCK_ZIG', '0.14.1'))
elif name == 'npm':
    pathlib.Path('dist').mkdir(exist_ok=True)
    pathlib.Path('dist/index.html').write_text('mock dashboard')
elif name == 'tar': pathlib.Path(args[1]).write_bytes(b'mock archive')
elif name == 'mv':
    source, destination = map(lambda value: pathlib.Path(value).resolve(), args)
    root = pathlib.Path.cwd().resolve()
    assert root in source.parents and root in destination.parents
    source.rename(destination)
elif name == 'python3':
    if args[0] == '-c': pass  # credential validation / ADB wait are recorded, never executed
    elif args[0].endswith('download-release.py'):
        if '--latest' in args: print('v1.2.3')
        else:
            output = pathlib.Path(args[args.index('--output') + 1]); output.mkdir(parents=True, exist_ok=True)
            for file in ['zte-agent', 'dashboard-dist.tar.gz', 'lpac', 'mihomo', 'openui-screen']:
                (output / file).write_bytes(b'mock release')
    elif args[0].endswith('deploy-components.py'):
        if '--bundle-output' in args: pathlib.Path(args[args.index('--bundle-output') + 1]).mkdir(parents=True, exist_ok=True)
        if '--release-agent' in args:
            path = pathlib.Path(args[args.index('--release-agent') + 1])
            path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b'mock release')
    elif '--output' in args:
        path = pathlib.Path(args[args.index('--output') + 1])
        if 'prepare-geodata.py' in args[0] or 'prepare-tailscale.py' in args[0]: path.mkdir(parents=True, exist_ok=True)
        else: path.write_bytes(b'mock tool')
'''


class MockCheckout:
    def __enter__(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / 'scripts').mkdir()
        (self.root / 'web-app').mkdir()
        (self.root / 'home').mkdir()
        self.bin = self.root / 'bin'; self.bin.mkdir()
        self.log = self.root / 'commands.jsonl'
        for script in ['install.sh', 'setup.sh', 'deploy.sh', 'scripts/build-offline-bundle.sh']:
            shutil.copyfile(ROOT / script, self.root / script)
        for tool in ['dirname', 'mktemp', 'rm', 'mkdir', 'grep', 'cp', 'uname']:
            (self.bin / tool).symlink_to(shutil.which(tool))
        for tool in ['python3', 'ssh', 'adb', 'git', 'cargo', 'rustup', 'zig', 'npm', 'node',
                     'openssl', 'ssh-keygen', 'curl', 'tar', 'mv']:
            path = self.bin / tool
            path.write_text(f'#!{sys.executable}\n' + TOOL_STUB)
            path.chmod(0o700)
        self.env = {'PATH': str(self.bin), 'HOME': str(self.root / 'home'), 'AUDIT_LOG': str(self.log),
                    'ZTE_AGENT_PASSWORD': 'mock-only', 'ROUTER_PW': 'mock-only', 'ZTE_BACKUP_SUFFIX': 'mock-only',
                    'ZTE_DRY_RUN': '0'}
        return self

    def run(self, script, args=(), input='', **env):
        return subprocess.run([BASH, script, *args], cwd=self.root, env={**self.env, **env},
                              input=input, capture_output=True, text=True, timeout=20)

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def __exit__(self, *args):
        self.temporary.cleanup()


@unittest.skipUnless(BASH and os.name != 'nt', 'isolated POSIX shell stubs required')
class WrapperAuditTests(unittest.TestCase):
    def test_exact_github_origin_and_no_silent_other_repository(self):
        origins = [f'https://github.com/{FORK}.git', f'git@github.com:{FORK}.git', f'ssh://git@github.com/{FORK}.git']
        for origin in origins:
            with self.subTest(origin=origin), MockCheckout() as fixture:
                result = fixture.run('install.sh', ['--skip-screen', '--dry-run', '--acknowledge-usb'], MOCK_ORIGIN=origin)
                self.assertEqual(result.returncode, 0, result.stderr)
                downloads = [c for c in fixture.commands() if c[0] == 'python3' and c[1].endswith('download-release.py')]
                self.assertTrue(downloads)
                self.assertTrue(all(c[c.index('--repo') + 1] == FORK for c in downloads))
        for origin in ['https://github.com/other/upstream.git', 'https://github.com.evil/owner/repo.git',
                       f'https://github.com/{FORK}/extra.git', f'https://evil.invalid/github.com/{FORK}.git', '']:
            with self.subTest(origin=origin), MockCheckout() as fixture:
                result = fixture.run('install.sh', ['--skip-screen', '--dry-run'], MOCK_ORIGIN=origin)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(c[0] in ['ssh', 'adb', 'curl'] or 'download-release.py' in ' '.join(c) for c in fixture.commands()))

    def test_locked_install_requires_acknowledgement_before_unlock_even_with_empty_adb(self):
        for script, args, input in [('install.sh', ['--skip-screen'], ''), ('setup.sh', ['router', 'agent'], '1\n')]:
            with self.subTest(script=script), MockCheckout() as fixture:
                result = fixture.run(script, args, input=input)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('--acknowledge-usb', result.stderr)
                self.assertIn('USB DATA cable', result.stdout)
                self.assertFalse(any('zunlock.py' in ' '.join(c) for c in fixture.commands()))
                self.assertFalse(any('deploy-components.py' in ' '.join(c) and '--prepare-only' not in c for c in fixture.commands()))

    def test_acknowledged_dry_run_never_performs_restore_or_deployment(self):
        for script, args, input, env in [
            ('install.sh', ['--skip-screen', '--acknowledge-usb', '--dry-run'], '', {}),
            ('setup.sh', ['--acknowledge-usb', 'router', 'agent'], '1\n', {'ZTE_DRY_RUN': '1'}),
        ]:
            with self.subTest(script=script), MockCheckout() as fixture:
                result = fixture.run(script, args, input=input, **env)
                self.assertEqual(result.returncode, 0, result.stderr)
                unlocks = [c for c in fixture.commands() if 'zunlock.py' in ' '.join(c)]
                self.assertEqual(len(unlocks), 1)
                self.assertIn('--dry-run', unlocks[0])
                self.assertFalse(any('deploy-components.py' in ' '.join(c) and '--prepare-only' not in c for c in fixture.commands()))

    def test_serial_is_pinned_after_mock_unlock(self):
        for script, args, input in [
            ('install.sh', ['--skip-screen', '--acknowledge-usb', '--adb-serial', 'chosen'], ''),
            ('setup.sh', ['--acknowledge-usb', '--adb-serial', 'chosen', 'router', 'agent'], '1\n'),
        ]:
            with self.subTest(script=script), MockCheckout() as fixture:
                result = fixture.run(script, args, input=input, MOCK_ADB_DEVICES='other\tdevice\n')
                self.assertEqual(result.returncode, 0, result.stderr)
                calls = fixture.commands()
                self.assertIn(['adb', '-s', 'chosen', 'get-state'], calls)
                self.assertNotIn(['adb', 'devices'], calls)
                waits = [c for c in calls if c[0:2] == ['python3', '-c'] and 'wait-for-device' in c[2]]
                self.assertEqual(len(waits), 1)
                self.assertEqual(waits[0][-1], 'chosen')
                deploy = [c for c in calls if 'deploy-components.py' in ' '.join(c) and '--prepare-only' not in c]
                self.assertEqual(len(deploy), 1)
                self.assertEqual(deploy[0][deploy[0].index('--adb-serial') + 1], 'chosen')
                self.assertNotIn('--auto-adb', deploy[0])
                self.assertIn('--identity-file', deploy[0])

    def test_source_builds_use_zigbuild_and_preflight_all_four_wrappers(self):
        cases = [('install.sh', ['--from-source', '--skip-screen', '--dry-run', '--acknowledge-usb'], ''),
                 ('setup.sh', ['--acknowledge-usb', 'router', 'agent'], '2\n'),
                 ('deploy.sh', ['--dry-run'], ''), ('scripts/build-offline-bundle.sh', ['output'], '')]
        for script, args, input in cases:
            with self.subTest(script=script), MockCheckout() as fixture:
                result = fixture.run(script, args, input=input)
                self.assertEqual(result.returncode, 0, result.stderr)
                builds = [c for c in fixture.commands() if c[0] == 'cargo' and '--release' in c]
                self.assertEqual(builds, [['cargo', 'zigbuild', '--locked', '--release', '--target', 'aarch64-unknown-linux-musl', '-p', 'zte-agent']])
            for override, message in [({'MOCK_NO_ZIGBUILD': '1'}, 'cargo-zigbuild'),
                                      ({'MOCK_TARGET': 'x86_64-unknown-linux-gnu'}, 'rustup target add'),
                                      ({'MOCK_ZIG': '0.13.0'}, 'Zig 0.14.1')]:
                with self.subTest(script=script, override=override), MockCheckout() as fixture:
                    result = fixture.run(script, args, input=input, **override)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(message, result.stderr)
                    self.assertFalse(any(c[0] in ['ssh', 'adb', 'curl'] or '--release' in c for c in fixture.commands()))

    def test_legacy_explicit_repo_is_forwarded_without_hardcoded_upstream(self):
        with MockCheckout() as fixture:
            result = fixture.run('setup.sh', ['--repo', FORK, '--acknowledge-usb', 'router', 'agent'], input='1\n', ZTE_DRY_RUN='1')
            self.assertEqual(result.returncode, 0, result.stderr)
            prepare = next(c for c in fixture.commands() if '--release-agent' in c)
            self.assertEqual(prepare[prepare.index('--release-repo') + 1], FORK)
        for path in ['setup.sh', 'installer/src-tauri/src/deploy.rs', 'scripts/deploy-components.py']:
            self.assertNotIn('dklasens/MU5250-OpenUI', (ROOT / path).read_text())


class ReleasePreparationAuditTests(unittest.TestCase):
    def setUp(self):
        self.module = load('deploy-components')
        self.downloader = load('download-release')

    def prepare(self, directory, origin, repo=None, bad_metadata=False):
        binary = b'offline release fixture'
        sums = hashlib.sha256(binary).hexdigest().encode() + b'  zte-agent\n'
        urls = []
        def get(url, limit=None):
            urls.append(url)
            if '/repos/' in url:
                return json.dumps({'tag_name': '../invalid' if bad_metadata else 'v1.2.3',
                                   'assets': [{'name': 'zte-agent', 'browser_download_url': 'https://untrusted.invalid/agent'}]}).encode()
            if url.endswith('sha256sums.txt'): return sums
            if url.endswith('/zte-agent'): return binary
            raise AssertionError(url)
        args = SimpleNamespace(bundle_output=str(directory / 'packages'), bundle=None,
                               release_agent=str(directory / 'agent'), release_repo=repo)
        with patch.object(self.module, 'run', return_value=(0, origin, '')), \
             patch.object(self.module, 'prepare_hardening'), \
             patch.object(self.module, 'load_script', return_value=self.downloader), \
             patch.object(self.downloader, 'get', side_effect=get), contextlib.redirect_stdout(io.StringIO()):
            self.module.prepare_only(args)
        return urls, binary

    def test_release_uses_current_fork_and_constructed_exact_origin_assets(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            urls, binary = self.prepare(directory, f'https://github.com/{FORK}.git')
            self.assertEqual(urls, [f'https://api.github.com/repos/{FORK}/releases/latest',
                                    f'https://github.com/{FORK}/releases/download/v1.2.3/sha256sums.txt',
                                    f'https://github.com/{FORK}/releases/download/v1.2.3/zte-agent'])
            self.assertEqual((directory / 'agent').read_bytes(), binary)

    def test_bad_origins_and_tags_fail_without_destination_or_fallback(self):
        for origin in ['https://github.com/other/upstream.git', f'https://github.com/{FORK}/extra',
                       f'https://github.com.evil/{FORK}.git', f'file:///github.com/{FORK}', '']:
            with self.subTest(origin=origin), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                with self.assertRaises(ValueError): self.prepare(directory, origin)
                self.assertFalse((directory / 'agent').exists())
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'Invalid release tag'):
                self.prepare(directory, f'https://github.com/{FORK}.git', bad_metadata=True)
            self.assertFalse((directory / 'agent').exists())

    def test_missing_fork_release_never_falls_back_or_contacts_device(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            args = SimpleNamespace(bundle_output=str(directory / 'packages'), bundle=None,
                                   release_agent=str(directory / 'agent'), release_repo=FORK)
            with patch.object(self.module, 'prepare_hardening'), \
                 patch.object(self.module, 'load_script', return_value=self.downloader), \
                 patch.object(self.downloader, 'get', side_effect=RuntimeError('mock fork release unavailable')) as get, \
                 patch.object(self.module, 'Transport', side_effect=AssertionError('Device contact forbidden')):
                with self.assertRaisesRegex(RuntimeError, 'mock fork release unavailable'): self.module.prepare_only(args)
            get.assert_called_once_with(f'https://api.github.com/repos/{FORK}/releases/latest', 1024 * 1024)
            self.assertFalse((directory / 'agent').exists())

    def test_explicit_repo_works_without_git_origin(self):
        with tempfile.TemporaryDirectory() as temporary:
            urls, _ = self.prepare(Path(temporary), '', repo=FORK)
            self.assertTrue(all(FORK in url for url in urls))

    def test_gui_is_fail_closed_before_prerequisites_or_cached_bundle(self):
        text = (ROOT / 'installer/src-tauri/src/deploy.rs').read_text()
        install = text[text.index('pub fn perform_install('):]
        self.assertIn(f'const REPOSITORY: &str = "{FORK}";', text)
        self.assertIn('const DESKTOP_RELEASES_SUPPORTED: bool = false;', text)
        self.assertLess(install.index('if !DESKTOP_RELEASES_SUPPORTED'), install.index('crate::host::prerequisites'))
        self.assertLess(install.index('return Err('), install.index('crate::bundle::find_cached'))
        self.assertIn('No compatible downloadable desktop distribution is promised.', install)
        self.assertIn('Deployment writes are disabled before restore/upload/activation; earlier detection may read device status.', install)
        self.assertNotIn('Deployment is disabled before device access', install)


class CredentialsAuditTests(unittest.TestCase):
    def setUp(self):
        self.module = load('deploy-components')
        self.startup = load('render-agent-startup')

    def read(self, script):
        transport = SimpleNamespace(shell=Mock(return_value=script.encode().hex(' ')))
        with patch.object(self.module, 'run', side_effect=AssertionError('No execution allowed')):
            result = self.module.current_credentials(transport)
        transport.shell.assert_called_once_with('od -An -tx1 -v /data/local/tmp/start_zte_agent.sh')
        return result

    def test_roundtrip_multiline_quoted_and_metacharacters_exactly(self):
        passwords = ["a'b\n$(never-executed); `no` & | < > $HOME \\",
                     "first\nexport ZTE_AGENT_PASSWORD='not-an-assignment'\nlast",
                     " space\r\n中文\ntrailing \n", 'plain@safe-123']
        for password in passwords:
            for pin in ['', '012345']:
                with self.subTest(password=password, pin=pin):
                    self.assertEqual(self.read(self.startup.render(password, pin)), (password, pin))
        self.assertEqual(self.read("export ZTE_AGENT_PASSWORD='a'\\''b'\nunset ZTE_AGENT_PIN\n"), ("a'b", ''))
        self.assertEqual(self.read('export ZTE_AGENT_PASSWORD="literal \\$HOME; \\`x\\` \\\\ end"\n'), ('literal $HOME; `x` \\ end', ''))

    def test_hex_transport_preserves_credentials_across_legacy_adb_crlf(self):
        password = " \r\nfirst'\\$`\nlast\r\n "
        script = self.startup.render(password, '001234')
        modem = self.module.Transport('192.168.0.1', 'mock-serial')
        output = script.encode().hex(' ').encode() + b'\r\r\n__U60_RESULT__0\r\r\n'
        with patch.object(self.module, 'run', return_value=self.module.outcome(0, output, b'', False)) as run:
            self.assertEqual(self.module.current_credentials(modem), (password, '001234'))
        self.assertIn('od -An -tx1 -v /data/local/tmp/start_zte_agent.sh', run.call_args.args[0][-1])

    def test_encoded_read_failures_are_explicit_and_never_fall_back_to_cat(self):
        for output in ['not hexadecimal', 'ff fe', '0']:
            modem = SimpleNamespace(shell=Mock(return_value=output))
            with self.subTest(output=output), self.assertRaisesRegex(RuntimeError, 'Cannot safely read installed credentials'):
                self.module.current_credentials(modem)
            modem.shell.assert_called_once_with('od -An -tx1 -v /data/local/tmp/start_zte_agent.sh')
        modem = SimpleNamespace(shell=Mock(side_effect=RuntimeError('mock od missing')))
        with self.assertRaisesRegex(RuntimeError, 'od is required'): self.module.current_credentials(modem)
        self.assertEqual(modem.shell.call_count, 1)

    def test_escaped_trailing_space_continuations_and_double_quoted_newlines(self):
        self.assertEqual(self.read('export ZTE_AGENT_PASSWORD=trailing\\ \nunset ZTE_AGENT_PIN \t\n'), ('trailing ', ''))
        self.assertEqual(self.read('export ZTE_AGENT_PASSWORD=pa\\\nss\n'), ('pass', ''))
        self.assertEqual(self.read('export ZTE_AGENT_PASSWORD="one\ntwo \\$HOME"\n'), ('one\ntwo $HOME', ''))

    def test_dynamic_executable_ambiguous_and_invalid_values_are_rejected(self):
        scripts = ['export ZTE_AGENT_PASSWORD=$(touch /never)\n',
                   'export ZTE_AGENT_PASSWORD="$HOME"\n', 'export ZTE_AGENT_PASSWORD=`id`\n',
                   "export ZTE_AGENT_PASSWORD='safe'; touch /never\n",
                   "export ZTE_AGENT_PASSWORD='one'\nexport ZTE_AGENT_PASSWORD='two'\n",
                   "export ZTE_AGENT_PASSWORD='safe'\nZTE_AGENT_PASSWORD=changed\n",
                   "export ZTE_AGENT_PASSWORD='unterminated\n", "export ZTE_AGENT_PASSWORD=''\n",
                   "export ZTE_AGENT_PASSWORD='safe'\nexport ZTE_AGENT_PIN='123456'\nunset ZTE_AGENT_PIN\n",
                   "export ZTE_AGENT_PASSWORD='safe'\nexport ZTE_AGENT_PIN='１２３４５６'\n",
                   "export ZTE_AGENT_PASSWORD='safe\0bad'\n"]
        for script in scripts:
            with self.subTest(script=script), self.assertRaisesRegex(RuntimeError, 'missing, ambiguous or dynamic'):
                self.read(script)

    def test_invalid_installed_credentials_stop_deploy_before_device_writes(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / 'agent'
            header = bytearray(64); header[:6] = b'\x7fELF\x02\x01'; header[18:20] = (183).to_bytes(2, 'little')
            binary.write_bytes(header)
            modem = SimpleNamespace(reachable=Mock(), identity=Mock(return_value={'model': 'MU5250', 'firmware': 'B31'}),
                                    shell=Mock(return_value='export ZTE_AGENT_PASSWORD=$(id)\n'.encode().hex(' ')), push=Mock(), serial='chosen')
            args = SimpleNamespace(agent=str(binary), dashboard=None, harden=False, bundle=None, gateway='192.168.0.1',
                                   auto_adb=False, adb_serial='chosen', dry_run=False, identity_file=None,
                                   keep_credentials=True, skip_unchanged=False)
            with patch.object(self.module, 'Transport', return_value=modem), \
                 patch.object(self.module, 'run', side_effect=AssertionError('No subprocess allowed')):
                with self.assertRaisesRegex(RuntimeError, 'missing, ambiguous or dynamic'): self.module.deploy(args)
            modem.push.assert_not_called()
            modem.shell.assert_called_once_with('od -An -tx1 -v /data/local/tmp/start_zte_agent.sh')


if __name__ == '__main__':
    unittest.main()
