#!/usr/bin/env python3
"""Offline synthetic host tests; never execute production/foreign helpers."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("native_build", ROOT / "build.py")
build = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(build)


def main():
    build.verified_sources()
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    for script in ("run_tests.py", "run_support_tests.py", "test_build.py"):
        subprocess.run([sys.executable, str(ROOT / "tests" / script)], cwd=ROOT,
                       env=env, check=True, timeout=240)
    print("ALL_NATIVE_OFFLINE_TESTS_PASS; synthetic fake DIAG only; no device helper execution")


if __name__ == "__main__":
    main()
