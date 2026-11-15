# Chatting and approvals

## Talking to a bot

Every conversation is a **chat** with one bot. Chats are kept, so you can come
back later. A message starts a **run** (also called a turn): the bot thinks,
calls tools, and streams its answer back. You can follow it in the desktop app,
in a terminal, or in Telegram.

From the terminal:

```sh
bot chat <bot-id> "Summarize the PDF files in ~/Downloads"
bot chat <bot-id> "and now rename them by date" --chat <chat-id>   # continue
```

`bot chat` creates a new chat unless you pass `--chat`. It prints the answer as
it streams. Tool calls, approval requests and routing decisions are printed to
stderr, the answer text to stdout (so `bot chat ... > answer.txt` works). With
the global `--json` flag stdout becomes NDJSON, one event per line.

### Sending a message while the bot is busy

If a turn is already running in the chat, `--mode` decides what happens:

- `queue` (default): the message waits and runs as the next turn.
- `steer`: the message is added to the running turn so you can redirect the bot.

### Slash commands

When the router is enabled, a few messages never reach the model at all:
`/stop` (stop the current run), `/new` and `/workflow <name> key=value ...` (run
a saved workflow directly, see [Workflows](workflows.md)). In Telegram there are
extra commands, see [Telegram](telegram.md).

### Questions

A bot can ask you something with a question card (`ask_user`). In the CLI you
see `[question]` with numbered options and answer with:

```sh
bot answer <call-id> "the second one"
```

## Approvals

When a bot wants to do something its policy says needs your say-so, the run
**waits** and an approval request appears: a card in the desktop app, inline
buttons in Telegram, or a `[approval needed]` line in the CLI.

```sh
bot status                              # how many approvals are pending
bot approve <tool-call-id>              # approve this one call
bot approve <tool-call-id> --scope this_chat
bot approve <tool-call-id> --deny --reason "not that folder"
```

The reason you give on a denial is passed to the model so it can try something
else.

### How far an approval reaches

| Scope | Meaning |
| --- | --- |
| `once` (default) | This single call only |
| `this_chat` | This tool, for the rest of this chat |
| `always` | This tool, for this bot, remembered permanently |
| `always_category` | The tool's whole category (for example every `fs` tool) for this bot, remembered permanently |

"Always" choices are stored as **grants**. Review and remove them:

```sh
bot grants list <bot-id>
bot grants revoke <bot-id> --tool fs_write
bot grants revoke <bot-id> --category fs
```

After a revoke the next matching call asks again. Revocations are written to
the audit log. Grants never lift the absolute floors below.

## Permission levels

Every tool belongs to a **category**, and every category has a **level** for
each bot.

| Level | What happens |
| --- | --- |
| Deny | The call is refused, the bot is told |
| Ask | The run pauses until you approve or deny |
| Allow | The call just runs |
| Allow with limits | Meant as "allowed within limits"; the current resolver treats it as Allow |

Categories: `fs` (files), `shell` (commands), `desktop` (clicks and typing in
apps), `browser`, `accounts` (connector accounts), `messaging` (email, chat),
`money` (payments, purchases), `system` (settings, installing software),
`remote` (another computer) and `script` (code execution). The internal
`meta` category (asking you a question, keeping a to-do list) is always allowed.

### The four presets

| Preset | In plain words |
| --- | --- |
| `approve-destructive` (default, recommended) | Every category is set to Ask, but **reading tools are waived** (see below). Anything that changes files, clicks or types in apps, runs commands or sends messages asks first |
| `read-and-draft` | Like the default; files and browser are explicitly Ask. Sending asks |
| `read-only` | Every category is Deny. The bot can only use tools that ask nothing of the permission system, such as asking you a question |
| `full` | Every category is Allow: no prompts, except for the floors below. Use it only for bots in a sandbox you trust |

"Reading tools are waived": the daemon lets tools that only read (for example
`fs_read`, `fs_list`, `browser_snapshot`, `desktop_get_state`) skip a policy-level
"Ask". This does **not** apply to `read-only`/Deny, and never to the floors.

> The wording of presets in this guide follows the code. The preset names
> `read-and-draft` and `approve-destructive` differ only slightly today; both
> ask for file and browser actions.

### Absolute floors

These hold no matter what preset, grant or profile says:

1. **Money always asks.** A `money` action is never auto-allowed.
2. **The never-list always asks.** Shell commands that are catastrophic and
   irreversible are recognized by parsing the command (not by loose string
   matching), including inside `sh -c`, `powershell -Command`, `sudo` and
   pipelines: formatting disks, `diskpart clean`, `mkfs`, `dd` onto a device,
   `bcdedit`, bootloader installers, and recursive deletes that target a
   protected location (filesystem or drive root, your home folder, system
   directories such as `C:\Windows`, `Program Files`, `/etc`, `/usr`). Deleting
   `/tmp/x` recursively is not on the list.
3. **Subagents never exceed their parent bot.**

Grants ("always allow") cannot waive a floor.

### Runs started by a schedule

A scheduled run (see [Scheduled tasks](scheduled-tasks.md)) behaves like you
unless its rule says otherwise. If it needs an approval and nobody answers
within **30 minutes**, the run **pauses** (it does not guess). Money and
never-list actions are never auto-approved in scheduled runs.

## Controlling the desktop

Desktop tools (Windows only today) act on your real screen. The desktop app
shows an on-screen overlay while a bot is driving and lets you take over:

- If you take control (`human_driving`) or pause the bot, desktop actions are
  refused immediately and the bot is told not to retry.
- The global stop hotkey (default `Ctrl+Alt+Esc`) stops the bot at any time.

The browser tools use a separate, isolated browser profile by default, so they
do not touch your normal browsing sessions.

## Undoing file changes

File tools take a **checkpoint** (a backup of the affected paths) before every
change. The bot can list and restore checkpoints (`checkpoint_list`,
`checkpoint_restore`); restoring first takes a checkpoint of the current state so
the restore itself can be undone. Shell commands and desktop actions are not
covered by checkpoints.

Restoring a whole run (or a connected set of files) from the run's change
review works in two phases. First the daemon writes an audit row
(`checkpoint.restore_started`) and takes a safety checkpoint of the files as
they are now. Only after that checkpoint is complete does it stage each file
next to its target and swap the staged copies in with renames. If a step fails,
the daemon tries to roll the swapped files back; that rollback is best effort
and can itself fail, in which case the result reports it and the files may be
in a mixed state. A graceful stop of the daemon waits for a running restore to
finish. After a restore that completes cleanly, the files count as reviewed; if
one of them is edited afterwards it shows up as "changed since" and asking to
restore it again needs confirmation. A path whose restoration was skipped (for
example a link that could not be recreated) is not marked as reviewed. After a
restore that failed part way, nothing is marked as reviewed, so the next
attempt shows "could not verify" and asks you to confirm.

If the daemon process is killed hard (power loss, crash, `kill -9`) in the middle
of a restore, no completion is recorded. What is left depends on the moment:

- Killed while the safety checkpoint was still being copied: an audit row
  `checkpoint.restore_started`, no usable safety checkpoint and no swapped
  files. Run the undo again.
- Killed after the swaps began: the audit row, a complete safety checkpoint
  (operation label `pre-restore:...`, recovery id `cp-...`, listed in
  `GET /v1/runs/{id}/changes` as `safety_checkpoint_ids`) holding the files as
  they were just before the restore, possibly a mix of restored and not yet
  restored files, and temporary files next to the targets named
  `<filename>.praxi-restore-<uuid>`, `<filename>.praxi-restore-old-<uuid>` or
  `<filename>.praxi-restore-aborted-<uuid>`.

To recover in the second case, restore that safety checkpoint (from the run's
change review, or `checkpoint_restore` with its `cp-...` id). Its preview can
show "could not verify", and you then have to confirm the overwrite. Delete the
leftover temporary files if any remain, then run the undo again. Nothing is
repaired automatically on startup. The checkpoint manifest is synced to disk, but
the copied file contents are not individually synced before the swaps, so a
power loss can still lose a safety copy; a plain process kill cannot.

Paths of other runs that are still active are compared before a restore. The
comparison ignores case on Windows and macOS and resolves links with the same
no-follow, local-only rules the file tools use. A path that cannot be verified
that way (a link to a network location, a network drive, an unreadable folder,
or more paths than the daemon will resolve in one check) counts as overlapping
and the restore is refused until the other run finishes. It can still miss hard
links, bind mounts and aliases created afterwards, so avoid undoing files that
another running bot is working on.

## Limits per turn

To stop runaway runs, each turn is capped by default at 40 model steps, 2
million tokens, 120 seconds per tool call and 300 seconds per model call, and
stops when it repeats an identical tool call several times in a row. Very long
tool output is saved to a file and only a short preview is shown to the model.
