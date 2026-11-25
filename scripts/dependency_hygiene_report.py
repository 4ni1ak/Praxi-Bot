#!/usr/bin/env python3
"""Render the weekly dependency hygiene report and select the single tracking issue.

Used by `.github/workflows/dependency-hygiene.yml`. Works offline over captured tool logs.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import sys

TRACKING_MARKER = "<!-- dependency-hygiene-tracking -->"
TRACKING_TITLE = "Dependency hygiene: weekly audit findings"
MAX_SECTION_LINES = 120


@dataclass(frozen=True)
class CheckResult:
    name: str
    command: str
    exit_code: int
    output: str

    @property
    def passed(self) -> bool:
        return self.exit_code == 0


def trim_output(text: str, max_lines: int = MAX_SECTION_LINES) -> str:
    lines = text.strip().splitlines()
    if not lines:
        return "(no output)"
    if len(lines) <= max_lines:
        return "\n".join(lines)
    kept = lines[:max_lines]
    omitted = len(lines) - max_lines
    return "\n".join(kept) + f"\n... ({omitted} more line(s); see the workflow run log)"


def render_report(
    checks: list[CheckResult],
    *,
    run_url: str,
    timestamp_utc: str,
) -> str:
    failing = [c for c in checks if not c.passed]
    lines = [
        TRACKING_MARKER,
        f"## Weekly dependency hygiene report ({timestamp_utc})",
        "",
        f"Workflow run: {run_url}" if run_url else "Workflow run: (local)",
        "",
        "| Check | Command | Status |",
        "| --- | --- | --- |",
    ]
    for check in checks:
        status = "PASS" if check.passed else f"FAIL (exit {check.exit_code})"
        lines.append(f"| {check.name} | `{check.command}` | {status} |")
    lines.append("")

    if not failing:
        lines.append("All dependency hygiene checks passed with no findings.")
        lines.append("")
        return "\n".join(lines)

    lines.append(f"{len(failing)} check(s) reported findings or policy drift:")
    lines.append("")
    for check in failing:
        lines.append(f"### {check.name}")
        lines.append("")
        lines.append("```text")
        lines.append(trim_output(check.output))
        lines.append("```")
        lines.append("")

    lines.append(
        "See `docs/contributing/dependencies.md` for how to upgrade dependencies, "
        "refresh notices, or record a time-bounded advisory exception."
    )
    lines.append("")
    return "\n".join(lines)


def find_tracking_issue(open_issues: list[dict]) -> dict | None:
    """Return the single open tracking issue to update, preferring the lowest issue number."""
    matches = []
    for issue in open_issues:
        body = issue.get("body") or ""
        title = issue.get("title") or ""
        if TRACKING_MARKER in body or title == TRACKING_TITLE:
            matches.append(issue)
    if not matches:
        return None
    return min(matches, key=lambda item: int(item["number"]))


def _parse_check_arg(spec: str) -> CheckResult:
    # Format: name::command::exit_code::path
    parts = spec.split("::", 3)
    if len(parts) != 4:
        raise SystemExit(f"invalid --check specification: {spec!r}")
    name, command, code_str, log_path = parts
    output = Path(log_path).read_text(encoding="utf-8", errors="replace") if Path(log_path).exists() else ""
    return CheckResult(name=name, command=command, exit_code=int(code_str), output=output)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check",
        action="append",
        default=[],
        help="Check result spec: name::command::exit_code::log_path",
    )
    parser.add_argument("--run-url", default="", help="GitHub Actions run URL")
    parser.add_argument("--timestamp", default="", help="UTC timestamp label")
    parser.add_argument("--out-markdown", required=True, help="Output Markdown path")
    parser.add_argument("--out-status", required=True, help="Output JSON status path")
    args = parser.parse_args(argv)

    checks = [_parse_check_arg(item) for item in args.check]
    report = render_report(checks, run_url=args.run_url, timestamp_utc=args.timestamp)
    has_findings = any(not check.passed for check in checks)

    md_path = Path(args.out_markdown)
    md_path.parent.mkdir(parents=True, exist_ok=True)
    md_path.write_text(report, encoding="utf-8")

    status_path = Path(args.out_status)
    status_path.parent.mkdir(parents=True, exist_ok=True)
    status_path.write_text(
        json.dumps(
            {
                "has_findings": has_findings,
                "failing_checks": [c.name for c in checks if not c.passed],
                "title": TRACKING_TITLE,
                "marker": TRACKING_MARKER,
                "run_url": args.run_url,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
