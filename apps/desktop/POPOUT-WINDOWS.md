# Pop-out chat and live-view windows

Issue #233. A pop-out is an independent client of the existing daemon. It subscribes to
that daemon's events and loads its own snapshots; no opener state, shared JavaScript store,
or cross-window localStorage channel carries chats, runs, decisions or credentials.
Composer drafts and queued messages stay in the window where they were written.

The main window alone creates a pop-out through a validated target request. Chat labels
are `chat-<UUID>`; live-view labels are `live-home` and `live-local`. Reopening an existing
label focuses it. A live view can also be retargeted through a native event, including
switching to another paired computer through the existing guarded host channel.

Each kind has a separate capability file. Neither can configure the app, create windows,
start/restart a daemon, execute programs, change providers or claim the main window's
native bridge. Owner-key access follows the existing local-daemon checks; paired hosts
still use their own guarded authority. Navigation stays on app origins and new webview
windows are denied. The main window gains only the explicit open-pop-out command.

Approvals and questions use the existing daemon endpoints: the daemon owns the first
accepted answer. A stale decision is shown as already handled or expired; unrelated errors
stay errors. Chat cancellation reuses the existing Stop retry machinery with an independent,
unpersisted observer ledger. The main window remains the native Stop/heartbeat owner.

Closing a pop-out saves geometry and destroys only that window. It never stops a run or
exits the app. Closing the main window, tray actions and app exit keep their existing policy.
Notifications and single-instance chat activation focus an existing chat pop-out when present.

Native bounds and pin state are stored separately from chat state, capped at 64 entries.
Placement uses monitor work areas in physical coordinates, preserves logical size across DPI
changes, and brings disconnected-monitor bounds back onto a connected display. Dragging
out places the window near the cursor. Pinning affects only the calling pop-out.

Native multi-monitor/DPI movement, tray integration and single-instance activation still
need a manual check on real hardware; pure geometry, target validation, origin checks and
capability tables have regression tests.
