# When something happens

Open a bot's panel, choose **Automation**, then **When something happens**.
Add a rule for new mail, a signed incoming call, or a time schedule. Write what
the bot should do, such as “Prepare a reply for me to review.” Save it, review
the rule, and turn it on. New rules always start off.

Existing mail schedules, prompt schedules and signed addresses appear in this same list. Open one to add quiet hours and limits; saving pauses it until you turn the reviewed rule on. Advanced workflow, precheck and file-watch schedules remain in Schedules.

For mail, connect an IMAP mailbox first and allow the bot's scheduled mailbox
reads. Choose a folder, sender (an exact address, or a domain that also covers its subdomains; never a
part of the text), and optional
subject or attachment filter. Checks run every two minutes by default, never
more often than once a minute. The first check establishes a starting point;
old mail does not start runs. Bursts become one run with a list of headers.
The connector's newest-100 search window is not a guaranteed delivery queue.
AgentMail cannot currently supply the complete loop-guard headers this feature
requires, so the builder offers IMAP mailboxes only. Attachment matching requires
the server to return MIME structure metadata; unknown metadata never matches.

Mail from the mailbox's own address, automatic replies, bulk/list mail and
mail with a list-unsubscribe header are ignored. No-reply addresses are ignored
unless the sender filter names them. Sender filters do not authenticate senders.
The existing e-mail conversation channel is independent: enabling both for the
same mailbox can produce both a conversation and a rule run.

For an incoming call, create a signed address under **Signed addresses**, then
select it in the rule. Signature, replay and incoming-call limits still apply.
Once linked, that address queues events for this rule instead of starting its
original run. The rule's instruction takes over. Turning off a linked rule
does not restore the original webhook instruction; calls are refused until it
is enabled or the rule is removed.

Quiet hours use the time zone you choose, including daylight-saving changes.
You can skip events or save them for one summary after the window. By default
a rule may start 10 runs per rolling hour and 50 per rolling day; its bot may
start 30 and 150 across all rules. The lowest configured bot ceiling applies
to every rule for that bot. Extra events wait and join the next summary. These
queues and counters survive a restart. “Today” counts use UTC.

Queues hold up to 100 events and 48 KB per rule. A full mail queue or a failed
run pauses the rule. Ten consecutive mailbox-check failures also pause it.
Check the activity, connection and filters before turning it on again. Editing
a rule discards its old queued events and restarts mailbox baselining; rate
counters are preserved. Delivery uses the scheduler's existing best-effort
crash boundary: a crash after a batch is reserved can lose that batch. It is
not exactly-once delivery. Keep critical mail in your mailbox.

**Test with a sample** shows the exact instruction and wrapped event text the
bot receives, with the same size cap. It uses no model and runs no tools.
Mail supplies headers only; message bodies and attachments are never fetched
by the rule. Incoming call text is limited to a 4,096-character snippet and
the final event block to 12 KB. Invisible characters are removed from headers.
Event text is untrusted data, even if it asks the bot to ignore your instruction.

Only the local owner can create, edit or enable these rules. The bot uses its
own permissions and normal approvals; external-event runs also keep the
scheduler's once-only approval floor. An action needing approval waits and
notifies you. Rules do not auto-approve anything. Credentials stay in the
mailbox or signed-address configuration, never in the rule.
