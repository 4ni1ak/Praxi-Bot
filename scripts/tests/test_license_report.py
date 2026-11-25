"""Hermetic tests for scripts/license_report.py (no network, no cargo, no pnpm)."""

from datetime import date
import importlib.util
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
from textwrap import dedent
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "license_report.py"


def load_module():
    spec = importlib.util.spec_from_file_location("license_report", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class LicenseReportTests(unittest.TestCase):
    def setUp(self):
        self.mod = load_module()
        self.tmp = TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def write(self, rel: str, content: str) -> Path:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(dedent(content).lstrip("\n"), encoding="utf-8")
        return path

    def seed_minimal_repo(self, extra_rust=(), extra_notices="", ignores=()):
        ignore_lines = "\n".join(ignores)
        self.write(
            "deny.toml",
            f"""
            [licenses]
            allow = ["MIT", "Apache-2.0", "MPL-2.0", "IJG", "OFL-1.1", "ISC"]

            [[licenses.exceptions]]
            allow = ["bzip2-1.0.6"]
            crate = "libbz2-rs-sys"

            [advisories]
            ignore = [
            {ignore_lines}
            ]
            """,
        )
        self.write(
            "pnpm-lock.yaml",
            """
            lockfileVersion: '9.0'
            importers:
              apps/desktop/ui:
                dependencies:
                  react:
                    specifier: ^19.0.0
                    version: 19.3.1
            packages:
              react@19.3.1:
                resolution: {integrity: sha512-dummy}
            snapshots:
              react@19.3.1: {}
            """,
        )
        self.write("apps/desktop/ui/public/licenses/geist.txt", "SIL OFL 1.1\n")
        self.write(
            "THIRD_PARTY_NOTICES.md",
            f"""
            # Third-party notices

            Inventory: [docs/third-party-licenses.md](docs/third-party-licenses.md)

            ## Geist
            Full notice in `apps/desktop/ui/public/licenses/geist.txt`.

            ## bzip2
            `libbz2-rs-sys` (`bzip2-1.0.6`).
            {extra_notices}
            """,
        )
        rust_packages = [
            {
                "name": "app",
                "version": "0.1.0",
                "source": None,
                "license": "Apache-2.0",
                "repository": None,
            },
            {
                "name": "serde",
                "version": "1.0.0",
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "license": "MIT OR Apache-2.0",
                "repository": "https://github.com/serde-rs/serde",
            },
            {
                "name": "r-efi",
                "version": "5.3.0",
                "source": "registry+https://github.com/rust-lang/crates.io-index",
                "license": "MIT OR Apache-2.0 OR LGPL-2.1-or-later",
                "repository": "https://github.com/r-efi/r-efi",
            },
            *extra_rust,
        ]
        meta_path = self.write("metadata.json", json.dumps({"packages": rust_packages}))
        ui_json = self.write(
            "ui-licenses.json",
            json.dumps({"MIT": [{"name": "react", "versions": ["19.3.1"]}]}),
        )
        return meta_path, ui_json

    def test_spdx_notice_requirements_distinguish_or_from_and(self):
        self.assertEqual(self.mod.required_notice_licences("MIT OR Apache-2.0"), set())
        self.assertEqual(self.mod.required_notice_licences("Apache-2.0 OR GPL-2.0-only"), set())
        self.assertEqual(self.mod.required_notice_licences("MIT OR Apache-2.0 OR LGPL-2.1-or-later"), set())
        self.assertEqual(self.mod.required_notice_licences("(MIT OR Apache-2.0) AND IJG"), {"IJG"})
        self.assertEqual(self.mod.required_notice_licences("MPL-2.0"), {"MPL-2.0"})
        self.assertEqual(self.mod.required_notice_licences("(MIT OR Apache-2.0) AND Unicode-3.0"), {"Unicode-3.0"})

    def test_fails_when_required_dependency_licence_is_missing_from_notices(self):
        meta_path, ui_json = self.seed_minimal_repo(
            extra_rust=[
                {
                    "name": "jpeg-encoder",
                    "version": "0.7.1",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "license": "(MIT OR Apache-2.0) AND IJG",
                    "repository": "https://github.com/vstroebel/jpeg-encoder",
                }
            ]
        )
        # Generate report from metadata + UI JSON.
        rc = self.mod.main(["--root", str(self.root), "--metadata-json", str(meta_path), "--ui-licenses-json", str(ui_json)])
        self.assertEqual(rc, 0)

        # Consistency check must fail because jpeg-encoder / IJG is missing from THIRD_PARTY_NOTICES.md.
        errors = self.mod.check_notices_consistency(self.root)
        self.assertTrue(any("jpeg-encoder" in e and "IJG" in e for e in errors), errors)

        # Adding the notice makes the check pass.
        notices = self.root / "THIRD_PARTY_NOTICES.md"
        notices.write_text(
            notices.read_text(encoding="utf-8") + "\n## jpeg-encoder (IJG)\nBased in part on IJG.\n",
            encoding="utf-8",
        )
        self.assertEqual(self.mod.check_notices_consistency(self.root), [])

    def test_fails_when_generated_report_drifts_from_lockfiles(self):
        meta_path, ui_json = self.seed_minimal_repo()
        self.assertEqual(
            self.mod.main(["--root", str(self.root), "--metadata-json", str(meta_path), "--ui-licenses-json", str(ui_json)]),
            0,
        )
        self.assertEqual(
            self.mod.main(
                ["--root", str(self.root), "--check", "--metadata-json", str(meta_path), "--ui-licenses-json", str(ui_json)]
            ),
            0,
        )

        # Bump a UI version in pnpm-lock.yaml without regenerating the report.
        self.write(
            "pnpm-lock.yaml",
            """
            lockfileVersion: '9.0'
            importers:
              apps/desktop/ui:
                dependencies:
                  react:
                    specifier: ^19.0.0
                    version: 19.4.0
            packages:
              react@19.4.0:
                resolution: {integrity: sha512-dummy}
            snapshots:
              react@19.4.0: {}
            """,
        )
        ui_json_v2 = self.write(
            "ui-licenses-v2.json",
            json.dumps({"MIT": [{"name": "react", "versions": ["19.4.0"]}]}),
        )
        rc = self.mod.main(
            ["--root", str(self.root), "--check", "--metadata-json", str(meta_path), "--ui-licenses-json", str(ui_json_v2)]
        )
        self.assertEqual(rc, 1)

    def test_advisory_expiry_validation(self):
        self.seed_minimal_repo(
            ignores=[
                '{ id = "RUSTSEC-2026-0001", reason = "waiting on upstream (expires: 2027-01-15)" },',
            ]
        )
        self.assertEqual(self.mod.check_advisory_expiries(self.root / "deny.toml", today=date(2026, 11, 25)), [])
        expired = self.mod.check_advisory_expiries(self.root / "deny.toml", today=date(2027, 2, 1))
        self.assertTrue(any("expired on 2027-01-15" in e for e in expired), expired)

        self.seed_minimal_repo(ignores=['{ id = "RUSTSEC-2026-0002", reason = "no expiry here" },'])
        undated = self.mod.check_advisory_expiries(self.root / "deny.toml", today=date(2026, 11, 25))
        self.assertTrue(any("missing '(expires: YYYY-MM-DD)'" in e for e in undated), undated)

    def test_repo_notices_and_expiry_pass(self):
        self.assertEqual(self.mod.check_notices_consistency(ROOT), [])
        self.assertEqual(self.mod.check_advisory_expiries(ROOT / "deny.toml", today=date(2026, 11, 25)), [])


if __name__ == "__main__":
    unittest.main()
