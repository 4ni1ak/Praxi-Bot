#!/usr/bin/env python3
"""Generate docs/third-party-licenses.md and verify THIRD_PARTY_NOTICES.md consistency.

Usage:
  python scripts/license_report.py                         # regenerate docs/third-party-licenses.md
  python scripts/license_report.py --check                 # verify report + notices (+ UI if installed)
  python scripts/license_report.py --check-notices         # pure-Python check of docs report vs notices
  python scripts/license_report.py --check-ui              # verify installed UI prod licences via pnpm
  python scripts/license_report.py --check-advisory-expiry # fail on undated/expired deny.toml ignores
"""
from __future__ import annotations

import argparse
from collections import Counter
from dataclasses import dataclass
from datetime import date, datetime, timezone
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib

# Standard low-ceremony permissive licences covered by the generated inventory alone.
# Any dependency whose effective SPDX expression cannot be satisfied solely by these
# licences requires an explicit notice in THIRD_PARTY_NOTICES.md.
STANDARD_PERMISSIVE = frozenset(
    {
        "MIT",
        "Apache-2.0",
        "Apache-2.0 WITH LLVM-exception",
        "BSD-2-Clause",
        "BSD-3-Clause",
        "BSD-1-Clause",
        "ISC",
        "Zlib",
        "Unlicense",
        "CC0-1.0",
        "0BSD",
        "MIT-0",
        "BSL-1.0",
    }
)

# Alternate spellings accepted in THIRD_PARTY_NOTICES.md for an SPDX identifier.
LICENCE_ALIASES: dict[str, tuple[str, ...]] = {
    "OFL-1.1": ("OFL-1.1", "SIL OFL 1.1", "OFL 1.1"),
}

EXPIRY_RE = re.compile(r"\bexpires:\s*(\d{4}-\d{2}-\d{2})\b", re.IGNORECASE)
NOTICE_FILE_REF_RE = re.compile(r"apps/desktop/ui/public/licenses/([A-Za-z0-9._-]+\.txt)")

REPORT_REL = Path("docs/third-party-licenses.md")
NOTICES_REL = Path("THIRD_PARTY_NOTICES.md")
DENY_REL = Path("deny.toml")
PNPM_LOCK_REL = Path("pnpm-lock.yaml")
UI_LICENSES_DIR_REL = Path("apps/desktop/ui/public/licenses")


@dataclass(frozen=True, order=True)
class Dependency:
    name: str
    version: str
    ecosystem: str
    license: str
    repository: str = ""


# ---------------------------------------------------------------------------
# Minimal SPDX boolean expression parser (OR, AND, /, parentheses, WITH)
# ---------------------------------------------------------------------------

_SPDX_TOKEN_RE = re.compile(
    r"\s*(\(|\)|/|WITH\b|AND\b|OR\b|[A-Za-z0-9.+_-]+)",
    re.IGNORECASE,
)


def _tokenize_spdx(expr: str) -> list[str]:
    tokens: list[str] = []
    pos = 0
    for match in _SPDX_TOKEN_RE.finditer(expr):
        if match.start() != pos and expr[pos : match.start()].strip():
            raise ValueError(f"unexpected character in SPDX expression: {expr!r}")
        pos = match.end()
        tok = match.group(1)
        upper = tok.upper()
        if upper in ("OR", "AND", "WITH"):
            tokens.append(upper)
        elif tok == "/":
            tokens.append("OR")
        else:
            tokens.append(tok)
    if expr[pos:].strip():
        raise ValueError(f"trailing text in SPDX expression: {expr!r}")
    return tokens


class _SpdxParser:
    def __init__(self, tokens: list[str]) -> None:
        self.tokens = tokens
        self.pos = 0

    def peek(self) -> str | None:
        return self.tokens[self.pos] if self.pos < len(self.tokens) else None

    def consume(self) -> str:
        tok = self.tokens[self.pos]
        self.pos += 1
        return tok

    def parse(self):
        if not self.tokens:
            return ("LEAF", "UNLICENSED")
        node = self.parse_or()
        if self.pos != len(self.tokens):
            raise ValueError(f"unexpected token {self.peek()!r}")
        return node

    def parse_or(self):
        left = self.parse_and()
        while self.peek() == "OR":
            self.consume()
            right = self.parse_and()
            left = ("OR", left, right)
        return left

    def parse_and(self):
        left = self.parse_atom()
        while self.peek() == "AND":
            self.consume()
            right = self.parse_atom()
            left = ("AND", left, right)
        return left

    def parse_atom(self):
        tok = self.peek()
        if tok == "(":
            self.consume()
            inner = self.parse_or()
            if self.peek() != ")":
                raise ValueError("unclosed '(' in SPDX expression")
            self.consume()
            return inner
        if tok in (None, ")", "OR", "AND", "WITH"):
            raise ValueError(f"unexpected token {tok!r} in SPDX expression")
        base = self.consume()
        if self.peek() == "WITH":
            self.consume()
            exc = self.consume()
            return ("LEAF", f"{base} WITH {exc}")
        return ("LEAF", base)


def parse_spdx(expr: str):
    return _SpdxParser(_tokenize_spdx(expr.strip())).parse()


def spdx_is_allowed(expr: str, allowed: frozenset[str]) -> bool:
    """Return True if `expr` is satisfied by `allowed` licences."""

    def _eval(node) -> bool:
        kind = node[0]
        if kind == "LEAF":
            return node[1] in allowed
        if kind == "OR":
            return _eval(node[1]) or _eval(node[2])
        if kind == "AND":
            return _eval(node[1]) and _eval(node[2])
        raise AssertionError(f"unknown node {kind}")

    return _eval(parse_spdx(expr))


def required_notice_licences(expr: str) -> set[str]:
    """Return the set of licence identifiers in `expr` that require explicit notices.

    Returns an empty set when the expression can be satisfied purely by
    STANDARD_PERMISSIVE licences (for example `Apache-2.0 OR GPL-2.0-only`).
    """

    def _eval(node) -> set[str]:
        kind = node[0]
        if kind == "LEAF":
            lic = node[1]
            return set() if lic in STANDARD_PERMISSIVE else {lic}
        if kind == "OR":
            left = _eval(node[1])
            right = _eval(node[2])
            if not left or not right:
                return set()
            return left | right
        if kind == "AND":
            return _eval(node[1]) | _eval(node[2])
        raise AssertionError(f"unknown node {kind}")

    return _eval(parse_spdx(expr))


# ---------------------------------------------------------------------------
# Loaders for deny.toml, Cargo metadata, pnpm-lock.yaml, and UI licences
# ---------------------------------------------------------------------------


def load_deny_config(deny_path: Path) -> tuple[frozenset[str], dict[str, set[str]], list[dict]]:
    data = tomllib.loads(deny_path.read_text(encoding="utf-8"))
    lic_cfg = data.get("licenses", {})
    allowed = frozenset(lic_cfg.get("allow", []))
    exceptions: dict[str, set[str]] = {}
    for item in lic_cfg.get("exceptions", []):
        crate = item.get("crate")
        if crate:
            exceptions.setdefault(crate, set()).update(item.get("allow", []))
    ignores = list(data.get("advisories", {}).get("ignore", []))
    return allowed, exceptions, ignores


def load_rust_dependencies(root: Path, metadata_json: Path | None = None) -> list[Dependency]:
    if metadata_json is not None:
        raw = metadata_json.read_text(encoding="utf-8")
    else:
        cmd = ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"]
        proc = subprocess.run(cmd, cwd=root, capture_output=True, text=True, encoding="utf-8")
        if proc.returncode != 0:
            fallback = ["cargo", "metadata", "--locked", "--format-version", "1"]
            proc = subprocess.run(
                fallback, cwd=root, capture_output=True, text=True, encoding="utf-8", check=True
            )
        raw = proc.stdout

    meta = json.loads(raw)
    seen: dict[tuple[str, str], Dependency] = {}
    for pkg in meta.get("packages", []):
        if pkg.get("source") is None:
            continue
        name = pkg["name"]
        version = pkg["version"]
        lic = pkg.get("license") or "UNLICENSED"
        repo = (pkg.get("repository") or "").strip()
        seen[(name, version)] = Dependency(
            name=name,
            version=version,
            ecosystem="cargo",
            license=lic,
            repository=repo,
        )
    return sorted(seen.values(), key=lambda d: (d.name.lower(), d.name, d.version))


def _unquote_yaml_key(text: str) -> str:
    text = text.strip()
    if len(text) >= 2 and text[0] == "'" and text[-1] == "'":
        return text[1:-1].replace("''", "'")
    if len(text) >= 2 and text[0] == '"' and text[-1] == '"':
        return text[1:-1]
    return text


def _split_pkg_key(key: str) -> tuple[str, str]:
    """Split a pnpm snapshot key like '@babel/runtime@7.29.13(foo@1.0)' -> ('@babel/runtime', '7.29.13')."""
    base = key.split("(", 1)[0]
    idx = base.rfind("@")
    if idx <= 0:
        raise ValueError(f"unexpected pnpm snapshot key: {key!r}")
    return base[:idx], base[idx + 1 :]


def parse_pnpm_ui_prod_packages(lock_path: Path) -> dict[tuple[str, str], str]:
    """Return {(name, version): snapshot_key} for apps/desktop/ui + packages/ui prod dependencies."""
    lines = lock_path.read_text(encoding="utf-8").splitlines()
    importers: dict[str, dict[str, str]] = {}
    pkg_peers: dict[str, set[str]] = {}
    snapshots: dict[str, dict[str, str]] = {}

    section = ""
    current_importer = ""
    in_deps = False
    current_pkg = ""
    current_pkg_entry = ""
    in_peers = False
    current_snap = ""
    snap_sub = ""

    for line in lines:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        indent = len(line) - len(line.lstrip(" "))
        stripped = line.strip()
        if indent == 0 and stripped.endswith(":"):
            section = stripped[:-1]
            current_importer = ""
            in_deps = False
            current_pkg = ""
            current_pkg_entry = ""
            in_peers = False
            current_snap = ""
            snap_sub = ""
            continue

        if section == "importers":
            if indent == 2 and stripped.endswith(":"):
                current_importer = _unquote_yaml_key(stripped[:-1])
                importers.setdefault(current_importer, {})
                in_deps = False
                current_pkg = ""
            elif indent == 4 and stripped.endswith(":"):
                in_deps = stripped[:-1] == "dependencies"
                current_pkg = ""
            elif in_deps and indent == 6 and stripped.endswith(":"):
                current_pkg = _unquote_yaml_key(stripped[:-1])
            elif in_deps and indent == 8 and current_pkg and stripped.startswith("version:"):
                ver = _unquote_yaml_key(stripped.split(":", 1)[1])
                importers[current_importer][current_pkg] = ver

        elif section == "packages":
            if indent == 2 and stripped.endswith(":"):
                current_pkg_entry = _unquote_yaml_key(stripped[:-1])
                pkg_peers.setdefault(current_pkg_entry, set())
                in_peers = False
            elif current_pkg_entry and indent == 4 and stripped.endswith(":"):
                in_peers = stripped[:-1] in ("peerDependencies", "peerDependenciesMeta")
            elif current_pkg_entry and in_peers and indent == 6 and ":" in stripped:
                peer_name = _unquote_yaml_key(stripped.partition(":")[0])
                pkg_peers[current_pkg_entry].add(peer_name)

        elif section == "snapshots":
            if indent == 2 and ":" in stripped:
                key_part, _, rest = stripped.partition(":")
                current_snap = _unquote_yaml_key(key_part)
                snapshots.setdefault(current_snap, {})
                snap_sub = ""
                if rest.strip() == "{}":
                    current_snap = ""
            elif current_snap and indent == 4 and stripped.endswith(":"):
                sub = stripped[:-1]
                snap_sub = sub if sub in ("dependencies", "optionalDependencies") else ""
            elif current_snap and snap_sub and indent == 6 and ":" in stripped:
                dep_name, _, dep_ver = stripped.partition(":")
                snapshots[current_snap][_unquote_yaml_key(dep_name)] = _unquote_yaml_key(dep_ver)

    resolved: dict[tuple[str, str], str] = {}
    queue: list[tuple[str, str]] = []
    visited_snaps: set[str] = set()

    def enqueue_importer(imp_name: str) -> None:
        for pkg_name, ver in importers.get(imp_name, {}).items():
            if ver.startswith("link:"):
                # Follow workspace package links (e.g. link:../../../packages/ui).
                if pkg_name == "ui" and "packages/ui" in importers:
                    enqueue_importer("packages/ui")
                continue
            queue.append((pkg_name, ver))

    enqueue_importer("apps/desktop/ui")

    while queue:
        pkg_name, ver = queue.pop(0)
        snap_key = f"{pkg_name}@{ver}"
        bare_ver = ver.split("(", 1)[0]
        base_key = f"{pkg_name}@{bare_ver}"
        resolved.setdefault((pkg_name, bare_ver), snap_key)
        if snap_key in visited_snaps:
            continue
        visited_snaps.add(snap_key)
        peers = pkg_peers.get(base_key, set())
        for dep_name, dep_ver in snapshots.get(snap_key, {}).items():
            if dep_name in peers:
                continue
            queue.append((dep_name, dep_ver))

    return resolved


def load_pnpm_installed_licences(root: Path, ui_json_path: Path | None = None) -> dict[tuple[str, str], str] | None:
    if ui_json_path is not None:
        data = json.loads(ui_json_path.read_text(encoding="utf-8"))
    else:
        if not (root / "apps/desktop/ui/node_modules").exists():
            return None
        pnpm_cmd = "pnpm.cmd" if sys.platform == "win32" else "pnpm"
        proc = subprocess.run(
            [pnpm_cmd, "--filter", "desktop-ui", "--filter", "ui", "licenses", "list", "--prod", "--json"],
            cwd=root,
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=True,
        )
        data = json.loads(proc.stdout)

    licences: dict[tuple[str, str], str] = {}
    for lic_expr, entries in data.items():
        for entry in entries:
            name = entry.get("name", "")
            for version in entry.get("versions", []):
                licences[(name, version)] = lic_expr
    return licences


def query_pnpm_view_licence(name: str, version: str) -> str:
    pnpm_cmd = "pnpm.cmd" if sys.platform == "win32" else "pnpm"
    out = subprocess.check_output(
        [pnpm_cmd, "view", f"{name}@{version}", "license", "--json"],
        text=True,
        encoding="utf-8",
    ).strip()
    if not out:
        return "UNKNOWN"
    val = json.loads(out)
    return str(val) if val else "UNKNOWN"


def parse_report_tables(report_path: Path) -> tuple[list[Dependency], list[Dependency]]:
    """Parse Rust and Desktop UI production dependency tables from docs/third-party-licenses.md."""
    if not report_path.exists():
        return [], []
    lines = report_path.read_text(encoding="utf-8").splitlines()
    section = ""
    rust_deps: list[Dependency] = []
    ui_deps: list[Dependency] = []

    for line in lines:
        if line.startswith("## "):
            section = line[3:].strip()
            continue
        if not line.startswith("|") or line.startswith("| ---") or line.startswith("| Package "):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if section == "Rust dependencies" and len(cells) >= 4:
            name = cells[0].strip("`")
            version = cells[1].strip("`")
            lic = cells[2].strip("`")
            repo = "" if cells[3] in ("-", "") else cells[3]
            if repo.startswith("[") and "](" in repo and repo.endswith(")"):
                repo = repo.split("](", 1)[1][:-1]
            rust_deps.append(Dependency(name=name, version=version, ecosystem="cargo", license=lic, repository=repo))
        elif section == "Desktop UI production dependencies" and len(cells) >= 3:
            name = cells[0].strip("`")
            version = cells[1].strip("`")
            lic = cells[2].strip("`")
            ui_deps.append(Dependency(name=name, version=version, ecosystem="npm", license=lic))

    return rust_deps, ui_deps


def load_ui_dependencies(
    root: Path,
    report_path: Path,
    *,
    ui_json_path: Path | None = None,
    allow_network_view: bool = False,
) -> list[Dependency]:
    resolved = parse_pnpm_ui_prod_packages(root / PNPM_LOCK_REL)
    installed = load_pnpm_installed_licences(root, ui_json_path=ui_json_path)
    _, existing_ui = parse_report_tables(report_path)
    existing_map = {(d.name, d.version): d.license for d in existing_ui}

    deps: list[Dependency] = []
    missing_licences: list[str] = []
    for name, version in sorted(resolved.keys(), key=lambda x: (x[0].lower(), x[0], x[1])):
        lic = None
        if installed is not None and (name, version) in installed:
            lic = installed[(name, version)]
        elif (name, version) in existing_map:
            lic = existing_map[(name, version)]
        elif allow_network_view:
            lic = query_pnpm_view_licence(name, version)
        if not lic or lic.lower() == "unknown":
            missing_licences.append(f"{name}@{version}")
            lic = "UNKNOWN"
        deps.append(Dependency(name=name, version=version, ecosystem="npm", license=lic))

    if missing_licences and not allow_network_view:
        raise RuntimeError(
            "missing licence metadata for Desktop UI prod packages "
            "(run `pnpm install --frozen-lockfile` or pass `--fetch-ui-licences`): "
            + ", ".join(missing_licences)
        )
    return deps


def render_report(
    rust_deps: list[Dependency],
    ui_deps: list[Dependency],
    exceptions: dict[str, set[str]],
) -> str:
    all_deps = rust_deps + ui_deps
    counts = Counter(d.license for d in all_deps)

    notice_rows: list[tuple[str, str, str, str, str]] = []
    for dep in sorted(all_deps, key=lambda d: (d.ecosystem, d.name.lower(), d.version)):
        req = required_notice_licences(dep.license)
        if dep.name in exceptions:
            req = req | exceptions[dep.name]
        if req:
            notice_rows.append(
                (dep.name, dep.version, dep.ecosystem, ", ".join(sorted(req)), dep.repository or "-")
            )
    # Include deny.toml exception crates even when gated behind optional features not in default metadata.
    present_names = {d.name for d in all_deps}
    for crate, exc_lics in sorted(exceptions.items()):
        if crate not in present_names:
            notice_rows.append((crate, "(optional feature)", "cargo", ", ".join(sorted(exc_lics)), "-"))

    lines = [
        "# Third-party licence report",
        "",
        "<!-- Generated by scripts/license_report.py. Do not edit by hand. -->",
        "",
        "This inventory lists the resolved third-party dependencies of Praxi-Bot (Rust workspace crates",
        "from `Cargo.lock` and Desktop UI production packages from `pnpm-lock.yaml`).",
        "Regenerate with `python scripts/license_report.py` and verify in CI with",
        "`python scripts/license_report.py --check`. Attribution texts and required notices live in",
        "[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md); contributor policy lives in",
        "[Dependency policy](contributing/dependencies.md).",
        "",
        "## Summary",
        "",
        f"- Rust external crates: **{len(rust_deps)}**",
        f"- Desktop UI production npm packages: **{len(ui_deps)}**",
        "",
        "| Licence expression | Packages |",
        "| --- | ---: |",
    ]
    for lic_expr, count in sorted(counts.items(), key=lambda kv: (-kv[1], kv[0].lower())):
        lines.append(f"| `{lic_expr}` | {count} |")

    lines.extend(
        [
            "",
            "## Notices required",
            "",
            "Packages whose effective licence requires an entry in `THIRD_PARTY_NOTICES.md`:",
            "",
            "| Package | Version | Ecosystem | Required notice | Repository |",
            "| --- | --- | --- | --- | --- |",
        ]
    )
    for name, version, eco, req, repo in notice_rows:
        repo_cell = repo if repo == "-" else f"[{repo}]({repo})"
        lines.append(f"| `{name}` | `{version}` | {eco} | `{req}` | {repo_cell} |")

    lines.extend(
        [
            "",
            "## Rust dependencies",
            "",
            "| Package | Version | Licence | Repository |",
            "| --- | --- | --- | --- |",
        ]
    )
    for dep in rust_deps:
        repo_cell = dep.repository if not dep.repository else f"[{dep.repository}]({dep.repository})"
        if not repo_cell:
            repo_cell = "-"
        lines.append(f"| `{dep.name}` | `{dep.version}` | `{dep.license}` | {repo_cell} |")

    lines.extend(
        [
            "",
            "## Desktop UI production dependencies",
            "",
            "| Package | Version | Licence |",
            "| --- | --- | --- |",
        ]
    )
    for dep in ui_deps:
        lines.append(f"| `{dep.name}` | `{dep.version}` | `{dep.license}` |")

    lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Consistency checks
# ---------------------------------------------------------------------------


def _notice_mentions_package(notices_lower: str, name: str) -> bool:
    escaped = re.escape(name.lower())
    return bool(re.search(rf"(?<![a-z0-9_-]){escaped}(?![a-z0-9_-])", notices_lower))


def _notice_mentions_licence(notices_lower: str, lic: str) -> bool:
    candidates = LICENCE_ALIASES.get(lic, (lic,))
    return any(cand.lower() in notices_lower for cand in candidates)


def check_notices_consistency(
    root: Path,
    *,
    report_path: Path | None = None,
    notices_path: Path | None = None,
    deny_path: Path | None = None,
    ui_licenses_dir: Path | None = None,
) -> list[str]:
    report_path = report_path or (root / REPORT_REL)
    notices_path = notices_path or (root / NOTICES_REL)
    deny_path = deny_path or (root / DENY_REL)
    ui_licenses_dir = ui_licenses_dir or (root / UI_LICENSES_DIR_REL)

    errors: list[str] = []
    if not report_path.exists():
        return [f"missing generated licence report: {report_path}"]
    if not notices_path.exists():
        return [f"missing notices file: {notices_path}"]

    notices_text = notices_path.read_text(encoding="utf-8")
    notices_lower = notices_text.lower()
    allowed, exceptions, _ = load_deny_config(deny_path)
    rust_deps, ui_deps = parse_report_tables(report_path)

    if not rust_deps:
        errors.append(f"{report_path} contains no Rust dependency entries")
    if not ui_deps:
        errors.append(f"{report_path} contains no Desktop UI production dependency entries")

    if "docs/third-party-licenses.md" not in notices_text:
        errors.append("THIRD_PARTY_NOTICES.md must link to docs/third-party-licenses.md")

    # 1. Check all report packages against deny.toml and notice requirements.
    for dep in rust_deps + ui_deps:
        if dep.license.upper() == "UNKNOWN" or dep.license == "UNLICENSED":
            errors.append(f"{dep.ecosystem} package {dep.name}@{dep.version} has unknown/missing licence ({dep.license})")
            continue
        crate_allowed = allowed | exceptions.get(dep.name, set())
        if dep.ecosystem == "cargo":
            try:
                is_ok = spdx_is_allowed(dep.license, crate_allowed)
            except ValueError as err:
                errors.append(f"{dep.name}@{dep.version}: {err}")
                continue
            if not is_ok:
                errors.append(
                    f"Rust crate {dep.name}@{dep.version} licence {dep.license!r} is not allowed by deny.toml"
                )

        try:
            req = required_notice_licences(dep.license)
        except ValueError as err:
            errors.append(f"{dep.name}@{dep.version}: {err}")
            continue

        if dep.name in exceptions:
            req = req | exceptions[dep.name]

        if req:
            if not _notice_mentions_package(notices_lower, dep.name):
                errors.append(
                    f"{dep.ecosystem} dependency '{dep.name}' ({dep.license}) requires a notice in THIRD_PARTY_NOTICES.md"
                )
            for lic in sorted(req):
                if not _notice_mentions_licence(notices_lower, lic):
                    errors.append(
                        f"{dep.ecosystem} dependency '{dep.name}' licence '{lic}' is missing from THIRD_PARTY_NOTICES.md"
                    )

    # 2. Every crate in deny.toml [[licenses.exceptions]] must be documented in THIRD_PARTY_NOTICES.md.
    for crate, exc_lics in sorted(exceptions.items()):
        if not _notice_mentions_package(notices_lower, crate):
            errors.append(f"deny.toml exception crate '{crate}' is missing from THIRD_PARTY_NOTICES.md")
        for lic in sorted(exc_lics):
            if not _notice_mentions_licence(notices_lower, lic):
                errors.append(
                    f"deny.toml exception licence '{lic}' for '{crate}' is missing from THIRD_PARTY_NOTICES.md"
                )

    # 3. Bundled UI licence files under apps/desktop/ui/public/licenses/ must stay in sync with notices.
    referenced_files = set(NOTICE_FILE_REF_RE.findall(notices_text))
    if ui_licenses_dir.is_dir():
        disk_files = {p.name for p in ui_licenses_dir.glob("*.txt")}
        for missing_on_disk in sorted(referenced_files - disk_files):
            errors.append(f"THIRD_PARTY_NOTICES.md references missing licence file: {UI_LICENSES_DIR_REL / missing_on_disk}")
        for unreferenced in sorted(disk_files - referenced_files):
            errors.append(f"UI licence file {UI_LICENSES_DIR_REL / unreferenced} is not mentioned in THIRD_PARTY_NOTICES.md")

    return errors


def check_advisory_expiries(deny_path: Path, *, today: date | None = None) -> list[str]:
    today = today or datetime.now(timezone.utc).date()
    _, _, ignores = load_deny_config(deny_path)
    errors: list[str] = []
    for item in ignores:
        adv_id = item.get("id", "<unknown>")
        reason = item.get("reason", "")
        match = EXPIRY_RE.search(reason)
        if not match:
            errors.append(
                f"deny.toml advisory ignore {adv_id!r} is missing '(expires: YYYY-MM-DD)' in reason"
            )
            continue
        try:
            expiry = date.fromisoformat(match.group(1))
        except ValueError:
            errors.append(f"deny.toml advisory ignore {adv_id!r} has invalid expiry date {match.group(1)!r}")
            continue
        if expiry < today:
            errors.append(f"deny.toml advisory ignore {adv_id!r} expired on {expiry.isoformat()} (today is {today.isoformat()})")
    return errors


def check_ui_installed_licences(
    root: Path,
    *,
    report_path: Path | None = None,
    ui_json_path: Path | None = None,
) -> list[str]:
    report_path = report_path or (root / REPORT_REL)
    installed = load_pnpm_installed_licences(root, ui_json_path=ui_json_path)
    if installed is None:
        return ["apps/desktop/ui/node_modules does not exist; run pnpm install --frozen-lockfile first"]

    resolved = parse_pnpm_ui_prod_packages(root / PNPM_LOCK_REL)
    _, report_ui = parse_report_tables(report_path)
    report_map = {(d.name, d.version): d.license for d in report_ui}
    errors: list[str] = []

    for name, version in sorted(resolved.keys()):
        actual = installed.get((name, version))
        if not actual or actual.lower() == "unknown":
            errors.append(f"UI prod package {name}@{version} has unknown licence in node_modules")
            continue
        reported = report_map.get((name, version))
        if reported is None:
            errors.append(f"UI prod package {name}@{version} ({actual}) is missing from {REPORT_REL}")
        elif reported != actual:
            errors.append(
                f"UI prod package {name}@{version} licence drifted: report has {reported!r}, node_modules has {actual!r}"
            )
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=".", help="repository root")
    parser.add_argument("--check", action="store_true", help="verify report and notices without rewriting")
    parser.add_argument(
        "--check-notices",
        action="store_true",
        help="verify THIRD_PARTY_NOTICES.md against the committed docs report and deny.toml (no cargo required)",
    )
    parser.add_argument(
        "--check-ui",
        action="store_true",
        help="verify installed Desktop UI prod licences against docs report and notices",
    )
    parser.add_argument(
        "--check-advisory-expiry",
        action="store_true",
        help="verify every deny.toml advisory ignore has a valid unexpired date",
    )
    parser.add_argument(
        "--fetch-ui-licences",
        action="store_true",
        help="query npm via `pnpm view` for any UI package missing from node_modules/existing report",
    )
    parser.add_argument("--metadata-json", type=Path, default=None, help="optional cargo metadata JSON path")
    parser.add_argument("--ui-licenses-json", type=Path, default=None, help="optional pnpm licenses JSON path")
    parser.add_argument("--today", default=None, help="override YYYY-MM-DD for expiry checks")
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    report_path = root / REPORT_REL
    deny_path = root / DENY_REL
    notices_path = root / NOTICES_REL

    if args.check_notices and not args.check and not args.check_ui and not args.check_advisory_expiry:
        errors = check_notices_consistency(root, report_path=report_path, notices_path=notices_path, deny_path=deny_path)
        if errors:
            print("THIRD_PARTY_NOTICES consistency check failed:", file=sys.stderr)
            for err in errors:
                print(f"  - {err}", file=sys.stderr)
            return 1
        print("THIRD_PARTY_NOTICES consistency check passed.")
        return 0

    all_errors: list[str] = []

    if args.check_ui:
        all_errors.extend(check_ui_installed_licences(root, report_path=report_path, ui_json_path=args.ui_licenses_json))
        all_errors.extend(check_notices_consistency(root, report_path=report_path, notices_path=notices_path, deny_path=deny_path))

    if args.check:
        _, exceptions, _ = load_deny_config(deny_path)
        rust_deps = load_rust_dependencies(root, metadata_json=args.metadata_json)
        ui_deps = load_ui_dependencies(
            root,
            report_path,
            ui_json_path=args.ui_licenses_json,
            allow_network_view=args.fetch_ui_licences,
        )
        expected = render_report(rust_deps, ui_deps, exceptions)
        if not report_path.exists():
            all_errors.append(f"{REPORT_REL} is missing; run `python scripts/license_report.py`")
        else:
            actual = report_path.read_text(encoding="utf-8")
            if actual != expected:
                all_errors.append(
                    f"{REPORT_REL} is out of date; run `python scripts/license_report.py` and commit the result"
                )
        all_errors.extend(check_notices_consistency(root, report_path=report_path, notices_path=notices_path, deny_path=deny_path))

    if args.check_advisory_expiry:
        override_today = date.fromisoformat(args.today) if args.today else None
        all_errors.extend(check_advisory_expiries(deny_path, today=override_today))

    if args.check or args.check_ui or args.check_advisory_expiry:
        if all_errors:
            print("Dependency licence / hygiene check failed:", file=sys.stderr)
            for err in all_errors:
                print(f"  - {err}", file=sys.stderr)
            return 1
        print("Dependency licence and notices check passed.")
        return 0

    # Default mode: regenerate docs/third-party-licenses.md.
    _, exceptions, _ = load_deny_config(deny_path)
    rust_deps = load_rust_dependencies(root, metadata_json=args.metadata_json)
    ui_deps = load_ui_dependencies(
        root,
        report_path,
        ui_json_path=args.ui_licenses_json,
        allow_network_view=args.fetch_ui_licences,
    )
    content = render_report(rust_deps, ui_deps, exceptions)
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(content, encoding="utf-8")
    print(f"Wrote {report_path} ({len(rust_deps)} Rust crates, {len(ui_deps)} UI prod packages).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
