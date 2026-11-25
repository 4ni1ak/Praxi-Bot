# Email integration

Praxi includes IMAP and SMTP tools that allow your bots to read, search, draft, and send e-mails.

## How to enable email

Email support requires two things:
1. Store your mailbox password (or app-specific password) in the Praxi secret store.
2. Configure your email account in `<data_dir>/email.toml` (or under the `[email]` section of `providers.toml`).

### 1. Store your password

Mail passwords are never stored in plain text configuration files. They are referenced by name via `SecretRef` and retrieved from the secret store only when connecting:

```sh
bot secrets set email/work/password
```

Type or paste your password when prompted.

### 2. Configure accounts

Create or edit `email.toml` in your Praxi data directory (the default per-user data directory, or the path passed via `--data-dir` / `BOT_DATA_DIR`):

```toml
[[accounts]]
name = "work"
address = "alice@example.com"
username = "alice@example.com"      # optional: defaults to address
password = "email/work/password"   # SecretRef handle stored in step 1

[accounts.imap]
host = "imap.example.com"
port = 993                        # default: 993 (implicit TLS)

[accounts.smtp]
host = "smtp.example.com"
port = 465                        # default: 465 (implicit TLS) or 587 (STARTTLS)
security = "implicit"             # "implicit" or "start_tls"
```

You can configure multiple accounts with distinct `name` identifiers.

Alternatively, you can place an `[email]` section directly inside `<data_dir>/providers.toml`:

```toml
[email]
[[email.accounts]]
name = "work"
address = "alice@example.com"
password = "email/work/password"

[email.accounts.imap]
host = "imap.example.com"

[email.accounts.smtp]
host = "smtp.example.com"
```

### 3. Start the daemon

Start the daemon with `bot serve`:

```sh
bot serve
```

When the secret store is available, the daemon registers the email tools once and prints, for example:
```
email: 6 tool(s) registered (1 account(s))
```

A bot is only offered the email tools when at least one account is usable by it
(see below), so registering them with no account configured does nothing visible.

## Managing accounts without editing files

With the daemon running you can add, change, test and remove accounts from
**Settings, Mail** in the desktop app, or from the command line:

```sh
bot email add work alice@gmail.com --preset gmail --bot Atlas --password-env MAIL_PW
bot email list
bot email test work        # IMAP login + SMTP auth, nothing is sent
bot email rm work
```

A new mailbox has no default owner: give `--bot <name-or-id>` (only that bot can
use it) or `--all-bots` (every bot that has the email tools can then read it
without asking). The app asks the same: its form starts on "Only one bot" with
no bot picked, and choosing "Any bot" shows a warning.

Presets exist for `gmail`, `outlook`, `yahoo` and `icloud`; for anything else give
`--imap-host`, `--smtp-host` and, if needed, `--imap-port`, `--smtp-port`,
`--smtp-security start_tls`. The password is read from the variable named by
`--password-env` or from a hidden prompt, never from a flag. Most providers need
an app password.

Changes are written to `email.toml` and take effect for the next tool call; no
restart is needed. The password goes to the secret store as
`email/<account>/password` and is never returned by the API, the CLI or the app
(the account list only says whether one is stored). Changing an account's
servers or login requires entering the password again, so a stored password can
not be redirected to another server.

The bot panel (right side, "Library" tab) has a **Mail** block for the selected
bot: the mailboxes it may use, a "Check connection" button with a next step when
a check fails, and a switch "Let <bot> use mail". The switch adds or removes
`email_*` in the bot's `[tools] allow` / `exclude` lists (the opt-in described in
[Bots and profiles](bots-and-profiles.md)); it touches nothing else, and sending
still follows the bot's permission mode.

Each mailbox in that block can also **show recent mail** (the newest ten
headers: subject, sender, date; read-only, nothing is marked read and no body is
fetched) and **send a test mail**. The test mail is one fixed message from the
mailbox to its own address; it cannot be addressed to anyone else. Both use
`GET /v1/email/accounts/{name}/messages` and
`POST /v1/email/accounts/{name}/send-test`, which need the owner key (the desktop
app has it; a bare API token does not). On a paired host only a desktop may read the
headers, and the test mail is sent from the host itself.

### One mailbox per bot

An account can belong to one bot:

```toml
[[accounts]]
name = "atlas"
bot = "6f1c2a4e-...-uuid-of-the-bot"      # the bot's id (`bot bots list`)
allowed_senders = ["alice@example.com", "@corp.example"]   # who may reach a bot through the e-mail channel
```

- A bound account is usable only by that bot, and that bot's email tools use it
  by default when no `account` is given. Any other bot asking for it by name gets
  the same "unknown account" error as for a name that does not exist.
- Accounts without `bot` keep working as before for every bot that has the
  email tools enabled. Bots do not get the email tools by default: add `email_*`
  to the bot's allowed tools.
- Deleting a bot **removes** the accounts bound to it, together with the
  passwords the daemon stored for them. They are never turned into shared
  accounts, which would give every bot access to that mailbox. If a stored
  password cannot be deleted at that moment, its name is kept in
  `email-secret-cleanup.json` and deleted on the next start.
- Bundle exports with "all secrets" leave out `email/<account>/password`
  secrets; a mailbox password is exported only when you name it.
- `allowed_senders` decides whose mail the e-mail channel may pass to a bot. The
  list is empty by default and an empty list passes **nothing** on (there is no
  "any sender"): name at least your own address. Mail from anyone else is
  ignored, counted and logged without its text. A mail that cannot be handed to
  the bot is retried three times and then skipped (counted), so it cannot block
  later mails. Mails above `max_per_poll` stay unread and come at the next check.
- The "Test connection" action logs in over TLS exactly as configured and
  reports one status per server (`ok`, `unreachable`, `tls`, `auth_failed`,
  `protocol`, `timeout`); it never sends or reads mail and never shows what the
  server said.

## Available tools

Once enabled, six tools are registered under the `Messaging` permission category:

| Tool | Effect | Description |
| --- | --- | --- |
| `email_list_folders` | Read | List folders and attributes via IMAP LIST |
| `email_search` | Read | Search messages with sender/subject/date/unread filters; returns UID summaries |
| `email_read` | Read | Read message content and attachment metadata by UID using BODY.PEEK |
| `email_mark_read` | Write | Set or clear the Seen flag on a message |
| `email_draft` | Write | Save a message as a draft in the drafts folder (never contacts SMTP) |
| `email_send` | Destructive | Send an e-mail immediately over SMTP (**requires human approval**) |

## Approvals and security

- **Sending requires approval**: `email_send` is an external effect marked `Effect::Destructive`. It sends an e-mail to external recipients and cannot be undone. Under standard permission presets (`approve-destructive`, `read-and-draft`), the engine always prompts for explicit approval before dispatching mail.
- **Mandatory TLS**: IMAP requires implicit TLS (port 993). SMTP requires implicit TLS (port 465) or mandatory STARTTLS (port 587). Hostname and certificate validation against Mozilla roots are always enforced; plaintext connections and insecure certificates are rejected.
- **Safe credentials**: Passwords are never echoed or written to logs; credentials resolve lazily from the secret store when establishing connections.
- **Drafts**: `email_draft` saves directly to your IMAP drafts folder and never contacts SMTP.

## Safe sending (send log, opt-out list, limits)

These guards live in code, not in the model. Settings > Mail > **Safe sending** shows and edits them; the routes (`/v1/email/guard/*`) need the owner key.

- **Send log**: every e-mail that was really sent is recorded (time, bot, run, account, recipients, subject start, message id, whether the opt-out line was added, whether you chose "Send anyway"). You can open it in the app and export it as CSV. It is stored in `<data dir>/email-guard/send-log.jsonl`.
- **Opt-out list**: addresses or whole domains you will not write to (`email-guard/opt-out.json`).
- **Opt-out line**: the app appends it to the body of every `email_send` (default text in Turkish and English, editable, `{sender}` = the sending address). The bot does not write it and cannot remove it. A bot may ask to leave it out (`opt_out_line: false`); that only makes the approval card show a warning.
- **Limits**: a warning above 20 e-mails per 24 hours per account, and when someone got mail from the account in the last 14 days. Both numbers are settings (0 turns a warning off).
- **Warnings are not blocks.** When a send hits the opt-out list, a limit or the missing opt-out line, the approval card lists the warnings (and once, the note that ignoring an opt-out on commercial mail can be unlawful, in Turkey under Law 6563, ETK) and its button reads **Send anyway**. Only you can click it; it is logged. A model cannot override a warning: the tool accepts a send past a warning only when it matches, exactly, a card that was shown to you, once.
- `email_send` always asks, whatever the permission preset or grants.

## Give a bot its own address (AgentMail)

Besides your own mailbox, a bot can have an address such as `name@agentmail.to`,
created at [AgentMail](https://agentmail.to) with **your own** AgentMail account
and API key. Praxi-Bot holds no key of its own and runs no relay.

- In **Settings, Mail** choose "Give this bot an address (AgentMail)",
  paste your API key, pick the bot (or any bot, explicitly) and, optionally, a name
  for the address. From the command line:
  `bot email add --provider agentmail --name helper --bot Atlas --api-key-env AGENTMAIL_KEY`.
- The key is write-only: it goes to the secret store as `email/<account>/api_key`
  (excluded from state bundles unless you name it) and is only ever sent, in the
  `Authorization` header, to `api.agentmail.to` over HTTPS. Redirects are never followed.
- `email.toml` stores `kind = "agentmail"` and the `inbox_id`; older files without a
  `kind` are IMAP/SMTP accounts, as before.
- The same `email_*` tools work. For an AgentMail account messages are identified by
  `message_id` (shown by `email_search`) instead of an IMAP `uid`; folders and drafts are
  not supported and say so.
- Removing an account (or deleting a bot) only forgets it locally. The inbox at AgentMail is
  deleted only when you ask: `bot email rm <name> --delete-inbox`, or the checkbox in the app.
- At the time of writing the free plan allows 3 inboxes, 3,000 e-mails a month and 100 a day;
  see their [pricing](https://agentmail.to/pricing). Cold outreach is prohibited on the shared domain.
