# Setup recipes

A recipe turns a goal ("I want a morning briefing") into a ready bot: the bot's
instructions and permissions, the apps it needs, and an optional schedule, shown
in plain words **before anything is created**.

Recipes are data files, so anyone can write one. The built-in ones live in
`crates/daemon-api/templates/recipes/` and are served by
`GET /v1/profile-share/recipes`. The format is versioned: `format` is
`praxi-recipe`, `format_version` is `1`. A newer version is refused with a clear
message; unknown fields are refused too.

## File

```json
{
  "format": "praxi-recipe",
  "format_version": 1,
  "id": "daily-briefing",
  "icon": "sun",
  "names":        { "en": "...", "tr": "...", "de": "..." },
  "descriptions": { "en": "...", "tr": "...", "de": "..." },
  "level": "approve-destructive",
  "can":    { "en": ["..."], "tr": ["..."], "de": ["..."] },
  "cannot": { "en": ["..."], "tr": ["..."], "de": ["..."] },
  "apps": [
    { "catalog_id": "fetch", "why": { "en": "...", "tr": "...", "de": "..." } }
  ],
  "schedule": {
    "title":  { "en": "...", "tr": "...", "de": "..." },
    "prompt": { "en": "...", "tr": "...", "de": "..." },
    "freq": "weekdays",
    "time": "08:00"
  },
  "profile": { "format": "praxi-bot-profile", "format_version": 1, "bot": { }, "permissions": { }, "tools": { } }
}
```

| Field | Rules |
| --- | --- |
| `id` | Short slug, unique. |
| `icon` | A lucide icon name. |
| `names`, `descriptions` | Text in **all of** `en`, `tr`, `de` (no other language, none missing). Real translations, not English copied. |
| `level` | `read-only`, `read-and-draft` or `approve-destructive`. Never `full`. Must equal `profile.permissions.preset`. |
| `can`, `cannot` | 1 to 8 short plain-language lines per language: what the bot will and will not be able to do. Write what is true; the app also shows what the permission engine computed. |
| `apps` | At most 4. Only an MCP catalog id (`crates/mcp-bridge/catalog.json`) and a reason. No commands, URLs, secrets or folders. Apps that need a folder cannot be used. |
| `schedule` | Optional. `freq` is `daily`, `weekdays` or `weekly` (weekly needs `weekday` 0 = Sunday to 6). `time` is local `HH:MM`. |
| `profile` | A [bot profile](guide/bots-and-profiles.md). `meta` and `schedules` must stay empty (the recipe has its own schedule). |

The profile and every text field go through the same checks as a shared profile
file: size and length caps, no control or hidden-direction characters, nothing that
looks like a secret.

## What a recipe can and cannot do

* **It is only data.** Reading a recipe installs, connects, schedules and creates
  nothing.
* **The user's choice is the ceiling.** The preview shows what the bot would be
  allowed to do and lets the user pick the highest permission level. The bot is
  created through the profile importer, which lowers every category to that level
  and never raises anything. The recipe's `level` is only the starting choice.
* **Apps arrive switched off.** After the bot exists, each app is added switched
  off and only for that bot, with no secret stored. The user connects and approves
  it in Apps. An app the user already has is left exactly as it is.
* **The schedule arrives switched off.** The user turns it on in Schedules.
* **The persona is shown in full** in the preview: it is the bot's instructions.

## Write your own

1. Copy a file from `crates/daemon-api/templates/recipes/` and change it.
2. Add it to `RECIPE_FILES` in `crates/daemon-api/src/recipes.rs`.
3. Run `cargo test -p daemon-api recipes`. The tests check the format, the three
   languages, that every app exists in the catalog, and that no recipe can give
   more than the confirmed level at any ceiling.
