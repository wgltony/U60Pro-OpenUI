"""Property-style literal round trips; no modem, network or shell evaluation."""
import importlib.util
from pathlib import Path
import random
import sys
from types import SimpleNamespace
import unittest

ROOT = Path(__file__).resolve().parents[1]


def load_deploy():
    sys.path.insert(0, str(ROOT / 'scripts'))
    try:
        spec = importlib.util.spec_from_file_location('credentials_property_deploy', ROOT / 'scripts/deploy-components.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    finally:
        sys.path.pop(0)


class LiteralProperties(unittest.TestCase):
    def test_generated_literal_values_round_trip_without_evaluation(self):
        deploy = load_deploy()
        randomizer = random.Random(0xB31)
        alphabet = "abcXYZ012 '-\"\\\n\r\t$`#;()[]{}!=中é"
        passwords = [
            "first\nexport ZTE_AGENT_PASSWORD='not-an-export'\nlast",
            "first\nexport ZTE_AGENT_PIN=123456\nlast",
            "first\nunset ZTE_AGENT_PIN\nlast",
            "$(must_not_run); `must_not_run` \\ ' \" #",
        ]
        passwords += [''.join(randomizer.choice(alphabet) for _ in range(randomizer.randint(1, 160))) for _ in range(150)]
        for index, password in enumerate(passwords):
            for pin in ('', '012345'):
                with self.subTest(case=index, pin_enabled=bool(pin)):
                    script = deploy.render_startup(password, pin).decode()
                    modem = SimpleNamespace(shell=lambda _command, text=script: text.encode().hex())
                    self.assertEqual(deploy.current_credentials(modem), (password, pin))

    def test_non_posix_unquoted_whitespace_never_silently_truncates(self):
        deploy = load_deploy()
        # These are literal shell word bytes, not POSIX space/tab separators.
        # Unsupported records must fail rather than silently changing a password.
        for char in ('\r', '\v', '\f', '\x1c', '\x1d', '\x1e', '\x1f',
                     '\x85', '\xa0', '\u1680', '\u2028', '\u2029',
                     '\u202f', '\u205f', '\u3000'):
            for tail in (char, char + '#not-a-shell-comment'):
                with self.subTest(codepoint=ord(char), comment_like=tail != char):
                    script = 'export ZTE_AGENT_PASSWORD=secret' + tail + '\nunset ZTE_AGENT_PIN\n'
                    modem = SimpleNamespace(shell=lambda _command, text=script: text.encode().hex())
                    with self.assertRaises(RuntimeError):
                        deploy.current_credentials(modem)


if __name__ == '__main__':
    unittest.main()
