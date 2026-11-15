# Backup and restore rehearsal

## Retention

Keep seven daily backup snapshots and four weekly snapshots. The retention job
only removes snapshots listed in its own catalog; filenames discovered beside a
backup are not automatically disposable. Snapshot manifests record application
version, database schema, source size and checksum. Store verification results
alongside the manifest so a recent but unreadable snapshot cannot replace the
last known-good restore point.

## Consistent capture

Use SQLite's backup API for a live WAL database. Copying the main database file
while a writer is active can omit committed state in its journal. Each captured
database is opened in a staging directory and checked before the archive is
published. Persistent note files are copied under their own stable paths. Browser
profiles, reusable session credentials and raw API keys are excluded from this
fixture's portable backup format.

## Restore rehearsal

Once a week, restore the archive into an isolated empty data directory. Verify
the checksum, schema compatibility and expected row counts before opening the
application. The rehearsal never points a background worker at production email
or filesystem sources. Imported sources start disabled and network grants are
cleared; a user reviews local bindings before reactivation. A successful process
exit alone is not evidence that the restore contains usable documents.

## Failed restore

Keep the prior installation intact until staging validation completes. A failed
restore removes only its staging directory and leaves the active application
unchanged. A release rollback can select an older application artifact, but a
database restore selects a snapshot and may discard later data. Present those
consequences separately in the review. Record the tested archive checksum and
rehearsal time, rather than logging the archive's private contents.
