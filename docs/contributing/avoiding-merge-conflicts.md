# Avoiding merge conflicts in shared files

Several files are touched by almost every feature. Parallel pull requests used to collide
in them. The lists below are kept **sorted, one entry per line** so that two branches add
their lines in different places and Git merges them without a conflict. Keep them that way.

| Shared file | Rule |
| --- | --- |
| `apps/desktop/ui/src/i18n/locales/{en,tr,de}.json` | Keys are sorted alphabetically at every level. Add the key where it sorts, then run `pnpm --dir apps/desktop/ui sort:i18n`. `check:i18n` fails when a file is unsorted. Never append at the end of the file. |
| `apps/desktop/ui/src/lib/humanError.ts` (`DAEMON_ERROR_CODES`) | Sorted, one code per line; a unit test checks it. Also add `errors.<code>` in all three locales. |
| `crates/daemon-api/src/openapi.rs` | `use crate::x;` one module per line (sorted); `paths(...)`, `components(schemas(...))` and `tags(...)` hold one entry per line in alphabetical order. Insert your entry at its sorted position, not at the end of a list. |
| `docs/api/openapi.json`, `apps/desktop/ui/src/api/schema.d.ts` | Generated. On a conflict never merge by hand: take main's file, then regenerate (`UPDATE_OPENAPI=1 cargo test -p daemon-api openapi_snapshot`, `pnpm --dir apps/desktop/ui gen:api`). |

## Habits that keep conflicts small

- Put new routes in a new module (`crates/daemon-api/src/<feature>_routes.rs`) instead of
  growing `routes.rs`; register only the `use` line and the sorted path entries.
- Start new daemon hosts in their own file (`apps/cli/src/<feature>_host.rs`) and keep the
  change in `serve.rs` to a single call line.
- Put new UI code in new components/hooks and add only an import line to `ChatView.tsx` and
  similar large screens, at its sorted position.
- Rebase (or merge) `origin/main` right before pushing; locale files conflict the least when you
  re-run `sort:i18n` after resolving.
