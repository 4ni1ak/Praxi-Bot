# Dependency hygiene policy

Praxi-Bot keeps third-party dependencies small, permissively licensed, and reviewed on a predictable schedule.
Automated checks live in [`.github/dependabot.yml`](../../.github/dependabot.yml),
[`.github/workflows/dependency-hygiene.yml`](../../.github/workflows/dependency-hygiene.yml),
[`deny.toml`](../../deny.toml), and [`scripts/license_report.py`](../../scripts/license_report.py).

## Adding or updating dependencies

- **Prefer existing workspace dependencies.** Reuse crates already listed in `[workspace.dependencies]` in [`Cargo.toml`](../../Cargo.toml) or packages already in [`pnpm-lock.yaml`](../../pnpm-lock.yaml) before introducing a new dependency.
- **Discuss new dependencies first.** As noted in [`CONTRIBUTING.md`](../../CONTRIBUTING.md#what-not-to-send), open an issue before adding a new dependency so its security, binary-size, and maintenance impact can be reviewed.
- **Check licences and bans locally before pushing:**
  ```sh
  cargo deny check licenses bans advisories sources
  python scripts/license_report.py
  python scripts/license_report.py --check --check-advisory-expiry
  ```
- **Regenerate the licence report when dependencies change.** Any PR that updates `Cargo.lock` or `pnpm-lock.yaml` must keep [the generated third-party licence report](../third-party-licenses.md) up to date (`python scripts/license_report.py`).

## Dependabot schedule, grouping, and cooldown

[`.github/dependabot.yml`](../../.github/dependabot.yml) covers:

- **Cargo** (`/`): weekly on Mondays (06:00 UTC), max 5 open PRs, grouped into `tauri`, `tokio-and-http`, and `rust-minor-patch`.
- **npm / pnpm** (`/` and `/docs-site`): weekly on Mondays (06:00 UTC), max 5 open PRs at root (`ui-runtime` and `ui-tooling`) and max 3 in `/docs-site` (`docs-site`).
- **GitHub Actions** (`/`): weekly on Mondays (06:00 UTC), max 5 open PRs, grouped into `github-actions`.

### Cooldown for brand-new releases

To reduce exposure to newly published compromised releases, typosquatting, or immediately yanked point releases, Dependabot waits before opening **version-update** PRs:

- **Cargo and npm / pnpm:** 3 days for patch updates, 7 days for minor updates (default), and 14 days for major updates.
- **GitHub Actions:** 7 days (`default-days: 7`).
- **Security updates bypass cooldown.** GitHub Dependabot automatically opens security-update PRs immediately when a known vulnerability has a patched release.

## Weekly automated audit and tracking issue

[`.github/workflows/dependency-hygiene.yml`](../../.github/workflows/dependency-hygiene.yml) runs every Monday at 05:17 UTC (and on `workflow_dispatch`) only on `emreocell/Praxi-Bot`, never on forks, with no secrets and least-privilege split permissions (`contents: read` for the audit job, `issues: write` for the reporting job).

It runs:

1. `cargo deny --all-features check advisories licenses bans sources`
2. `cargo audit`
3. `pnpm audit --prod`
4. `python3 scripts/license_report.py --check --check-advisory-expiry`

All findings are collected into a single report and posted to **one** open tracking issue (`Dependency hygiene: weekly audit findings`, identified by `<!-- dependency-hygiene-tracking -->`), updating it in place each week or closing it when all checks pass.

## Accepting an advisory exception with an expiry date

Never silence a vulnerability just to make CI green. Only record an exception when **no non-breaking patched upgrade exists** or the affected code path is verified unreachable in Praxi-Bot (for example, an unmaintained-crate notice with no security advisory).

1. **Check reachability and upgrade paths first.** Try `cargo update -p <crate>` (or `pnpm up <pkg>`), and inspect `cargo tree -i <crate>` to see if an unused optional feature or old direct pin is holding back a patched version.
2. **Choose a short expiry date.**
   - Active vulnerability (mitigated / unreachable): **at most 30–90 days**.
   - Unmaintained crate with no known vulnerability (awaiting upstream or planned migration): **at most 180 days**.
3. **Record the exception in [`deny.toml`](../../deny.toml)** under `[advisories].ignore` with:
   - An inline comment explaining why no upgrade is possible and what removes the exception.
   - An explicit `(expires: YYYY-MM-DD)` tag inside `reason`, for example:
     ```toml
     { id = "RUSTSEC-2026-0000", reason = "unreachable feature foo; upgrade blocked on bar (expires: 2027-02-01)" }
     ```
4. **Verify before pushing:**
   ```sh
   python scripts/license_report.py --check-advisory-expiry
   ```
   CI (`license-check`) and the weekly hygiene workflow fail if any `[advisories].ignore` entry lacks `(expires: YYYY-MM-DD)` or if its expiry date has passed. When an exception expires, either land the upgrade/migration and delete the entry, or re-evaluate reachability and renew the expiry date in a reviewed PR.
5. **For npm production advisories** with no upstream fix, document the justification and expiry date in the weekly tracking issue (and PR description) before using a temporary `pnpm audit --ignore` entry.

## `THIRD_PARTY_NOTICES.md` consistency check

Standard low-ceremony permissive licences (`MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `BSD-1-Clause`, `ISC`, `Zlib`, `Unlicense`, `CC0-1.0`, `0BSD`, `MIT-0`, `BSL-1.0`) are inventoried in [the generated licence report](../third-party-licenses.md).

Dependencies whose licence requires explicit attribution, source-offer text, or special notice (such as `IJG`, `MPL-2.0`, `OFL-1.1`, `CDLA-Permissive-2.0`, `Unicode-3.0`, `CC-BY-*`, copyleft licences, and any crate under `[[licenses.exceptions]]` in `deny.toml`), as well as bundled UI notice files under `apps/desktop/ui/public/licenses/`, **must** be documented in [`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md). CI fails (`scripts/license_report.py --check-notices`, `--check`, and `--check-ui`) when a required dependency or notice file is missing.
