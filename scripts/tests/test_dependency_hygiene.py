"""Hermetic checks for Dependabot config, dependency-hygiene workflow, and tracking report."""

import importlib.util
import json
from pathlib import Path
import re
import sys
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parents[2]
DEPENDABOT = ROOT / ".github" / "dependabot.yml"
WORKFLOW = ROOT / ".github" / "workflows" / "dependency-hygiene.yml"
REPORT_SCRIPT = ROOT / "scripts" / "dependency_hygiene_report.py"


def load_report_module():
    spec = importlib.util.spec_from_file_location("dependency_hygiene_report", REPORT_SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def jobs(text: str) -> dict[str, str]:
    section = text.split("\njobs:\n", 1)[1]
    parts = re.split(r"(?m)^  ([a-z][a-z0-9-]*):[ \t]*$", section)
    return {parts[i]: parts[i + 1] for i in range(1, len(parts), 2)}


class DependabotAndWorkflowPolicyTests(unittest.TestCase):
    def test_dependabot_covers_cargo_npm_and_actions_with_weekly_groups_limits_and_cooldown(self):
        text = DEPENDABOT.read_text(encoding="utf-8")
        self.assertIn("version: 2", text)
        for eco in ('package-ecosystem: "cargo"', 'package-ecosystem: "npm"', 'package-ecosystem: "github-actions"'):
            self.assertIn(eco, text)
        self.assertEqual(text.count('interval: "weekly"'), 4)
        self.assertIn("open-pull-requests-limit:", text)
        self.assertIn("groups:", text)
        self.assertIn("cooldown:", text)
        self.assertIn("default-days: 7", text)
        # github-actions must not set semver-*-days (Dependabot rejects it).
        gh_section = text.split('package-ecosystem: "github-actions"', 1)[1]
        self.assertNotIn("semver-", gh_section)

    def test_hygiene_workflow_is_least_privilege_secret_free_and_fork_safe(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        self.assertRegex(text, r"(?m)^permissions:\s*\{\s*\}$")
        self.assertNotIn("secrets.", text)
        self.assertNotIn("pull_request_target", text)
        self.assertNotIn("pull_request:", text)
        self.assertIn("schedule:", text)

        uses = re.findall(r"(?m)^\s*(?:-\s*)?uses:\s*(\S+)", text)
        self.assertGreaterEqual(len(uses), 6)
        for action in uses:
            self.assertRegex(action, r"^[^@\s]+@[0-9a-f]{40}$", f"{action} must be pinned to a commit SHA")

        parsed_jobs = jobs(text)
        self.assertEqual(set(parsed_jobs), {"audit", "report-issue"})
        for name, body in parsed_jobs.items():
            self.assertIn("github.repository == 'emreocell/Praxi-Bot'", body, name)

        self.assertIn("contents: read", parsed_jobs["audit"])
        self.assertNotIn("issues: write", parsed_jobs["audit"])
        self.assertIn("issues: write", parsed_jobs["report-issue"])
        self.assertNotIn("actions/checkout", parsed_jobs["report-issue"])

        for tool_cmd in ("cargo deny", "cargo audit", "pnpm audit --prod"):
            self.assertIn(tool_cmd, parsed_jobs["audit"])

    def test_report_renderer_and_single_tracking_issue_selector(self):
        mod = load_report_module()
        with TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            deny_log = root / "deny.log"
            deny_log.write_text("error[vulnerability]: example\n", encoding="utf-8")
            audit_log = root / "audit.log"
            audit_log.write_text("0 vulnerabilities found\n", encoding="utf-8")
            out_md = root / "report.md"
            out_status = root / "status.json"

            rc = mod.main(
                [
                    "--check",
                    f"cargo-deny::cargo deny check::1::{deny_log}",
                    "--check",
                    f"cargo-audit::cargo audit::0::{audit_log}",
                    "--run-url",
                    "https://github.com/emreocell/Praxi-Bot/actions/runs/1",
                    "--timestamp",
                    "2026-11-25T05:17:00Z",
                    "--out-markdown",
                    str(out_md),
                    "--out-status",
                    str(out_status),
                ]
            )
            self.assertEqual(rc, 0)
            status = json.loads(out_status.read_text(encoding="utf-8"))
            self.assertTrue(status["has_findings"])
            self.assertEqual(status["failing_checks"], ["cargo-deny"])
            md = out_md.read_text(encoding="utf-8")
            self.assertIn(mod.TRACKING_MARKER, md)
            self.assertIn("FAIL (exit 1)", md)
            self.assertIn("PASS", md)

            # Tracking issue selection picks the existing issue by marker or title.
            self.assertIsNone(mod.find_tracking_issue([{"number": 10, "title": "Other", "body": "unrelated"}]))
            picked = mod.find_tracking_issue(
                [
                    {"number": 42, "title": "Reused", "body": f"header\n{mod.TRACKING_MARKER}\nold"},
                    {"number": 19, "title": mod.TRACKING_TITLE, "body": "legacy"},
                ]
            )
            self.assertEqual(picked["number"], 19)


if __name__ == "__main__":
    unittest.main()
