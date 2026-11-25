# In-app updates and rollback

Settings > System > About can check for a new version, show what is new (in the
user's language when the release has it), install it with one click and go back
to the previous version if the new one does not start.

**Nothing is applied unless its signature verifies** against the public key built into
the app (minisign, checked by `tauri-plugin-updater` before the installer is launched).
Without a key, an https feed and a build that was given them, the feature is off: the
Updates page says so, no request is made, nothing can be installed.

## What the maintainer must configure (one time)

1. Create a signing key pair: `pnpm --dir apps/desktop tauri signer generate -w ~/.tauri/praxi-updater.key`.
   Keep the private key and its password secret and backed up; losing it means existing
   installs can never update again.
2. GitHub > Settings > Secrets and variables > Actions:
   * secret `TAURI_SIGNING_PRIVATE_KEY` (contents of the private key file),
     secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`;
   * variable `PRAXI_UPDATER_PUBKEY` (contents of the `.pub` file),
     variable `PRAXI_UPDATER_ENDPOINT` (https URL of `latest.json`, e.g. the `latest.json`
     asset of the latest GitHub release),
     variable `PRAXI_UPDATER_PREVIEW_ENDPOINT` (separate https feed for Preview, optional),
     variable `PRAXI_UPDATER_ROLLBACK_ENDPOINT` (https URL of `rollback.json`, optional).
3. `.github/workflows/installer.yml` then also produces `*-setup.exe` + `*-setup.exe.sig`.
   Publish them with the release and write the two feeds (Tauri static JSON format):

   ```json
   {
     "version": "0.2.0",
     "notes": "Default text\n<!-- lang: en -->\nWhat is new.\n<!-- lang: tr -->\nYenilikler.\n<!-- lang: de -->\nNeuigkeiten.",
     "pub_date": "2026-12-01T00:00:00Z",
     "platforms": {
       "windows-x86_64": { "url": "https://.../Praxi-Bot_0.2.0_x64-setup.exe", "signature": "<contents of the .sig file>" }
     }
   }
   ```

   `latest.json` points at the newest release; `rollback.json` has the same format and
   points at the last release known to work (older than the newest one). Notes may hold
   one `<!-- lang: xx -->` block per language; text before the first marker is the default.
   Notes are shown as plain text.

   Tagged installer builds use the complete release version (including `-alpha.2`,
   for example) in Tauri and the signed update metadata. The updater and About use
   that same packaged version, so successive previews of one numeric version can
   update correctly and each release gets its own “What's new” dismissal.

## Channels and release notes

About shows the app version, source build ID, channel and last attempted check time.
Stable is the default and rejects prereleases even if the stable feed lists one.
Choosing Preview explicitly uses `PRAXI_UPDATER_PREVIEW_ENDPOINT`, which may include
prereleases and stable releases. A missing or invalid Preview feed disables that choice;
it never falls back to Stable while claiming to check Preview. Both feeds use the same
built-in signing key. Release builds ignore environment overrides for every feed.

Checks happen only on request and do not download the installer. “Skip this version”
remembers the offered version across restarts; newer versions are still offered.
Changing channels clears the skipped version and any pending installer candidate.
A failed check also clears any previous candidate so it cannot install stale results.

After signature verification, notes are saved locally before the installer starts.
The existing “What's new” dialog shows those notes for the running version once,
after onboarding, and remembers dismissal in local settings. Notes survive health
confirmation, use the user's en/tr/de block with English fallback, and render as
inert plain text (including HTML and links). Bundled notes remain the fallback for
manual upgrades or releases without feed notes. No network request is needed to
show saved notes.

## How rollback works

Just before the installer runs, the app writes `update-state.json` in its data folder.
Every start of the new version without a healthy run (60 seconds up) is counted:

* 2nd failed start: Settings > System > About shows "Go back to the previous version";
* 3rd failed start: the rollback recommendation remains visible. The user must click
  “Go back to the previous version”; no installer, including rollback, runs automatically.

Going back downloads the installer named by `rollback.json`, verifies its signature with
the same key, and only accepts a version **older** than the running one. A healthy run
clears the bookkeeping.

## Limits

* Windows installer (NSIS) only; other platforms are not wired up.
* The bundled `requireSignedVersion` option makes the app refuse an installer whose
  signature was made for a different version (downgrade replay). It needs releases built
  with a current Tauri CLI; re-sign older releases before listing them in a feed.
* Rollback needs the previous installer to stay available at the URL in `rollback.json`.
