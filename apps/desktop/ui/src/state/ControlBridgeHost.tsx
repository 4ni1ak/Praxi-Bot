import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { botView } from "../lib/botProfile";
import {
  beginAction,
  type ControlSnapshot,
  finishBotRun,
  getControlState,
  isTauriRuntime,
  overlayClick,
  startBotRun,
  subscribeOverlayEvents
} from "../overlay/events";
import type { ComputerTarget } from "../lib/settingsPrefs";
import { DEFAULT_SETTINGS, loadSettings } from "../store/settingsStore";
import { appLedger } from "./appStop";
import { useDaemon } from "./DaemonContext";
import {
  ControlBridge,
  HEARTBEAT_MS,
  resolveTargetScope
} from "./controlBridge";
import { useLiveView } from "./LiveViewContext";
import {
  overrideOptionsFor,
  restoreRuns,
  routeNativeSnapshot,
  stopTransport
} from "./stopHost";

/**
 * Connects the daemon to the on-screen overlay (task 017b) and live-view panel
 * overlay (task 173), and owns the Stop ledger's connections (card 237):
 *
 * - the native subscription lives for the window's lifetime (Stops pressed while
 *   no daemon client exists are still recorded and sent to the next client);
 * - each daemon client is attached to the ledger (pending Stops go first), its
 *   runs are restored, and its bridge is revoked synchronously when it goes away.
 */
export function ControlBridgeHost() {
  const { t } = useTranslation();
  const { client, bots, onDaemonEvent, eventsConnected } = useDaemon();
  const {
    selectedTarget,
    showPanelAction,
    registerBridge,
    ingestControlSnapshot,
    issueSnapshotTicket,
    overrideStop,
    currentTargets
  } = useLiveView();

  const ledger = appLedger();
  const botsRef = useRef(bots);
  botsRef.current = bots;
  const selectedRef = useRef(selectedTarget);
  selectedRef.current = selectedTarget;
  const stepRef = useRef(t("control.starting"));
  stepRef.current = t("control.starting");
  const overrideRef = useRef(overrideStop);
  overrideRef.current = overrideStop;
  const bridgeRef = useRef<ControlBridge | null>(null);
  const nativeRef = useRef<ControlSnapshot | null>(null);
  const resyncRef = useRef<(() => void) | null>(null);

  // Native Stops, for the window's lifetime: subscribe first, then read.
  useEffect(() => {
    if (!isTauriRuntime()) {
      return undefined;
    }
    const route = (snap: ControlSnapshot) => {
      nativeRef.current = snap;
      routeNativeSnapshot(ledger, bridgeRef.current, snap);
    };
    const off = subscribeOverlayEvents({ onControlStateChanged: route });
    void getControlState().then(route);
    return off;
  }, [ledger]);

  // A Stop pauses a held screen takeover too (same decision, every entry point).
  useEffect(
    () =>
      ledger.onStop((rec) => {
        void overrideRef.current(overrideOptionsFor(rec));
      }),
    [ledger]
  );

  // The event stream reconnected: wait for (and fetch) a fresh run list.
  useEffect(() => {
    if (eventsConnected) {
      resyncRef.current?.();
    } else {
      const handle = ledger.client();
      if (handle) {
        ledger.markUnsynced(handle);
      }
    }
  }, [eventsConnected, ledger]);

  useEffect(() => {
    if (!client) {
      registerBridge(null);
      return undefined;
    }
    // Pending Stops are sent to this client before anything else runs on it.
    const handle = ledger.attachClient(stopTransport(client));
    let bridge: ControlBridge | null = null;
    let offEvents = () => {};

    if (isTauriRuntime()) {
      let computerTarget: ComputerTarget = DEFAULT_SETTINGS.computerTarget;
      let settingsLoaded = false;
      const loadComputerTarget = () => {
        void loadSettings()
          .then((s) => {
            if (handle.live) {
              computerTarget = s.computerTarget;
              settingsLoaded = true;
            }
          })
          .catch(() => {});
      };
      loadComputerTarget();
      const offSettings = subscribeOverlayEvents({ onSettingsUpdated: loadComputerTarget });
      const created = new ControlBridge({
        client,
        native: {
          beginAction: (rect, label, action) => beginAction(rect, label, action),
          startRun: async (bot, step) => (await startBotRun(bot, step)).runGeneration,
          finishRun: async (generation) => {
            routeNativeSnapshot(ledger, bridgeRef.current, await finishBotRun(generation));
          },
          click: overlayClick
        },
        ledger,
        // Unknown until THIS client's targets and the settings loaded: never guessed.
        resolveTargetScope: ({ runId, botId }) => {
          const loaded = currentTargets();
          if (!settingsLoaded || !loaded || loaded.client !== client) {
            return "unknown";
          }
          return resolveTargetScope({
            runId,
            botId,
            targets: loaded.targets,
            selectedTarget: selectedRef.current,
            computerTarget
          });
        },
        onSnapshot: ingestControlSnapshot,
        issueSnapshotTicket,
        panelOverlay: {
          showAction: showPanelAction
        },
        botMeta: (id) => {
          const bot = botsRef.current.find((b) => b.id === id);
          if (!bot) {
            return { botId: id, botName: id.slice(0, 8) };
          }
          const v = botView(bot);
          return {
            botId: v.id,
            botName: v.name,
            botShape: v.shape,
            botAvatar: v.avatar,
            botColor: v.color
          };
        },
        get startingStep() {
          return stepRef.current;
        },
        now: () => Date.now()
      });
      bridge = created;
      bridgeRef.current = created;
      registerBridge(bridge);
      offEvents = onDaemonEvent((env) => void created.handleEvent(env));
      const known = nativeRef.current;
      if (known) {
        void created.onNativeState(known.mode, known.runGeneration);
      } else {
        void created.heartbeat();
      }
      const previousOff = offEvents;
      offEvents = () => {
        previousOff();
        offSettings();
      };
    } else {
      registerBridge(null);
      // Without the desktop shell the ledger still follows runs for Stop.
      offEvents = onDaemonEvent((env) => {
        if (env.event.type === "run_started") {
          ledger.observeRun(env.event.run_id, env.event.bot_id);
        } else if (env.event.type === "run_finished") {
          ledger.observeRunFinished(env.event.run_id);
        }
      });
    }

    const resync = () => {
      if (handle.live) {
        void restoreRuns(ledger, handle, client, bridge);
      }
    };
    resyncRef.current = resync;
    resync();
    const beat = window.setInterval(() => {
      if (bridge) {
        void bridge.heartbeat();
        // events are notifications only and never replayed: re-read the snapshot
        void bridge.refreshControlState();
      }
      if (!handle.synced) {
        resync();
      }
    }, HEARTBEAT_MS);

    return () => {
      // Revoke first, synchronously: nothing of this client acts after cleanup.
      bridge?.revoke();
      ledger.detachClient(handle);
      if (bridgeRef.current === bridge) {
        bridgeRef.current = null;
      }
      if (resyncRef.current === resync) {
        resyncRef.current = null;
      }
      registerBridge(null);
      offEvents();
      window.clearInterval(beat);
    };
  }, [client, ledger, onDaemonEvent, registerBridge, showPanelAction, ingestControlSnapshot, issueSnapshotTicket, currentTargets]);

  return null;
}
