# Release operations

## Atomic deployment

The deployment controller builds a versioned release directory before changing
the active application. An atomic symlink switch publishes the verified release.
The release manifest records the commit identifier, migration compatibility and
asset checksum. A failed health probe causes rollback to the previous manifest;
the controller never reconstructs that release from whichever files happen to
be visible in a working directory. Keep the previous two releases until the new
one has completed an observation window. This rule is about the application
files, not the database's backup retention schedule.

## Publication checklist

| Check | Owner | Evidence |
| --- | --- | --- |
| Dependency lock | Build job | Reproducible install log |
| Asset fingerprint | Build job | Manifest checksum |
| Health probe | Release controller | Successful isolated probe |
| Schema compatibility | Maintainer | Reviewed migration note |

The manifest is written next to the immutable artifact. Static files carry
content hashes so a browser does not combine an older page with a newer script.
The controller serializes release publication per application. Building another
candidate may happen concurrently, but switching the same live symlink must not.

## Rollback boundaries

Rollback selects a previously verified artifact. It does not run arbitrary
commands copied out of a health endpoint. An irreversible database migration
requires its own forward-recovery plan and cannot be undone by a symlink switch.
Before rollback, check whether background workers are running incompatible job
payloads. Suspend new jobs, allow compatible in-flight jobs to finish and resume
only after the selected application version is healthy. Record the release IDs
and the reason for the change in the operations log.

## Small-host scheduling

The always-on host has two CPU workers available for application services.
Builds run elsewhere; publication only verifies and moves already prepared
artifacts. Chunk indexing and backup compression run as bounded background
jobs. If the host is under pressure, delay nonurgent maintenance instead of
launching several CPU-heavy subprocesses at once.
