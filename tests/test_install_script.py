import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
BASH = shutil.which('bash')


def run(args, path):
    env = {**os.environ, 'PATH': path, 'ZTE_AGENT_PASSWORD': 'test-only'}
    return subprocess.run([BASH, 'install.sh', *args], cwd=ROOT, env=env, capture_output=True, text=True, timeout=30)


class InstallScriptTests(unittest.TestCase):
    def test_help_and_unknown_options(self):
        self.assertEqual(run(['--help'], os.environ['PATH']).returncode, 0)
        result = run(['--frobnicate'], os.environ['PATH'])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Unknown option', result.stderr)

    def test_missing_tools_stop_before_any_download_or_device_contact(self):
        with tempfile.TemporaryDirectory() as temp:
            stubs = Path(temp)
            touched = stubs / 'touched'
            # Only bash and the basics; every networked tool records that it ran.
            for tool in ['python3', 'curl', 'adb', 'ssh']:
                stub = stubs / tool
                stub.write_text(f"#!/bin/sh\n{shutil.which('touch')} '{touched}'\nexit 1\n")
                stub.chmod(0o700)
            for tool in ['mktemp', 'rm', 'dirname']:
                (stubs / tool).symlink_to(shutil.which(tool))
            result = run(['--dry-run'], str(stubs))
            self.assertFalse(touched.exists(), 'No tool may run before the computer check passes')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Missing:', result.stderr)


class StartupScriptTests(unittest.TestCase):
    def test_agent_log_and_optional_services(self):
        spec = importlib.util.spec_from_file_location('startup', ROOT / 'scripts/render-agent-startup.py')
        startup = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(startup)
        script = startup.render('secret', '')
        self.assertIn("| /data/zte-agent log-writer'", script)
        self.assertNotIn('logger', script)
        # Each optional service starts only when deployed.
        self.assertIn('[ -x /etc/init.d/openui-tailscale ] && /etc/init.d/openui-tailscale start', script)
        self.assertIn('[ -x /data/bin/relay-band-guard.sh ] && nohup /data/bin/relay-band-guard.sh', script)
        subprocess.run(['sh', '-n'], input=script, text=True, check=True)


if __name__ == '__main__':
    unittest.main()
