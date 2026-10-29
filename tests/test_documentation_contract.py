"""Read-only documentation regression checks; no network, secrets or device tools.

These tests protect wording/path contracts, not physical installation qualification.
They never import installers or execute commands extracted from the documentation.
"""
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
READMES = ("README.md", "README.en.md", "README.zh-TW.md")
ENTRANCES = READMES + (
    "docs/INSTALL.md", "docs/INSTALL.zh.md", "docs/ADB.md", "docs/ADB.zh.md",
    "docs/DEPLOYMENT.md", "docs/B31-UNLOCK-STATUS.md", "installer/README.md",
)
CURRENT = ENTRANCES + (
    "docs/OPERATIONS.md", "docs/SAFETY.md", "docs/WIFI-UPLINK.md",
    "docs/DASHBOARD.md", "docs/INSTALLER-WIREFRAMES.md",
)


def text(name):
    return (ROOT / name).read_text(encoding="utf-8")


def compact(name):
    return " ".join(text(name).split())


def shell_blocks(name):
    return re.findall(r"^```(?:sh|bash)\n(.*?)^```", text(name), re.M | re.S)


class DocumentationContractTests(unittest.TestCase):
    def test_supported_entrances_explain_usb_acknowledgement(self):
        for name in ENTRANCES:
            with self.subTest(document=name):
                body = text(name)
                for required in ("install.sh", "USB", "Wi-Fi", "YES",
                                 "--acknowledge-usb", "--adb-serial"):
                    self.assertIn(required, body)
                self.assertNotIn(".env.local", body)
        for name in ("README.en.md", "docs/INSTALL.md", "docs/DEPLOYMENT.md"):
            with self.subTest(document=name):
                body = compact(name)
                self.assertIn("USB DATA cable", body)
                self.assertIn("network access", body)
                self.assertRegex(body, r"(?:Wi-Fi only|Wi-Fi alone)")

    def test_current_shell_examples_do_not_recommend_old_setup_or_source(self):
        for name in CURRENT:
            with self.subTest(document=name):
                commands = "\n".join(shell_blocks(name))
                self.assertNotRegex(commands, r"(?m)^\s*(?:bash\s+|\./)setup\.sh\b")
                self.assertNotRegex(commands, r"(?m)^\s*(?:source|\.)\s+.*(?:start_|\.env\.local)")
                self.assertNotIn("strings /usr/sbin/zte_web", commands)
        for name in ("docs/DEPLOYMENT.md", "installer/README.md"):
            self.assertIn("no qualified desktop distribution", compact(name))
            self.assertNotIn("https://github.com/dklasens/MU5250-OpenUI/releases/latest", text(name))

    def test_operations_host_paths_stay_at_repository_root(self):
        body = text("docs/OPERATIONS.md")
        self.assertIn("All host commands run from the repository root", body)
        commands = "\n".join(shell_blocks("docs/OPERATIONS.md"))
        self.assertNotRegex(commands, r"(?m)^\s*cd\s+[^\n]*scripts(?:/|\s|$)")
        self.assertNotRegex(commands, r"\bpython3\s+zunlock\.py\b")
        self.assertIn("python3 scripts/zunlock.py", commands)
        self.assertIn('--identity-file "$UNLOCK_WORK/identity.json"', commands)

    def test_source_build_docs_match_zigbuild_entry(self):
        for name in READMES + ("docs/INSTALL.md", "docs/INSTALL.zh.md", "docs/DEPLOYMENT.md"):
            with self.subTest(document=name):
                self.assertIn("cargo-zigbuild", text(name))
                self.assertIn("cargo zigbuild", text(name))
        # Static inspection only: do not run the installer or a build.
        self.assertIn("cargo zigbuild", text("install.sh"))

    def test_current_node_requirements_match_vite_engine(self):
        for name in READMES + ("docs/INSTALL.md", "docs/INSTALL.zh.md",
                               "docs/DEPLOYMENT.md", "docs/DASHBOARD.md",
                               "installer/README.md"):
            with self.subTest(document=name):
                self.assertIn("Node.js `^20.19.0 || >=22.12.0`", text(name))
                self.assertNotRegex(text(name), r"Node(?:\.js)?\s+20\+")

    def test_relay_docs_cover_persistent_guard_and_backup_management(self):
        marker = "/data/local/tmp/relay-band-guard.disabled"
        for name in ("docs/WIFI-UPLINK.md", "docs/SAFETY.md", "docs/DASHBOARD.md"):
            with self.subTest(document=name):
                body = compact(name)
                for required in (marker, "UCI", "same-band", "opposite-band", "USB"):
                    self.assertIn(required, body)
                self.assertNotIn("It never writes UCI", body)
                self.assertNotIn("All of it is runtime state: no UCI", body)
        self.assertIn("180-second boot grace", text("docs/WIFI-UPLINK.md"))
        guard = text("scripts/device/relay-band-guard.sh")
        self.assertIn("STATE=" + marker, guard)
        self.assertIn("BOOT_GRACE=180", guard)
        self.assertIn("openui-tailscale", text("docs/SAFETY.md"))
        self.assertIn("stock boot-critical", text("docs/SAFETY.md"))

    def test_relay_policy_does_not_claim_absolute_hardware_limit(self):
        for name in READMES + ("docs/INSTALL.md", "docs/INSTALL.zh.md"):
            with self.subTest(document=name):
                body = compact(name)
                self.assertIn("AP/STA", body)
                for unsupported in ("同一频段只能", "同一頻段只能", "无线芯片只能",
                                    "on different bands only", "a band carries either"):
                    self.assertNotIn(unsupported, body)
        for name in ("README.en.md", "docs/INSTALL.md"):
            with self.subTest(document=name):
                body = compact(name)
                self.assertIn("observed AP/STA conflicts", body)
                self.assertIn("not an absolute claim", body)

    def test_rollback_docs_do_not_upgrade_old_snapshot_coverage(self):
        for name in ("docs/DEPLOYMENT.md", "docs/OPERATIONS.md"):
            with self.subTest(document=name):
                body = compact(name)
                for required in ("legacy coverage only", "FOTA/wireless UCI", "sidecar"):
                    self.assertIn(required, body)
                self.assertIn("not", body)
                self.assertIn("guarantee", body)
                self.assertIn("power loss", body)

    def test_privacy_covers_computer_backups_and_decrypted_work(self):
        for name in ("docs/INSTALL.md", "docs/ADB.md", "docs/DEPLOYMENT.md"):
            with self.subTest(document=name):
                body = compact(name)
                for required in ("computer", "encrypted", "decrypted", "Tailscale", "temporary"):
                    self.assertIn(required, body)
                self.assertNotIn("identity stay on your router", body)
        self.assertIn("not assume", compact("docs/DEPLOYMENT.md"))
        self.assertIn("does not imply a confirmed leak", compact("docs/DEPLOYMENT.md"))

    def test_historical_contradictions_are_not_current_howto(self):
        self.assertNotIn(".env.local", text("docs/B31-UNLOCK-STATUS.md"))
        self.assertIn("Historical record", text("docs/archive/b31-unlock-status-2026-10-09.md"))
        self.assertIn("Historical audit", text("docs/archive/safety-audit-2026-08-09.md"))
        self.assertIn("archive/safety-audit-2026-08-09.md", text("docs/SAFETY.md"))
        self.assertNotIn("Tailscale module | skipped", text("docs/SAFETY.md"))

    def test_local_document_links_resolve_without_network(self):
        names = CURRENT + (
            "docs/archive/b31-unlock-status-2026-10-09.md",
            "docs/archive/safety-audit-2026-08-09.md",
        )
        for name in names:
            for target in re.findall(r"\]\(([^)]+)\)", text(name)):
                target = target.strip("<>").split("#", 1)[0]
                if not target or re.match(r"[a-z]+://", target):
                    continue
                with self.subTest(document=name, link=target):
                    self.assertTrue(((ROOT / name).parent / target).exists())


if __name__ == "__main__":
    unittest.main()
