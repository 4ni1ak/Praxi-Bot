# Releasing the Linux server packages

Pushing a `v*` tag (including pre-release tags such as `v0.1.0-alpha.1`) runs *Linux Server Packages*:
native x86_64 and aarch64 builds, then build provenance (attestation), then an independent verification.
Nothing is published by the tag run itself.

## One-time repository settings (the run fails at "Attest protected release tag" without them)
1. **Tag ruleset** for `refs/tags/v*` (Settings > Rules), so the tag counts as *protected*.
2. **Environment `release`** (Settings > Environments): add a *required reviewer* other than the person
   who pushes the tag and turn on *Prevent self-review*. A repository with one account cannot satisfy
   this; add a second account as reviewer.

## Publishing
Run the workflow by hand (*Run workflow*) on the tag with `dry_run` off. If the release already
exists (the Windows installer is attached first) the Linux assets are added to it; an asset that is
already there is never replaced. A tag with a suffix (`-alpha.1`) is published as a pre-release.

## Which tag the server installs
`bot host add` installs the release tag the desktop app was built from (`PRAXI_RELEASE_TAG`, set by the
Windows installer workflow), not `v<Cargo version>`. `install-linux.sh --version latest` falls back to
the newest release including pre-releases (through the pinned repository's API) while GitHub's "latest"
is empty.
