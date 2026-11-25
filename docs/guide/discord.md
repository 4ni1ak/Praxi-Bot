# Discord

Talk to your bots from Discord, in a private chat with your own bot account or
in one server channel you allow. Only **your** Discord account can use it:
everyone else is ignored (in a private chat they get one polite line).

## Connect it (about 5 minutes)

```sh
bot channels discord setup     # explains the steps, asks for the token (hidden), then tests it
bot serve                      # the channel starts with the daemon
bot channels discord pair      # prints a one-time 6-digit code (valid 10 minutes)
```

1. Open <https://discord.com/developers/applications> and press **New Application**.
2. Open **Bot**, press **Reset Token** and copy the token.
3. On the same page, under **Privileged Gateway Intents**, switch **Message Content Intent** on. Without it Discord sends the bot empty messages.
4. Run `bot channels discord setup` and paste the token. It goes into your secret store and is never shown again.
5. The setup ends with a connection test in plain words and an invite link. You only need the link if you want to use a server channel.
6. In Discord, open a **private message** with your bot and send `!pair 123456` (your code).

Use `bot channels discord setup --lang tr` or `--lang de` for Turkish or German setup steps and default replies. You can change replies later with `!lang`.

`bot channels discord test` repeats the connection test at any time. You can also
test from the app (Connection test, channel `discord`).

## Who can talk to the bot

- Pairing only works in a private message with the bot, and only with a code you created on your computer. A code typed into a server does nothing; create a new one.
- The paired account is the **owner**. Nobody else is ever served, not even in a server channel you allowed. A new valid code from another account replaces the owner and forgets the allowed server channels.
- Bots and other apps are ignored.
- Wrong codes are refused and, after a few, locked for a while.

## In the chat

Commands start with `!` (a `/` also works if you send it as plain text).

| Message | Does |
| --- | --- |
| `!bots` | Lists your bots |
| `!use <name>` | Chooses the bot this chat talks to |
| `!stop` | Stops the current task |
| `!status` | Shows how many tasks run and wait for your OK |
| `!lang en`, `!lang tr`, `!lang de` | Language of the bot's own messages |
| `!allow` / `!disallow` | In a server channel: let the bot answer you there / stop (owner only) |
| `!help` | Short help |
| anything else | Sent to the selected bot |

In an allowed server channel **every** message you write goes to the bot, so pick
a channel that is only for this.

## Approvals

When a bot needs your OK you get a message with **Approve once** and **Deny**
buttons. Only you can press them, only in the chat where the card appeared,
once, within 10 minutes.

- Buttons appear only for low-risk requests whose full text fits in the message. Everything else (risky actions, requests with hidden secrets, very long requests) says so and waits for the app on your computer.
- There is no "always allow" here. Standing permissions are decided in the app.

## Messages and attachments

- Long answers are split into several messages and updated about once a second. The last unfinished word stays hidden until it is safe to redact.
- The bot never pings anyone: `@everyone`, `@here` and user or role mentions in an answer are made harmless.
- Attachments are **not downloaded or opened**. The bot only learns that a file with a certain name and type was sent, marked as untrusted. Send the text instead.
- Everything sent to Discord goes through the secret redactor, which hides secrets it knows, including the bot token.
- The bot stays within Discord's rate limits and waits when Discord asks it to.

## Good to know

- You create your own Discord application, so the token is yours. The bot connects out from your computer; no open port is needed.
- Discord messages travel through Discord's servers. Do not use it for anything you would not send through Discord.
- `bot channels discord status` shows whether the channel is configured, enabled and running and whether an owner is paired. `bot channels discord disable` turns it off at the next `bot serve`; the token stays stored.
- If the bot says nothing: check `bot channels discord test` (token and Message Content Intent), and that `bot serve` was restarted after setup.
