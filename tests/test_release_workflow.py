"""Read-only release source contracts; no builds, network, YAML dependency or devices.

The small indentation matcher covers this repository's workflow layout, not arbitrary
YAML. Commands are inspected as text and are never executed by these tests.
"""
import ast
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
PAYLOADS = ("zte-agent", "dashboard-dist.tar.gz", "openui-screen", "lpac", "mihomo")
ZIG_URL = "https://ziglang.org/download/0.14.1/zig-x86_64-linux-0.14.1.tar.xz"
ZIG_SHA256 = "24aeeec8af16c381934a6cd7d95c807a8cb2cf7df9fa40d359aa884195c4716c"


def text(name):
    return (ROOT / name).read_text(encoding="utf-8")


def jobs(source):
    body = source.split("\njobs:\n", 1)[1]
    return dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:\n|\Z)", body, re.M | re.S))


class ReleaseWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.source = text(".github/workflows/release.yml")
        self.jobs = jobs(self.source)

    def test_tag_release_cannot_bypass_safety_or_component_checks(self):
        trigger = self.source.split("\npermissions:", 1)[0]
        self.assertIn("tags: ['v*']", trigger)
        self.assertEqual(set(self.jobs), {"checks", "agent", "dashboard", "screen", "lpac", "mihomo", "publish"})
        for name in ("checks", "lpac", "mihomo"):
            self.assertIn(f"uses: ./.github/workflows/{name}.yml", self.jobs[name])
        publish = self.jobs["publish"]
        needs = re.search(r"needs: \[([^\]]+)\]", publish)[1]
        self.assertEqual({name.strip() for name in needs.split(",")}, set(self.jobs) - {"publish"})
        self.assertNotRegex(publish, r"(?m)^    if:")
        self.assertNotIn("continue-on-error:", self.source)
        self.assertNotIn("always()", self.source)

    def test_only_publish_receives_repository_write_permission(self):
        self.assertIn("\npermissions:\n  contents: read\n", self.source)
        self.assertIn("    permissions:\n      contents: write\n", self.jobs["publish"])
        self.assertEqual(self.source.count("contents: write"), 1)

    def test_agent_and_screen_use_the_same_verified_zig_as_lpac(self):
        for name in ("agent", "screen"):
            with self.subTest(job=name):
                job = self.jobs[name]
                self.assertIn(ZIG_URL, job)
                self.assertIn(ZIG_SHA256, job)
                self.assertLess(job.index(ZIG_URL), job.index("sha256sum -c -"))
                self.assertLess(job.index("sha256sum -c -"), job.index("tar xJf"))
                self.assertIn('test "$("$RUNNER_TEMP/zig/zig" version)" = 0.14.1', job)
                self.assertIn('echo "$RUNNER_TEMP/zig" >> "$GITHUB_PATH"', job)
        self.assertNotIn("pipx", self.source)
        self.assertNotIn("python-zig", self.source)
        self.assertNotIn("ln -s", self.source)
        lpac = text(".github/workflows/lpac.yml")
        self.assertIn(ZIG_URL, lpac)
        self.assertIn(ZIG_SHA256, lpac)
        self.assertIn("ZIG_VERSION = '0.14.1'", text("scripts/prepare-lpac.py"))

    def test_payloads_come_from_real_builds_and_uploads_fail_closed(self):
        self.assertIn("cargo zigbuild --release --target aarch64-unknown-linux-musl -p zte-agent --locked", self.jobs["agent"])
        self.assertIn("npm ci\n          npm run build", self.jobs["dashboard"])
        self.assertIn("tar czf dashboard-dist.tar.gz -C web-app/dist .", self.jobs["dashboard"])
        self.assertIn("ZIG=zig sh screen/build.sh", self.jobs["screen"])
        for name in ("agent", "dashboard", "screen"):
            self.assertIn("if-no-files-found: error", self.jobs[name])
        for name, command in (("lpac", "python3 scripts/prepare-lpac.py"),
                              ("mihomo", "python3 scripts/prepare-mihomo.py --docker --output build/mihomo/mihomo")):
            workflow = text(f".github/workflows/{name}.yml")
            self.assertIn(command, workflow)
            self.assertNotIn("--no-verify", workflow)

    def test_staged_assets_and_checksums_cover_exactly_five_payloads(self):
        publish = self.jobs["publish"]
        copies = re.findall(r"(?m)^          cp (artifacts/\S+) (assets/\S*)$", publish)
        self.assertEqual(dict(copies), {
            "artifacts/zte-agent/zte-agent": "assets/zte-agent",
            "artifacts/dashboard-dist/dashboard-dist.tar.gz": "assets/",
            "artifacts/lpac-linux-arm64/lpac": "assets/lpac",
            "artifacts/openui-screen/openui-screen": "assets/openui-screen",
            "artifacts/mihomo-linux-arm64/mihomo": "assets/mihomo",
        })
        names = re.search(r"sha256sum (.+) > sha256sums.txt", publish)[1].split()
        self.assertEqual(set(names), set(PAYLOADS))
        self.assertEqual(len(names), len(PAYLOADS))
        self.assertIn('gh release create "$TAG" "${ARGS[@]}" assets/*', publish)

    def test_release_stays_draft_and_tag_does_not_publish_desktop(self):
        publish = self.jobs["publish"]
        self.assertIn('ARGS=(--draft --verify-tag --title "$TAG"', publish)
        self.assertNotIn("gh release edit", self.source)
        self.assertNotIn("--draft=false", self.source)
        self.assertNotIn("installer.yml", self.source)
        desktop_trigger = text(".github/workflows/installer.yml").split("\npermissions:", 1)[0]
        self.assertIn("  workflow_dispatch:", desktop_trigger)
        self.assertNotRegex(desktop_trigger, r"(?m)^  (push|release|workflow_run):")

    def test_screen_dependencies_are_source_pinned_and_build_uses_devui(self):
        self.assertIn('exec python3 "$root/screen/devui/build.py"', text("screen/build.sh"))
        source = text("screen/devui/build.py")
        tree = ast.parse(source)
        pins = next(ast.literal_eval(node.value) for node in tree.body
                    if isinstance(node, ast.Assign)
                    and any(isinstance(target, ast.Name) and target.id == "PINS" for target in node.targets))
        self.assertEqual(set(pins), {"litehtml", "freetype", "unifont"})
        for url, digest in pins.values():
            self.assertTrue(url.startswith("https://"))
            self.assertRegex(digest, r"^[0-9a-f]{64}$")
        self.assertIn("hashlib.sha256(p.read_bytes()).hexdigest() != digest", source)
        self.assertIn("'aarch64-linux-musl'", source)
        self.assertIn("['-static','-s']", source)
        self.assertIn("'build/screen/openui-screen'", source)


if __name__ == "__main__":
    unittest.main()
