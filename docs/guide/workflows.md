# Workflows: record, replay, remember

Calling a language model for every click is slow and costs money. Praxi's answer
is to turn a task that worked once into something it can do again with **no
model calls**. There are three related mechanisms. Know which exist today:

| Mechanism | What it does | Status |
| --- | --- | --- |
| **Replay** | Run a saved, parameterised workflow deterministically | Available |
| **Remember** | Tell Praxi "this request should go straight to the route that just worked" | Available (desktop app button, API) |
| **Record** | Review a successful run and explicitly save its mutating steps | Available (desktop review, API, CLI) |

## Workflows

A workflow belongs to one bot and is stored at
`<data dir>/bots/<bot-id>/workflows/<name>.toml`. Its index is rebuilt automatically.
This location is inside the daemon state boundary protected from bot file tools.
Legacy files at `<data dir>/workflows` are not discovered by the daemon.
A workflow has **parameters** and a list of **steps**. `workflow_list` and
`workflow_run` use the calling bot's store.

```toml
version = 1
name = "notepad_note"
description = "Open Notepad and type a note"

[[params]]
name = "text"
type = "string"          # string | number | bool
default = "hello"        # no default = required
description = "Text to type"

[[steps]]
kind = "launch"
executable = "notepad.exe"

[[steps]]
kind = "desktop_action"
app = "notepad.exe"
window_title_pattern = "*Notepad"   # * and ? wildcards, case-insensitive
action = "type"                     # click | type | set_value | key
value = "{text}"
[steps.target]
role = "edit"
name = "Text editor"
fingerprint = "9f2c0a41d3b7e855"    # recorded identity (hex)
```

Names use letters, digits and simple separators. `{param}` placeholders are
filled in when the workflow runs.

### Record and review a run

After a successful top-level run, press **Save as workflow** to request a
proposal. Nothing is saved until you review the complete steps and explicitly
save. Read and meta calls are listed but omitted. Unsupported or missing
mutating calls make the recording ineligible; partial automation cannot be saved.
Runs that used the normal model path are eligible too.

The proposal tracks parameter origins and prior external/private tool output.
Unknown origins are treated conservatively. Review and acknowledge each affected
step. Saved files retain the source run, bot and source-content mark; a replay
inherits that mark and asks for fresh approvals even when a standing grant exists.

Recorded data fields, including destinations, paths, commands, credentials and
all typed text, become required parameters without defaults. This intentionally
favours fresh input over silently reusing a recipient or destination. Such
workflows cannot be selected by fuzzy routing or the model's `workflow_run` tool.
Use the human replay endpoint or CLI, which keep parameter values outside chat
history, run-step inputs, event journals and provider requests:

```text
bot workflow propose <run-id>
bot workflow save reviewed-proposal.json
bot workflow list --bot <bot-id>
bot workflow show <name> --bot <bot-id>
bot workflow run <name> --chat <chat-id> --params-stdin
bot workflow delete <name> --bot <bot-id>
```

For a proposal requiring acknowledgement, edit its top-level `confirmed_steps`
array to contain the zero-based indexes you reviewed before saving with the CLI.
`--params-stdin` reads a JSON object of parameter names and string values from
standard input. Supply it through a private input stream; never put credentials
in shell arguments. Runtime values currently need at least six bytes because the
shared redactor deliberately ignores shorter values. Unsupported short values
are refused. Values containing quotes, newlines and backslashes are accepted.
Values remain only in process memory; after a restart, start a new replay and
supply them again. An interrupted human replay fails without a model handover.

The API requires `bot_id` on workflow list/show/delete, derives the save scope
from the source run, and derives replay scope from the owned `chat_id`. Phone-role
peers have no workflow read, proposal, mutation or replay access.

### Step kinds

| `kind` | Does |
| --- | --- |
| `launch` | Start a desktop application |
| `desktop_action` | Click, type, set a value or press a key chord on a located element in a window |
| `browser_action` | Click, fill, select, press a key or go back, on a located page element |
| `navigate` | Open a URL in the browser |
| `wait_for` | Wait (default 15 s) for a window title, page text, a CSS selector or network idle |
| `shell` | Run a command (through `shell_run`) |
| `tool` | Call any registered tool with a templated input |
| `agent` | An explicit "ask the model" sub-task |

Elements are found by several strategies in order of trust (role and name,
accessible label, CSS selector, position hints) and checked against a recorded
fingerprint so a renamed button is noticed.

### Running a workflow

- Tell the bot: "run the notepad_note workflow with text 'meeting at 3'". It
  calls `workflow_run` with `{"name": "notepad_note", "params": {"text": "..."}}`.
- Type the slash command: `/workflow notepad_note text="meeting at 3"`.
- Put it on a [schedule](scheduled-tasks.md) (through the API / desktop app).

Replay needs no model while the screen still matches the recording. The moment
it does not (an element is renamed or ambiguous, an action fails, a permission
is denied, an `agent` step comes up, a success check fails) replay **stops and
hands over** to the bot, reporting which step it reached, so the bot can carry on
from there. Every step still passes through the permission system, exactly like
a direct tool call; a workflow cannot do anything its bot may not do. The
`workflow_run` tool itself is in the `script` category.

## Remember

After a run **succeeds**, you can mark it as worth remembering. In the desktop
app press **Remember this** under the answer; through the API it is
`POST /v1/runs/{id}/remember`. From then on the same request text is routed
directly to the route that worked (a saved workflow with its parameters, or a
tool or skill hint) without asking the main model to decide.

What it can remember:

- the route the router chose for that request, or
- the saved workflow the model ran with `workflow_run`.

What it refuses: runs that did not succeed, subagent runs, runs that followed no
route and ran no workflow, and bots with the router turned off. If you edit or
delete a workflow, the routes remembered for it are dropped. Remembered routes
expire after a while.

## The decision ladder, briefly

Before the main model sees a message, Praxi tries cheaper levels in order, each
failing open to the main model:

| Level | Mechanism |
| --- | --- |
| K0 | Slash commands and your own regex rules |
| K1 | The remembered routes above |
| K2 | Keyword (BM25) plus, when a Gemini key is configured, embedding search over your workflows, skills and tools |
| K3 | A fast classifier model (`models.classifier`) that answers in strict JSON |
| K4 | The main model with the full loop |

A workflow is chosen automatically only at high confidence (0.9) and only when all
its required parameters are known; otherwise the result is a hint that narrows the
tools the model sees. In the CLI the decision is printed as `[route ...]`. Turn
the whole ladder off per bot with `[router] enabled = false`.

## Planned

- A workflow editor in the desktop app.
