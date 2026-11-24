# Stop cancels the run

Pressing Stop in the chat composer cancels the run that is actually in
flight. The run leaves `running` within about a second. It does not stay
`running` after Stop.

## What Stop does

- The daemon cancel route signals the run. The agent loop also arms a
  supervisor: if the run is still not finished one second later, the
  supervisor aborts the drive task and marks the run `cancelled`. That covers
  a tool that blocks the worker without ever polling the cancel token.
- The in-flight model request is aborted. A retry or a fallback model is not
  started once Stop has been pressed. Dropping the stream drops the request.
- The in-flight tool is dropped. A shell the run started is killed when its
  task is dropped. A browser the run launched is killed when that child is
  dropped. A page load that is dropped mid-wait sends `Page.stopLoading`. A
  load that merely times out is left alone.
- An approval the run was waiting on is stored as `denied` and closed with a
  reason that does not start with `expired`. The owner said no. A timeout, and
  a schedule run that pauses because nobody answered, still use `expired`.
- Messages the user already queued stay queued. Stop does not start them and
  does not delete them. The chat is free for a later send. The composer does
  not ask "Send queued messages now?".
- The activity line says "Stopped by you".

## What the composer shows

- The round button switches to Stopping as soon as Stop is pressed.
- A second press while it says Stopping is the hard stop.
- When the event stream has finished the run, the button is Send again, even
  if the last run list still says `queued`, `running`, or `waiting_approval`.
- Text in the box still makes the button Send, so the user can queue another
  message. Queued bubbles do not hide Stop while the box is empty.
