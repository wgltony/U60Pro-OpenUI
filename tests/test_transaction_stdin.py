"""Native procd parser must consume its captured JSON, not management stdin."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class NativeProcdInput(unittest.TestCase):
    def test_parser_uses_snapshot_even_when_management_stdin_remains_open(self):
        source = (ROOT / "scripts/device/transaction.sh").read_text()
        function = "procd_status() {" + source.split("procd_status() {", 1)[1].split("\n}\n", 1)[0] + "\n}\n"
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            received = base / "received.json"
            fake_lua = base / "lua"
            fake_lua.write_text(
                f"#!{sys.executable}\n"
                "import os, pathlib, sys\n"
                "pathlib.Path(os.environ['RECEIVED']).write_bytes(sys.stdin.buffer.read())\n"
                "print('absent')\n"
            )
            fake_lua.chmod(0o700)
            payload = b'{"openui-tailscale":{"instances":{}}}\n'
            environment = dict(os.environ, PATH=str(base) + os.pathsep + os.environ.get("PATH", ""),
                               RECEIVED=str(received), TRANSACTION=str(base))
            script = ('root=; transaction="$TRANSACTION"\n'
                      'runtime() { printf \'%s\\n\' \'{"openui-tailscale":{"instances":{}}}\'; }\n'
                      + function + "procd_status\n")
            script_file = base / "probe.sh"
            script_file.write_text(script)
            # Do NOT use communicate(input=...) before wait: it would close the
            # management stdin and mask the original native Lua EOF deadlock.
            process = subprocess.Popen(["sh", str(script_file)], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                       env=environment, start_new_session=True)
            try:
                process.wait(timeout=5)
                stdout, stderr = process.communicate()
                self.assertEqual(process.returncode, 0, stderr.decode(errors="replace"))
                self.assertEqual(stdout.strip(), b"absent")
                self.assertEqual(received.read_bytes(), payload)
                self.assertEqual((base / "procd.json").read_bytes(), payload)
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.communicate()


if __name__ == "__main__":
    unittest.main()
