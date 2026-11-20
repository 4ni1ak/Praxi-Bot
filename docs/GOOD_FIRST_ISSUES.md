# New starter tasks

These tasks are **not GitHub issues yet**. They were checked against the code and against every
open and closed issue, including the list in [starter-issues.md](contributing/starter-issues.md)
and issues #163–#238 and #316–#325. Open one issue per task, with the labels below, then people
can claim it. Do not start the work from this file alone.

Each body is written so it can be pasted into a new issue. Labels use the names already on the
repository.

## 1. Fix the German first-run freedom examples so the step does not crash

**Labels:** `bug`, `good first issue`, `help wanted`, `area: desktop-ui`, `area: i18n`, `skill: typescript`, `size: S`

## What

Turn the three `examples` values in the German first-run copy into arrays, matching English and
Turkish, and make `check:i18n` fail when a locale's JSON kind (array, object, or string) differs
from English.

## Why

`FreedomStep` does `examples.map(...)`. In `en.json` and `tr.json`, `firstRun.freedom.levels.*.examples`
is a JSON array. In `de.json` the same keys are objects (`{"0": "...", "1": "...", "2": "..."}`).
i18next then returns a plain object, and `.map` throws, so the "How much freedom?" step crashes in
German. `apps/desktop/ui/scripts/check-i18n.mjs` flattens both shapes with `Object.entries`, so CI
stays green.

## Where

- `apps/desktop/ui/src/i18n/locales/de.json` (`firstRun.freedom.levels.ask|balanced|auto.examples`)
- `apps/desktop/ui/src/i18n/locales/en.json` and `tr.json` (the shape to copy; do not edit the wording)
- `apps/desktop/ui/src/views/firstrun/FreedomStep.tsx` (the `.map` call)
- `apps/desktop/ui/scripts/check-i18n.mjs`

## How to do it

1. In `de.json`, change each of the three `examples` values from an object to an array. Keep the
   existing German sentences and their order.
2. In `check-i18n.mjs`, compare kinds as well as key names. Walk objects only; treat arrays as
   values. For every English leaf, the locale must have the same kind: array, object, or string.
   A mismatch prints `kind mismatch in de: firstRun.freedom...` and fails the script, the same way
   a missing key does.
3. Leave the passing line `i18n ok (N keys)` as it is.
4. Do not add the "identical to English" warning. That is issue #324, which is still open.

## Acceptance criteria

- [ ] `de.json` stores the three `examples` lists as arrays, with the same German text.
- [ ] `pnpm --dir apps/desktop/ui check:i18n` passes on the fixed files.
- [ ] Changing one English array into an object (a scratch edit you do not commit) makes the script
      fail with a kind mismatch, and restoring the array makes it pass again.
- [ ] `pnpm --dir apps/desktop/ui test` passes.
- [ ] `pnpm lint` passes.

## Not needed

No new translations, no change to `FreedomStep.tsx` unless a test shows the array still is not
what `.map` receives, and no new dependencies.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 to 2 hours

## 2. Correct two stale sentences in the skills guide

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: daemon`, `skill: docs`, `size: S`

## What

Fix two sentences in `docs/guide/skills.md` that disagree with the code: where a bot's learned
skill is saved, and what a `version` string is allowed to be.

## Why

The "Where skills live" list says the per-bot folder is only "supported by the skills library"
and that the daemon "saves what bots learn" in `<data dir>/skills/`. `skill_write` saves under
`<data dir>/bots/<bot_id>/skills/` (`SkillPaths` in `crates/skills/src/index.rs`; the test
`skill_write saved under the bot's own folder` in `crates/agent-core/src/tests.rs`).

The same page says `` `version`: free text. `` `check_version` in
`crates/skills/src/scoped_store.rs` rejects an empty version, anything longer than 64 characters,
a leading non-alphanumeric, characters other than letters, digits, `.`, `-`, `_` and `+`, a `..`,
and a trailing `.`. `skill_write` calls that check.

## Where

- `docs/guide/skills.md` (the per-bot bullet, and the `version` bullet)
- Code to read, not change: `crates/skills/src/index.rs` (`SkillPaths`), `crates/skills/src/scoped_store.rs` (`check_version`), `crates/agent-core/src/tests.rs` (the save-path test)

## How to do it

1. Rewrite only those two sentences so they match the code. Say that `skill_write` writes the
   calling bot's own folder, and that shared `<data dir>/skills/` is the published/shared layer.
2. Replace "free text" with the actual version grammar, in one or two sentences.
3. Do not rewrite the rest of the page. Search the page for any other sentence that still says
   learned skills are saved only in the shared folder, and fix that sentence too if you find one.

## Acceptance criteria

- [ ] Both sentences match the files named above, and the PR cites them.
- [ ] No other page is rewritten.
- [ ] `python3 scripts/check_markdown_links.py` passes.

## Not needed

No code changes and no new skill files (new example skills are issue #169).

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 hour

## 3. Add the calendar page to the user guide

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: daemon`, `skill: docs`, `size: S`

## What

Move `docs/calendar.md` into the user guide and link it from the guide index, after checking each
claim against the calendar connector.

## Why

Email has a guide page and an index row. Calendar setup lives only in `docs/calendar.md`, and
nothing in the repository links to that file. The tools and the safety rules are implemented.

## Where

- `docs/calendar.md` (move to `docs/guide/calendar.md`; a search shows no inbound links today, confirm that before moving)
- `docs/guide/index.md` (the "What is where" table, next to Email)
- Code to check, not change: `crates/connector-calendar/src/tools.rs` (tool names) and `crates/connector-calendar/src/client.rs` (https, loopback http, redirects)

## How to do it

1. Confirm the three tool names, the https rule, and "redirects are not followed" against the
   connector. Fix any sentence in the page that disagrees. Leave sentences that match.
2. Move the page to `docs/guide/calendar.md`.
3. Add one row to the index table: read and add calendar events, linking the new page.
4. Relative links only.

## Acceptance criteria

- [ ] The index links to `docs/guide/calendar.md`.
- [ ] Every sentence you changed is backed by a file path in the PR.
- [ ] `python3 scripts/check_markdown_links.py` passes. The checker only sees tracked files, so
      `git add` the new path first.

## Not needed

No connector code changes and no screenshots.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 hour

## 4. Add the Home Assistant page to the user guide

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: daemon`, `skill: docs`, `size: S`

## What

Move `docs/home-assistant.md` into the user guide and link it from the guide index, after checking
each claim against the Home Assistant connector.

## Why

The page documents a shipped connector (long-lived token, `homeassistant.toml`, three tools, a
blocked-service list) and nothing links to it. Email is in the guide; this is the same kind of page.

## Where

- `docs/home-assistant.md` (move to `docs/guide/home-assistant.md`; confirm nothing links the old path)
- `docs/guide/index.md`
- Code to check: `crates/connector-homeassistant/src/tools.rs` and `crates/connector-homeassistant/src/client.rs` (https or private/loopback http, the blocked domains, `allowed_domains`)

## How to do it

1. Check the tool names (`ha_list_entities`, `ha_get_state`, `ha_call_service`), the URL rule, and
   the blocked-service list against `client.rs`. Fix only sentences that disagree.
2. Move the page and add one index row next to Email.
3. Relative links only.

## Acceptance criteria

- [ ] The index links to `docs/guide/home-assistant.md`.
- [ ] Every sentence you changed cites the file you checked.
- [ ] `python3 scripts/check_markdown_links.py` passes after `git add`.

## Not needed

No connector code changes and no screenshots.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 hour

## 5. Link the coding-CLI page from the user guide index

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: cli-engines`, `skill: docs`, `size: S`

## What

Add one row to the "What is where" table in `docs/guide/index.md` pointing at
`docs/guide/cli-engines.md`.

## Why

The page exists and the README already links it ("support differs per CLI"). The guide index,
which is the table people use to find a page, does not. The page does not need a rewrite.

## Where

- `docs/guide/index.md` (the table only)
- `docs/guide/cli-engines.md` (read it so the row's wording matches the page; do not edit it)

## How to do it

Add one row, for example "Use a local coding CLI (Claude Code, Codex, Cursor, ...) as the engine",
linking `cli-engines.md`. Relative link. Do not copy the engine table into the index.

## Acceptance criteria

- [ ] The index table has one new row and the link resolves.
- [ ] `docs/guide/cli-engines.md` is unchanged.
- [ ] `python3 scripts/check_markdown_links.py` passes.

## Not needed

No code changes. Sign-in work for Antigravity, Cursor, and OpenCode is issues #185–#187.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 30 minutes

## 6. Link the shared-skill management page from the guide

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: daemon`, `skill: docs`, `size: S`

## What

Link `docs/guide/skill-management-api.md` from the skills guide and from the guide index.

## Why

The page documents the local-owner routes for creating, importing, updating, and deleting shared
skills. A search shows nothing links to it, so it is invisible from the guide.

## Where

- `docs/guide/skill-management-api.md` (already written)
- `docs/guide/skills.md` (a short pointer, one paragraph or one bullet)
- `docs/guide/index.md` (one table row)
- Code to check: `crates/daemon-api/src/skill_management.rs` (`POST /v1/skills`, `POST /v1/skills/import`, `PUT /v1/skills/{name}`, `DELETE /v1/skills/{name}`) and `GET /v1/skills` in `crates/daemon-api/src/skills.rs`

## How to do it

1. Compare the route table in the page with the `utoipa::path` attributes. Fix a cell that
   disagrees; leave the rest.
2. From `skills.md`, add one pointer to the page (who may call it: the local owner, not a paired
   device). Do not merge the two pages.
3. Add one index row.

## Acceptance criteria

- [ ] Both pages link to `skill-management-api.md` with relative links.
- [ ] Any corrected cell cites the route attribute.
- [ ] `python3 scripts/check_markdown_links.py` passes.

## Not needed

No API changes. New example skills are issue #169.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 hour

## 7. Link the model-catalog page from the providers guide

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: daemon`, `skill: docs`, `size: S`

## What

Link `docs/guide/model-catalog.md` from `docs/guide/providers-and-models.md` and from the guide index.

## Why

The page explains how the model picker gets display names, recommendation rank, and `model_details`.
Nothing in the repository links to it. The providers guide never mentions it.

## Where

- `docs/guide/model-catalog.md`
- `docs/guide/providers-and-models.md` (one pointer where models are chosen)
- `docs/guide/index.md` (one row, or a mention in the existing providers row if a new row does not fit)

## How to do it

Add the links. Do not refresh the "Verified on ..." model table and do not edit
`model_metadata.json`. Freshness tooling is issue #204.

## Acceptance criteria

- [ ] A reader of the providers guide can reach the catalog page.
- [ ] The catalog page's model table is unchanged.
- [ ] `python3 scripts/check_markdown_links.py` passes.

## Not needed

No Rust changes and no new provider preset (that is issue #184).

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 30 minutes

## 8. Link the knowledge-embedding setup page from the guide index

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: daemon`, `skill: docs`, `size: S`

## What

Add `docs/guide/knowledge-embeddings.md` to the guide index.

## Why

The page is the host setup guide for optional remote and local embeddings (approvals, the ledger,
`knowledge-authority.db`). The only link to it is from `docs/design/rag.md`, which is a design
note, not the guide people open first.

## Where

- `docs/guide/index.md` (one row)
- `docs/guide/knowledge-embeddings.md` (read it; do not rewrite it)
- The existing link in `docs/design/rag.md` (leave it; it should keep working)

## How to do it

Add one index row, "Set up knowledge embeddings", pointing at `knowledge-embeddings.md`. If the
first paragraph of that page says something the index row would contradict, fix the row, not the
page.

## Acceptance criteria

- [ ] The index links to the page.
- [ ] `docs/design/rag.md` still links to it.
- [ ] `python3 scripts/check_markdown_links.py` passes.

## Not needed

No embedding code and no new providers.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 30 minutes

## 9. Link file and email schedule triggers from the scheduled-tasks guide

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: daemon`, `skill: docs`, `size: S`

## What

Link `docs/schedule-triggers-prechecks.md` from `docs/guide/scheduled-tasks.md`.

## Why

The scheduled-tasks guide covers cron, intervals, and webhooks. File-watch and email triggers, and
the deterministic precheck, are documented only in `docs/schedule-triggers-prechecks.md`, which
nothing links to. `Trigger::FileWatch` is a real variant in `crates/scheduler/src/model.rs`.

## Where

- `docs/guide/scheduled-tasks.md` (a short section or pointer after the webhook section)
- `docs/schedule-triggers-prechecks.md`
- Code to confirm: `crates/scheduler/src/model.rs` (`FileWatch`) and `crates/scheduler/src/watch.rs`

## How to do it

1. Add a pointer from the guide page. Do not paste the whole reference into the guide.
2. The reference's example uses a personal directory. If you touch that file, replace that
   directory with a placeholder such as `<watch-root>/invoices`. Do not otherwise rewrite it.
3. Relative links. From `docs/guide/scheduled-tasks.md` the path is `../schedule-triggers-prechecks.md`.

## Acceptance criteria

- [ ] The scheduled-tasks guide links to the triggers page.
- [ ] `python3 scripts/check_markdown_links.py` passes.

## Not needed

No scheduler code. More natural-language phrases are issue #188, and the runnable parser example
is issue #318.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 30 minutes

## 10. Say that plugin-host is a placeholder, not a loader

**Labels:** `documentation`, `good first issue`, `help wanted`, `area: daemon`, `skill: docs`, `size: S`

## What

Replace the `plugin-host` crate docs so they no longer claim the crate loads manifests, starts
plugin processes, or runs WASM.

## Why

`crates/plugin-host/src/lib.rs` is only a module comment. That comment says the crate reads
`plugin.toml`, supervises processes or WASM, and brokers capability grants. None of that code is
in the crate. `CONTRIBUTING.md`'s crate table says "Plugin manifests" for the same crate. A
newcomer will look for an implementation that is not there. The design discussion is issue #213.

## Where

- `crates/plugin-host/src/lib.rs` (the only Rust file; it has no items)
- `CONTRIBUTING.md` (one cell in the crate table: `plugin-host`)
- Optional: a new `crates/plugin-host/README.md` of one short paragraph
- Issue #213 (link it; do not change the issue)

## How to do it

1. Rewrite the module comment to say the crate is a reserved placeholder and that plugin design
   is tracked in issue #213.
2. Change the one CONTRIBUTING table cell to the same statement.
3. If you add a README, keep it to that paragraph plus `cargo test -p plugin-host` (the crate has
   no tests today; say so).

## Acceptance criteria

- [ ] Neither the module comment nor the CONTRIBUTING cell describes a loader that is not in the file.
- [ ] No plugin implementation is added.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `python3 scripts/check_markdown_links.py` passes.

## Not needed

Do not implement plugins, manifests, or WASM. That is issue #213. Crate READMEs for the crates
listed in issue #168 are that issue, not this one.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 30 minutes

## 11. Add tests for the light-palette bot gradient

**Labels:** `good first issue`, `help wanted`, `area: desktop-ui`, `skill: typescript`, `size: S`

## What

Extend the existing gradient test so `gradientBase(color, true)` is covered.

## Why

`gradientBase` in `apps/desktop/ui/src/lib/botGradient.ts` takes a `light` flag. On a light palette
it clamps lightness to `0.42..0.62` and raises chroma to at least `0.1` (`LIGHT_BAND`), so a white
bot does not wash out. `apps/desktop/ui/src/lib/computerView.test.ts` calls `gradientBase` with one
argument only, so the light-palette branch never runs. There is no `botGradient.test.ts`; the
coverage lives in `computerView.test.ts`. Do not add a second file unless you prefer to move the
existing cases with it.

## Where

- `apps/desktop/ui/src/lib/botGradient.ts` (`gradientBase`, `LIGHT_BAND` is not exported; test through behavior)
- `apps/desktop/ui/src/lib/computerView.test.ts` (the test named "the gradient keeps a white bot tinted...")

## How to do it

1. Call `gradientBase("#ffffff", true)` and `gradientBase("#000000", true)`. Assert lightness is
   within `0.42` and `0.62`, and chroma is at least `0.1`.
2. Call `gradientBase("#3b82f6", true)` and assert the hue still matches `hexToOklch("#3b82f6")`,
   the same way the dark-palette test does.
3. Keep the existing one-argument assertions. Do not change `botGradient.ts`.

## Acceptance criteria

- [ ] The light-flag cases are in the test file and fail if you temporarily force `light` to be ignored (a scratch edit you do not commit).
- [ ] `pnpm --dir apps/desktop/ui test` passes.
- [ ] No production code changes.

## Not needed

No visual change and no new tokens. Issue #170 lists other UI modules; this function is not one of them, and it already has a partial test.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 hour

## 12. Add tests for the "last synced" time helper

**Labels:** `good first issue`, `help wanted`, `area: desktop-ui`, `area: host`, `skill: typescript`, `size: S`

## What

Add `apps/desktop/ui/src/lib/lastSync.test.ts` for `readLastSync`, `writeLastSync`, and `lastSyncParts`.

## Why

`lastSync.ts` remembers, in `localStorage`, when this computer last copied its setup to a paired
server. Nothing tests it. A bad parse should show no time, and "today" should hide the date.

## Where

- `apps/desktop/ui/src/lib/lastSync.ts`
- New file: `apps/desktop/ui/src/lib/lastSync.test.ts`
- A `window` stand-in is already used in `apps/desktop/ui/src/lib/botVoice.test.ts`

## How to do it

1. Install a fake `window.localStorage` (a `Map` is enough) and remove it in `afterEach`, the way
   `executionControls.test.ts` restores `window`.
2. `writeLastSync` then `readLastSync` returns the same milliseconds. An unreadable or non-numeric
   value, `0`, and a thrown `getItem` return `null`. `writeLastSync` must not throw when `setItem` throws.
3. `lastSyncParts`: build `ms` and `now` with `new Date(y, m, d, h, min)` so the test uses local
   time and does not depend on the machine's timezone. Same calendar day returns `day: null`. A
   different day returns a non-null `day`. `time` is a non-empty string. Pass `lang: "tr"` and
   `lang: "en"` once each.
4. Do not change `lastSync.ts`.

## Acceptance criteria

- [ ] The cases above are in `lastSync.test.ts`.
- [ ] `pnpm --dir apps/desktop/ui test` passes.
- [ ] No production code changes.

## Not needed

No UI change and no daemon change. The daemon does not store this timestamp; do not add one.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 to 2 hours

## 13. Add tests for the chat "now" summary

**Labels:** `good first issue`, `help wanted`, `area: desktop-ui`, `skill: typescript`, `size: S`

## What

Add `apps/desktop/ui/src/lib/runNow.test.ts` for `runNow`.

## Why

`runNow` picks the chat's live top-level run, its current step, a pending approval, and the
sub-agent count. The right panel and the slim status line both use it. It has no test. The
fixture style already exists in `activityPanel.test.ts`.

## Where

- `apps/desktop/ui/src/lib/runNow.ts`
- `apps/desktop/ui/src/lib/activityPanel.test.ts` (copy the small `run` / `tool` helpers; `EventsState` there is `{ runs, lastGseq: 0 }`)
- `currentStep` in `activityPanel.ts` (discovery tools are not a "step")

## How to do it

Cover at least:

1. `runNow(events, undefined)` is `null`.
2. A chat whose only run has `finishedMs` set is `null`.
3. A live run (`finishedMs` undefined) is returned, with `startedMs` taken from that run.
4. When several top-level runs exist, the live one is chosen even if an older one is unfinished
   only if it is the latest such run (the code walks `topLevelRuns` in reverse). Set up two live
   runs and assert which one comes back, and write that down in a comment so the order is pinned.
5. A tool item with `status: "awaiting_approval"` is `approval`. A `find_tool` item is not `step`
   when a real tool sits earlier in the list (`currentStep` skips discovery tools).
6. `subagents` counts runs whose `parentRunId` is the live run's id, including finished children.

Do not change `runNow.ts`. If a case shows the function does something surprising, assert the
current behavior and mention it in the PR instead of "fixing" it.

## Acceptance criteria

- [ ] The cases above pass under `pnpm --dir apps/desktop/ui test`.
- [ ] No production code changes.

## Not needed

No component test and no browser. Issue #170's module list does not include this file.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 to 2 hours

## 14. Add tests for the read-aloud guard

**Labels:** `good first issue`, `help wanted`, `area: desktop-ui`, `skill: typescript`, `size: S`

## What

Add `apps/desktop/ui/src/lib/speaker.test.ts` for `speechSupported`, `speak`, and `stopSpeaking`.

## Why

`speaker.ts` is the read-aloud path (`window.speechSynthesis`). It has no tests. The guards
matter: with no speech engine, `speak` must return without throwing, and a second `speak` cancels
the previous utterance.

## Where

- `apps/desktop/ui/src/lib/speaker.ts`
- New file: `apps/desktop/ui/src/lib/speaker.test.ts`
- Window stand-in: `apps/desktop/ui/src/lib/botVoice.test.ts`

## How to do it

1. With no `window`, `speechSupported()` is false, and `speak(...)` and `stopSpeaking()` do not throw.
   `currentSpeakingId()` stays `null`.
2. Install a fake `window.speechSynthesis` with `cancel`, `speak`, and `getVoices` (return `[]`),
   and a fake `SpeechSynthesisUtterance` class that records `text`, `lang`, `rate`, `onend`, and
   `onerror`. `speak("id", "hello", { voiceName: "", rate: 1, lang: "en" })` calls `cancel`, then
   `speak`, and `currentSpeakingId()` is `"id"`. Calling `onend` clears the id. `speak` with a
   blank string does not call `speak` on the fake.
3. `stopSpeaking()` calls `cancel` and clears the id.
4. Restore `window` afterwards. Do not change `speaker.ts`.

## Acceptance criteria

- [ ] The cases above pass under `pnpm --dir apps/desktop/ui test`.
- [ ] No production code changes.

## Not needed

No new voice feature. Voice input and a voice conversation are issue #229; this task only tests
the helper that is already there.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 to 2 hours

## 15. Test the MCP verb classifier

**Labels:** `good first issue`, `help wanted`, `area: desktop-ui`, `area: apps`, `skill: typescript`, `size: S`

## What

Add direct tests for `mcpVerb` in `apps/desktop/ui/src/lib/toolLabels.test.ts`.

## Why

`mcpVerb` decides whether an MCP action is worded as search, read, draft, or send. The existing
tests cover a search action (`chat.step.mcp_search`) and a `createIssue` action that stays on the
generic `chat.step.mcp` key. The draft, send, and read branches are untested, so a change to the
word lists would not be caught.

## Where

- `apps/desktop/ui/src/lib/toolLabels.ts` (`mcpVerb`, and the `describeTool` branch that uses it)
- `apps/desktop/ui/src/lib/toolLabels.test.ts`

## How to do it

Assert `mcpVerb` (import it; it is already exported):

- `"draft reply"` and `"save drafts"` → `"draft"` (draft is checked before send)
- `"send message"`, `"post update"`, `"reply thread"` → `"send"`
- `"search pull requests"` → `"search"`
- `"get file"`, `"list issues"` → `"read"`
- `"create Issue"` → `null`

Also one `describeTool` assertion each for a `server.get_file` name (key `chat.step.mcp_read`) and
a `server.send_message` name (key `chat.step.mcp_send`), so the key prefix is pinned. Do not change
`toolLabels.ts`.

## Acceptance criteria

- [ ] The cases above are in `toolLabels.test.ts`.
- [ ] `pnpm --dir apps/desktop/ui test` passes.
- [ ] No production code changes.

## Not needed

No locale edits. The Turkish `{{appAt}}` placeholder is intentional (`trLocative`); do not make
Turkish use `{{app}}`.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 hour

## 16. Add direct tests for the skill frontmatter edge cases

**Labels:** `good first issue`, `help wanted`, `area: daemon`, `skill: rust`, `size: S`

## What

Add tests for the frontmatter branches that `parse_skill` does not hit today.

## Why

`crates/skills/src/frontmatter.rs` accepts a BOM, JSON-style escapes inside double quotes,
comments, and it rejects an unterminated quote in a list, an empty key, unexpected indentation,
and a duplicate key inside `when`. `crates/skills/src/tests.rs` covers the happy path and several
`parse_skill` errors, but none of those frontmatter branches. A parser change can break them
silently.

## Where

- `crates/skills/src/frontmatter.rs` (`split`, `parse`, `unquote`)
- `crates/skills/src/tests.rs` (crate tests can call `frontmatter::`; the module is `pub(crate)`)

## How to do it

Add tests, and do not change the parser:

1. `split` on a file that starts with U+FEFF then `---` returns the frontmatter, not an error.
2. `parse` of `title: "say \"hi\""` yields the scalar `say "hi"`.
3. A `# comment` line and a blank line are skipped.
4. These fail: a list with an unterminated quote (`[ "a ]`), an empty key (`: value`), an indented
   line when no `when:` block is open, and a duplicate key inside `when`.
5. Assert the error string contains the existing phrase (`unterminated quote`, `empty key`,
   `unexpected indentation`, `duplicate key`).

## Acceptance criteria

- [ ] The cases above are in the skills crate tests.
- [ ] No change to non-test code.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy -p skills --all-targets -- -D warnings` passes.
- [ ] `cargo test -p skills` passes.

## Not needed

No new frontmatter syntax. Hashing tests for the skill hub are issue #317. Example skills are
issue #169.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 to 2 hours

## 17. Test custom provider setup and the "keep the old key" path

**Labels:** `good first issue`, `help wanted`, `area: daemon`, `skill: rust`, `size: S`

## What

Add unit tests for `custom_entry`, `entry_for`, and the re-add path of `configure_provider`.

## Why

`crates/provider-catalog/src/setup.rs` is the one function `bot providers add`, the setup wizard,
and the host-admin route all use. It has no tests of its own. The important untested behavior is:
a custom base URL loses a trailing slash, an unknown catalog id is `SetupError::Unknown`, and
adding the same provider again without a new key keeps the key that was already stored.

## Where

- `crates/provider-catalog/src/setup.rs`
- `secret_store::MemoryStore` (already a public type; `provider-catalog` depends on `secret-store`)
- `tempfile` is already a dev-dependency of `provider-catalog`
- `Catalog` derives `Default`, so an empty catalog is `Catalog::default()`

## How to do it

1. `custom_entry("local", "http://127.0.0.1:11434/v1/")` has `base_url == "http://127.0.0.1:11434/v1"`,
   `models_endpoint == Some("http://127.0.0.1:11434/v1/models")`, kind `OpenAiCompatible`, and
   `auth` is `Auth::None` (the key is not stored on the entry).
2. `entry_for(&ProviderChoice::Catalog("nope".into()), &Catalog::default())` is `Err(SetupError::Unknown)`.
3. `configure_provider` with `ProviderChoice::Custom`, a `MemoryStore`, a temp data dir, a key
   `"secret-a"`, and a model: the settings' key name is `api_key_secret_name("local")` and the
   store returns `"secret-a"`. Call it again with `key: None` and `model: None`. The stored secret
   is still `"secret-a"` and `default_model` is still the first model.
4. Do not change `setup.rs`.

## Acceptance criteria

- [ ] The three cases above pass.
- [ ] No change to non-test code.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy -p provider-catalog --all-targets -- -D warnings` passes.
- [ ] `cargo test -p provider-catalog` passes.

## Not needed

No new provider preset (issue #184) and no change to the wizard's own error text.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 to 2 hours

## 18. Accept left/right modifier names in the Windows key parser

**Labels:** `enhancement`, `good first issue`, `help wanted`, `area: daemon`, `platform: windows`, `skill: rust`, `size: S`

## What

Make `desktop-windows`'s `parse_key` accept the left/right modifier names that `desktop-linux`
already accepts.

## Why

Both parsers are documented as understanding names such as `"ctrl"` and `"Return"`. Linux
(`crates/desktop-linux/src/pure.rs`) accepts `lctrl`, `rctrl`, `lshift`, `rshift`, `lalt`, `ralt`,
`altgr`, `lwin`, `rwin`, and `rsuper`. Windows (`crates/desktop-windows/src/pure.rs`) returns
`None` for those, so a key name that works on Linux fails on Windows with `unknown key`. The
virtual-key constants are already in `crates/desktop-windows/src/live_view_win.rs`
(`VK_LCONTROL` `0xA2`, `VK_RCONTROL` `0xA3`, `VK_LSHIFT` `0xA0`, `VK_RSHIFT` `0xA1`,
`VK_LMENU` `0xA4`, `VK_RMENU` `0xA5`, `VK_LWIN` `0x5B`, `VK_RWIN` `0x5C`).
`is_win_extended_vk` already treats `VK_RCONTROL` and `VK_RMENU` as extended keys.

## Where

- `crates/desktop-windows/src/pure.rs` (`parse_key` and its tests)
- `crates/desktop-windows/src/input.rs` (the caller; read it, change it only if a right-side key
  would miss `KEYEVENTF_EXTENDEDKEY`)
- `crates/desktop-linux/src/pure.rs` (the name list to match; do not change Linux behavior)

## How to do it

1. Map `lctrl`/`rctrl`, `lshift`/`rshift`, `lalt`/`ralt`/`altgr`, `lwin`/`rwin`/`rsuper` to the
   constants above. Leave the generic names (`ctrl`, `shift`, `alt`, `win`) on the virtual keys
   they use today (`0x11`, `0x10`, `0x12`, `0x5B`).
2. Add assertions next to the existing `parse_key("Ctrl")` test.
3. Confirm a right-ctrl press still gets the extended-key flag (`is_win_extended_vk`). If
   `input.rs` uses only `pure::is_extended`, route the new codes through `is_win_extended_vk`
   and add one assertion. Do not change the scan-code table.

## Acceptance criteria

- [ ] Each name above parses to the virtual key listed.
- [ ] Existing `parse_key` assertions still pass, including `parse_key("nonsense") == None`.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy -p desktop-windows --all-targets -- -D warnings` passes.
- [ ] `cargo test -p desktop-windows` passes.

## Not needed

No Linux changes, no new key names beyond that list, and no change to the UIA click path.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 to 2 hours

## 19. Tell the model that app_post clicks are not implemented

**Labels:** `enhancement`, `good first issue`, `help wanted`, `area: daemon`, `platform: windows`, `platform: linux`, `skill: rust`, `size: S`

## What

State, in the desktop click tool's schema, that `method: "app_post"` is not implemented on Windows
or Linux and the call will fail. Lock that sentence with a test.

## Why

The tool schema offers `"accessibility" | "app_post" | "global"`
(`method_prop` in `crates/builtin-tools/src/desktop.rs`) and the description does not say
`app_post` fails. Windows (`crates/desktop-windows/src/uia.rs`) and Linux
(`crates/desktop-linux/src/atspi_tree.rs`) both return `DesktopError::Unsupported` with
"app_post click is not implemented ... yet". A model that picks `app_post` always errors. Nothing
tests that sentence because it is not in the schema.

## Where

- `crates/builtin-tools/src/desktop.rs` (`method_prop`, around the `"enum":["accessibility","app_post","global"]` value, and `parse_method`)
- An existing test module in that file, or a small new test next to the schema construction
- Do not edit the two backend `Unsupported` arms except to keep their wording if you quote it

## How to do it

1. Extend the `method` property's `description` so it says `app_post` is not implemented on Windows
   or Linux yet and the call fails. Keep `app_post` in the enum: `crates/workflow-replay` still
   records the method, and removing it would be a separate change.
2. Add a test that builds the tool spec (or reads the `method_prop` JSON the registration uses)
   and asserts the description contains "not implemented". Follow an existing test in `desktop.rs`
   for how specs are constructed.
3. Do not implement the click.

## Acceptance criteria

- [ ] The schema description says `app_post` is not implemented on Windows or Linux.
- [ ] A test fails if that sentence is removed.
- [ ] `parse_method(Some("app_post"), ...)` still returns `ClickMethod::AppPost` (behavior of the parser stays).
- [ ] `cargo fmt --all -- --check`, `cargo clippy -p builtin-tools --all-targets -- -D warnings`, and `cargo test -p builtin-tools` pass.

## Not needed

No Windows or Linux click implementation, and no change to workflow replay's stored method names.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 1 to 2 hours

## 20. Encode IMAP folder names as modified UTF-7

**Labels:** `enhancement`, `good first issue`, `help wanted`, `area: daemon`, `skill: rust`, `size: S`

## What

Add a modified UTF-7 encode and decode for IMAP folder names, with known-answer tests, and use it
when a folder name is sent to the server or read back.

## Why

`crates/connector-email/src/lib.rs` says folder names are passed through with no modified UTF-7
conversion, so a non-ASCII folder only works if the caller already used the server's encoding.
`check_folder` in `crates/connector-email/src/imap.rs` only rejects an empty name and CR, LF, or
NUL. `session.select(folder)` then sends that string. RFC 3501 section 5.1.3 (modified UTF-7) is
the encoding IMAP mailbox names use.

## Where

- New helpers next to `check_folder` in `crates/connector-email/src/imap.rs`, or a small sibling
  module if the file is already hard to navigate. Keep them `pub(crate)`.
- Call sites that pass a folder name into the IMAP session (`select`, list, search, store) and the
  place that returns folder names to the tool. Read `list_folders` and `set_seen` in the same file
  before editing.
- Tests in the existing `imap.rs` test module (there is already a `check_folder("INBOX\r\nX")` assertion).

## How to do it

1. Encode and decode modified UTF-7. Verify these against RFC 3501 before committing them:
   - Printable ASCII other than `&`, including `INBOX` and `Drafts`, is unchanged.
   - A literal `&` that is not already a shift becomes `&-`, so `R&D` becomes `R&-D`.
   - `Entwürfe` becomes `Entw&APw-rfe`.
   - Decode is the inverse, and a bad `&` sequence returns the existing `EmailError::Input` style
     of error rather than panicking.
2. Callers can already pass a server-encoded name, because that is what the crate does today.
   Do not encode those a second time: `Entw&APw-rfe` sent again would become `Entw&-APw-rfe` and
   select a different mailbox. After `check_folder`, if the name is already valid modified UTF-7
   (ASCII with well-formed `&...-` shifts, or no `&` at all), send it unchanged. Otherwise treat
   it as a Unicode name and encode it. Decode names in the list returned to the tool. Add a test
   that `Entw&APw-rfe` and `Drafts` are not rewritten. A name with no `&` and no non-ASCII
   character stays byte-for-byte the same, so current tests that expect `"INBOX"` and `"Drafts"`
   keep passing.
3. Do not change TLS, login, or the message-size cap.

## Acceptance criteria

- [ ] The vectors above pass, including the `&` and `Entwürfe` round trip.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy -p connector-email --all-targets -- -D warnings` passes.
- [ ] `cargo test -p connector-email` passes.

## Not needed

No network test and no change to the user guide unless a sentence there says non-ASCII folders
must be pre-encoded. If you find that sentence, correct it in the same PR.

## Claiming

Comment on the issue to claim it; one pull request per issue. See CONTRIBUTING.md.

Size: S, about 2 to 3 hours
