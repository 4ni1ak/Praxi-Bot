import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { botView } from "../lib/botProfile";
import {
  beginAction,
  type ControlMode,
  getControlState,
  isTauriRuntime,
  overlayClick,
  startBotRun,
  stopBotRun,
  subscribeOverlayEvents
} from "../overlay/events";
import type { ComputerTarget } from "../lib/settingsPrefs";
import { DEFAULT_SETTINGS, loadSettings } from "../store/settingsStore";
import { useDaemon } from "./DaemonContext";
import {
  ControlBridge,
  HEARTBEAT_MS,
  resolveTargetScope,
  StopLedger
} from "./controlBridge";
import { BridgeSession } from "./bridgeSession";
import { useLiveView } from "./LiveViewContext";

/**
 * Connects the daemon to the on-screen overlay (task 017b) and live-view
 * panel overlay (task 173). Renders nothing; registers native bridge hooks
 * only in the Tauri desktop runtime.
 */
export function ControlBridgeHost() {
  const { t } = useTranslation();
  const { client, bots, onDaemonEvent } = useDaemon();
  const {
    targets,
    targetsSettled,
    selectedTarget,
    showPanelAction,
    registerBridge,
    ingestControlSnapshot,
    issueSnapshotTicket
  } = useLiveView();

  const botsRef = useRef(bots);
  botsRef.current = bots;
  const targetsRef = useRef(targets);
  targetsRef.current = targets;
  const targetsSettledRef = useRef(targetsSettled);
  targetsSettledRef.current = targetsSettled;
  const settingsLoadedRef = useRef(false);
  const selectedRef = useRef(selectedTarget);
  selectedRef.current = selectedTarget;
  const computerTargetRef = useRef<ComputerTarget>(DEFAULT_SETTINGS.computerTarget);
  const stepRef = useRef(t("control.starting"));
  stepRef.current = t("control.starting");
  // Survive a bridge swap (daemon client replaced): ids a Stop already cancelled,
  // and the retired bridge's queued cancels, which the next bridge waits for.
  const ledgerRef = useRef(new StopLedger());
  const retiringRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    if (!client || !isTauriRuntime()) {
      registerBridge(null);
      return undefined;
    }
    let alive = true;
    const refreshComputerTarget = () => {
      void loadSettings()
        .then((s) => {
          if (alive) {
            computerTargetRef.current = s.computerTarget;
            settingsLoadedRef.current = true;
          }
        })
        .catch(() => {
          settingsLoadedRef.current = true; // settled: defaults apply
        });
    };
    refreshComputerTarget();
    const bridge = new ControlBridge({
      client,
      native: {
        beginAction: (rect, label, action) => beginAction(rect, label, action),
        startRun: async (bot, step) => {
          await startBotRun(bot, step);
        },
        stopRun: async () => {
          return (await stopBotRun()).runGeneration;
        },
        click: overlayClick
      },
      resolveTargetScope: ({ runId, botId }) =>
        resolveTargetScope({
          runId,
          botId,
          targets: targetsRef.current,
          selectedTarget: selectedRef.current,
          computerTarget: computerTargetRef.current
        }),
      ledger: ledgerRef.current,
      // Scope is never guessed: wait until targets AND the computer-target setting settled.
      targetsLoaded: () => targetsSettledRef.current && settingsLoadedRef.current,
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
    registerBridge(bridge);
    const offEvents = onDaemonEvent((env) => void bridge.handleEvent(env));
    // The session owns the start-up protocol (buffering native events behind the
    // initial snapshot, flushing on tear-down, waiting for the retired bridge).
    const session = new BridgeSession(bridge, {
      getNativeSnapshot: () => getControlState(),
      retiring: retiringRef.current,
      ledger: ledgerRef.current
    });
    const offState = subscribeOverlayEvents({
      onControlStateChanged: (snap) =>
        session.onNativeEvent(snap.mode as ControlMode, snap.runGeneration),
      onSettingsUpdated: refreshComputerTarget
    });
    void session.tick();
    const beat = window.setInterval(() => void session.tick(), HEARTBEAT_MS);
    return () => {
      alive = false;
      registerBridge(null);
      session.dispose();
      retiringRef.current = session.retiredDone;
      offEvents();
      offState();
      window.clearInterval(beat);
    };
  }, [client, onDaemonEvent, registerBridge, showPanelAction, ingestControlSnapshot, issueSnapshotTicket]);

  return null;
}
