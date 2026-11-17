# Releasing the Linux server packages

Pushing a `v*` tag runs *Linux Server Packages*: native x86_64 and aarch64 builds, build provenance
(attestation) and an independent verification. What happens next depends on the tag.

## Pre-release tags (`v0.1.0-alpha.1`, `-beta.N`, `-rc.N`: any suffix)
A single maintainer can publish them. The run builds both architectures, attests and verifies them,
and attaches the assets to that tag's GitHub pre-release by itself. No reviewer, no environment.
It still requires this repository (no forks, no pull requests), the tag's own ref or a manual run on
`main`, and it never overwrites an existing release asset. To (re)publish the assets of an existing
pre-release tag: `gh workflow run linux-release.yml --ref main -f tag=v0.1.0-alpha.1 -f dry_run=false`.
Protection is lower than for final tags: nobody else approves the publication and a manual run on
`main` is attested as a build of `main` (the installer accepts that ref for pre-release tags only).

## Final tags (`v1.0.0`, no suffix)
Unchanged and strict. The run fails at "Attest protected release tag" unless the repository has a
tag ruleset for `refs/tags/v*` and the `release` environment (Settings > Environments) has a required
reviewer other than the person who pushes the tag, with "Prevent self-review" on. Publishing is a
manual run on the tag itself with `dry_run` off.

## Which tag the server installs
`bot host add` installs the release tag the desktop app was built from (`PRAXI_RELEASE_TAG`, set by
the Windows installer workflow). `install-linux.sh --version latest` falls back to the newest release
including pre-releases (through the pinned repository's API) while GitHub's "latest" is empty.
