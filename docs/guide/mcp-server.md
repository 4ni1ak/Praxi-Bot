# Use Praxi as an MCP server

`bot mcp serve` exposes one selected Praxi bot to an external MCP client over
stdin/stdout. Keep the ordinary Praxi daemon running. The MCP process connects
to its loopback HTTP facade with a scoped credential stored in the secret store;
it does not read the daemon's `api-token` file.

## Create a credential and connect

Find the bot's storage UUID with `bot bots list`, then run these commands as the
local owner. Replace the example UUID with your bot's actual UUID.

```sh
bot token create --scope mcp:tasks --bot 00000000-0000-0000-0000-000000000001 --ttl 30d
bot mcp serve --bot 00000000-0000-0000-0000-000000000001
```

The first command prints a public credential `id`, allowed bots and scopes, and
`expires_at_ms`. It stores the bearer internally and never prints its value.
The second command speaks MCP on stdout; it is intended to be launched by your
MCP client, rather than used as an interactive terminal.

When more than one active credential matches the selected bot and layers, pass
the public id explicitly:

```sh
bot mcp serve --bot 00000000-0000-0000-0000-000000000001 --token-id 00000000-0000-0000-0000-000000000002
```

For example, the owner's manual Claude Code check can register the stdio command:

```sh
claude mcp add praxi -- bot mcp serve --bot 00000000-0000-0000-0000-000000000001 --token-id 00000000-0000-0000-0000-000000000002
```

Use the same data directory and secret backend as the daemon. The OS keychain
must be accessible to the launching process. An existing encrypted file can use
the established `BOT_SECRETS_PASSPHRASE` environment setting. MCP mode never
prompts for that passphrase on stdin. Avoid putting credentials in client config
files; client configuration needs only the public bot and token ids.

`--url` accepts an HTTP loopback origin only. Redirects and HTTP proxies are
disabled for this connection. `--remote` is not supported. There is no inbound
MCP HTTP listener in this feature.

## Task tools

The default layer is `--allow tasks`, requiring scope `mcp:tasks`.

| Tool | Arguments | Result |
| --- | --- | --- |
| `bot_list` | `{}` | The selected bot's id and name |
| `task_start` | `{ "text": "...", "bot": "optional selected UUID" }` | New run id |
| `task_status` | `{ "run": "UUID" }` | State and external-agent attribution |
| `task_result` | `{ "run": "UUID" }` | Readiness, bounded redacted text and error flag |
| `task_cancel` | `{ "run": "UUID" }` | Cancellation acknowledgment |
| `schedule_list` | `{}` | This bot's schedule ids, names and enabled states |

Every task starts in its own fresh chat. Task start returns immediately; use
status/result while the bot works or waits for approval. A credential can inspect
and cancel only runs it created for the selected bot. Schedule prompts and
arbitrary chat history are not exposed by these tools.

The bot uses its configured permissions. External task text is persisted with
role `external_agent`, then presented to the model in an explicitly untrusted
data block. It cannot override bot policy. Automatic memory recall and cached
command/workflow routing are skipped. The task chat cannot accept follow-up
messages or become a later human chat; start a separate task or chat instead.

## Computer tools

Computer access requires both the credential scope and an explicit server layer:

```sh
bot token create --scope mcp:tasks,mcp:computer --bot 00000000-0000-0000-0000-000000000001 --ttl 1d
bot mcp serve --bot 00000000-0000-0000-0000-000000000001 --allow tasks,computer --token-id 00000000-0000-0000-0000-000000000002
```

`--allow computer` also works for a computer-only credential. The server lists
the selected bot's available `browser_*` and `desktop_*` tools from the existing
registry. `browser_eval` and shell tools are never exported by this layer.
The exported names are explicitly reviewed; adding a new native computer tool
requires updating this allowlist, including any future batch-action tool.

Each computer call starts a normal attributed tool run, using the same permission
engine, approval cards, computer target, control gate, timeout and audit path as
other Praxi calls. The MCP request waits for completion or approval for at most
ten minutes, then cancels the run and returns an error. A refusal also becomes an
MCP error result. No MCP operation can approve an action, release control, change
policy or change the bot's configuration.

## Approvals and authority

Every external-agent tool call requires **Approve once**, including reads and
Meta tools. Existing grants, the read shortcut and a full bot policy cannot waive
this floor. A deny remains a deny. Approve in Praxi's existing UI, CLI or Telegram
approval flow. Persistent scopes such as Always, Always category and This chat
are rejected for this initiator.

This is deliberately stricter than ordinary Praxi runs. The independent native
egress/taint work is not present on this feature's base, so this version does not
claim to implement that separate policy. It uses a persistent external source
and per-call approval instead. Both resumed runs and already persisted children
retain that source. New subagent spawning is denied so the credential's run cap
cannot be exceeded through fan-out. Native `cli:` model sessions, including
fallbacks and child model roles, are refused because they can operate tools
outside the host's per-call gate. Use an API-backed model for MCP tasks.

These are application permissions. A program running as the same OS user is
outside that boundary and may already have filesystem or keychain access.

## Limits, revocation and storage

Each credential allows 30 tool calls per rolling minute and two active runs,
including paused runs and persisted descendants. A separate owner-created
credential has its own limits. The rate window is in memory and resets on daemon
restart; active-run admission is reconstructed from stored runs. Daemon payloads
are bounded to 64 KiB of JSON and passed through the production redactor. MCP
adds its bounded text wrapper and protocol framing. Results
are labeled as untrusted data. Inputs are limited to 32 KiB, with a 64 KiB MCP
frame limit before JSON-RPC decoding.

Credential lifetimes range from 60 seconds to 30 days. Expiry is checked on each
facade request and while computer calls wait. Revoke a credential with its public
id:

```sh
bot token revoke 00000000-0000-0000-0000-000000000002
```

Revocation invalidates the grant before cancelling outstanding tasks and their
descendants. It does not undo actions already approved and completed. Scoped
tokens can use only `GET /v1/mcp-server/tools` and
`POST /v1/mcp-server/call`; owner policy, secret, provider, MCP configuration and
approval endpoints reject them. Every paired peer role is refused on all four
MCP facade and credential-management routes.

The reserved `mcp-server/` secret-store namespace contains the bearer and grant.
It cannot be selected as a provider or connector secret, modified through generic
secret commands, exported in a state bundle, or imported from one. A moved
installation needs newly issued MCP credentials. Audit entries contain operation
labels and public ids only, and a failed audit write prevents the action.

## Troubleshooting

“No matching scoped token” means that no live credential permits this bot and
the requested layers. Create a matching credential as the owner, or select its
id. “Multiple matching scoped tokens” requires `--token-id`. An expired or
revoked credential cannot reconnect.

If a call is waiting, review the approval in Praxi. If a bot uses a native CLI
model, select an API-backed model. If the two-run cap is reached, complete or
cancel existing tasks. A 429 response is returned as an MCP tool error; wait for
the rolling rate window before retrying.
