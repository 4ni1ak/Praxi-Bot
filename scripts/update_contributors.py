#!/usr/bin/env python3
"""Rewrite the contributor avatars in README.md: every person who authored a commit,
and no AI tools or bots.

    python3 scripts/update_contributors.py            # rewrite README.md
    python3 scripts/update_contributors.py --check    # exit 1 if README.md is out of date
    python3 scripts/update_contributors.py --self-test

The people come from the GitHub commits API (the account that *authored* each commit on the
default branch), so someone whose commits GitHub's contributors graph misses still appears.
`Co-authored-by` trailers are never read: AI tools (Claude, Codex, Cursor, ...) are added
that way, and are not contributors. Accounts that are bots, or listed in NOT_PEOPLE, are
dropped. Set GITHUB_TOKEN to avoid the anonymous rate limit.
"""
from __future__ import annotations

import json
import os
import re
import sys
import unittest
import urllib.request

REPO = "emreocell/Praxi-Bot"
README = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "README.md")
START = "<!-- contributors:start -->"
END = "<!-- contributors:end -->"
AVATAR = 48

# Accounts that are tools, not people. Lower case. A login ending in "[bot]" or with a
# GitHub account type other than "User" is dropped as well.
NOT_PEOPLE = {
    "claude", "anthropic", "codex", "openai", "cursor", "cursoragent", "copilot",
    "github-copilot", "devin-ai-integration", "gemini", "gemini-code-assist", "agy",
    "dependabot", "renovate", "coderabbitai", "github-actions", "web-flow",
}


def is_person(login: str, kind: str) -> bool:
    name = login.lower()
    return kind == "User" and not name.endswith("[bot]") and name not in NOT_PEOPLE


def people(commits: list[dict]) -> list[str]:
    """Logins of the people who authored commits, most commits first, then by name."""
    counts: dict[str, int] = {}
    for commit in commits:
        author = commit.get("author") or {}
        login = author.get("login")
        if login and is_person(login, author.get("type", "User")):
            counts[login] = counts.get(login, 0) + 1
    return sorted(counts, key=lambda login: (-counts[login], login.lower()))


def render(logins: list[str]) -> str:
    cells = [
        f'<a href="https://github.com/{login}"><img src="https://github.com/{login}.png?size={AVATAR * 2}" '
        f'width="{AVATAR}" height="{AVATAR}" alt="{login}" title="{login}"></a>'
        for login in logins
    ]
    return "\n".join(cells)


def splice(readme: str, block: str) -> str:
    pattern = re.compile(re.escape(START) + r".*?" + re.escape(END), re.S)
    if not pattern.search(readme):
        raise SystemExit(f"README.md has no {START} ... {END} block")
    return pattern.sub(lambda _m: f"{START}\n{block}\n{END}", readme, count=1)


def fetch_commits() -> list[dict]:
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "praxi-contributors"}
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    commits: list[dict] = []
    for page in range(1, 101):
        url = f"https://api.github.com/repos/{REPO}/commits?per_page=100&page={page}"
        with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=30) as r:
            batch = json.load(r)
        if not batch:
            break
        commits += batch
    return commits


def main(argv: list[str]) -> int:
    with open(README, encoding="utf-8", newline="") as f:
        current = f.read()
    updated = splice(current, render(people(fetch_commits())))
    if "--check" in argv:
        if updated != current:
            print("README.md contributors are out of date: run scripts/update_contributors.py")
            return 1
        return 0
    if updated != current:
        with open(README, "w", encoding="utf-8", newline="") as f:
            f.write(updated)
        print("README.md contributors updated")
    return 0


class Tests(unittest.TestCase):
    COMMITS = [
        {"author": {"login": "alice", "type": "User"}},
        {"author": {"login": "bob", "type": "User"}},
        {"author": {"login": "bob", "type": "User"}},
        {"author": {"login": "claude", "type": "User"}},
        {"author": {"login": "CursorAgent", "type": "User"}},
        {"author": {"login": "dependabot[bot]", "type": "Bot"}},
        {"author": {"login": "robot", "type": "Bot"}},
        {"author": None},
    ]

    def test_only_people_most_active_first(self) -> None:
        self.assertEqual(people(self.COMMITS), ["bob", "alice"])

    def test_render_and_splice_are_stable(self) -> None:
        block = render(["bob"])
        self.assertIn('href="https://github.com/bob"', block)
        text = f"before\n{START}\nold\n{END}\nafter\n"
        once = splice(text, block)
        self.assertEqual(splice(once, block), once)
        self.assertTrue(once.startswith("before\n") and once.endswith("\nafter\n"))
        self.assertNotIn("old", once)


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    sys.exit(main(sys.argv[1:]))
