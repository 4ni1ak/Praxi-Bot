# Managing shared skills from the local app

The local owner can create, import, update and delete shared skills through the
daemon API. All paired roles, including paired desktops, are refused these
mutations. The normal loopback bearer and Origin/Host checks still apply.

## Routes

| Method and path | Input | Result |
| --- | --- | --- |
| `POST /v1/skills` | `{name, description, instructions}` or `{skill_md}` | 201 with name, description, version and scope |
| `POST /v1/skills/import` | `{kind:"folder", path:"absolute path"}` or `{kind:"zip", base64:"..."}` | 201 with metadata; 409 if the logical name exists |
| `PUT /v1/skills/{name}` | Same document input, optionally `expected_version` | 200; atomic SKILL.md replacement, auxiliary files retained |
| `DELETE /v1/skills/{name}?force=false` | Optional explicit `force=true` | 200 with `deleted` and `used_by`; 409 with affected bots when assigned |
| `GET /v1/skills` | Existing catalog request | Adds local-only `locations.shared`, `locations.bot_template`, `locations.bundled` |

`skill_md` is a complete SKILL.md including the ordinary frontmatter. Field
input produces the same format and goes through the same runtime parser. The
document's name must match the URL on update. Reserved Windows device names
and the `import` route name are not available as owner-managed skill names.

An all-skills assignment also counts as use. A forced deletion leaves selected
assignment names in place as `missing`; reinstalling the same name restores the
selection. Bundled skills and per-bot skills are not deleted by these routes.
Catalog mutations and assignment writes are serialized inside the daemon.
Independent skill-store instances additionally share the existing filesystem
scope lock. A same-name concurrent create has one success and one conflict.

Published `name@version` directories remain visible throughout an owner
update. The document is replaced atomically before its directory is renamed;
a small version-only marker lets the next audited owner mutation or bot
publication finish an interrupted rename. Fresh runtime readers can use the
complete old or new document without doing repair writes. This covers process
interruption, not power-loss durability guarantees for filesystem metadata.

## Import limits and behavior

- One SKILL.md at the archive/folder root; no wrapper-folder guessing.
- At most 128 files, 256 entries including directories, 16 path segments,
  1 MiB per auxiliary file, 64 KiB for SKILL.md, and 8 MiB total file bytes.
- ZIP uploads are capped at 8 MiB compressed and 12 MiB JSON request size.
- Only stored/deflate ZIP entries are supported by the selected zip features.
- Each path segment is ASCII letters, digits, dots, underscores or hyphens.
  Absolute paths, parent traversal, backslashes, drive/stream separators,
  hidden entries, trailing dots and Windows device names are rejected.
- Symlinks, junctions/reparse points, special files, case-insensitive duplicate
  paths and executable Unix file modes are rejected. Directory traversal bits
  are allowed. Source modes are never copied to the new files.
- UNIX folder reads use descriptor-relative `openat` with `NOFOLLOW`; Windows
  holds non-reparse directory/file handles without write/delete sharing while
  reading. Source folders/files that are being changed may be refused.
- No imported program, script, package hook or command is run. Auxiliary script
  source files with ordinary non-executable modes may be stored as inert data.
- Known secrets are checked with the running agent's production redactor,
  including environment-only registered values, before any publication. A
  matching document or auxiliary file is refused, not silently rewritten.

Owner-created files use the existing shared/user source. This does not change
the behavior of bot `skill_write` or `skill_publish`. Main at `11917d2` has no
native #89 trust-sidecar/taint implementation yet; this PR does not claim to
introduce that system. When reconciling #89, preserve its bot-side markings.

Each mutation commits a host-authored audit intent before changing the skill
files. The payload contains only the file count, never names, instructions,
source paths or archive content. Audit failure blocks the mutation. Responses
and catalog metadata use the production redactor; parser and filesystem errors
return fixed messages without reflecting supplied content.

The existing picker redesign can use these routes directly. No new Tauri
command, UI dialog, CSS or localized string is added here.

## Install from a git repository (preview, then install)

`POST /v1/skills/hub/preview` with `{ "url": "https://host/owner/repo.git", "rev": null, "subdir": null }`
fetches the repository into a private staging folder, scans it and returns the skill's name, a risk level
(`low`, `medium`, `high`), the findings (scripts, network access, credential patterns, obfuscation,
instruction injection) and a `fingerprint`. Nothing is installed.

`POST /v1/skills/hub/install` takes the same body plus `"accept": "<fingerprint>"`. It fetches again and
installs only when the fresh fingerprint equals the accepted one (otherwise `409 skill_changed`). The
install uses the same owner path as `/v1/skills/import`: links and executable files are refused, secrets
are refused, nothing is executed. Local owner only; only `https://` repositories on a named host.
