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
  resolveTargetScope
} from "./controlBridge";
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
  const selectedRef = useRef(selectedTarget);
  selectedRef.current = selectedTarget;
  const computerTargetRef = useRef<ComputerTarget>(DEFAULT_SETTINGS.computerTarget);
  const stepRef = useRef(t("control.starting"));
  stepRef.current = t("control.starting");
  // Survive a bridge swap (daemon client replaced): ids a Stop already cancelled,
  // and the retired bridge's queued cancels, which the next bridge waits for.
  const cancelledRef = useRef(new Set<string>());
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
          }
        })
        .catch(() => {});
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
      cancelled: cancelledRef.current,
      targetsLoaded: () => targetsRef.current.length > 0,
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
    // Native events that land before the initial snapshot are replayed after it,
    // so each one is judged against the snapshot's `run_generation` (a Stop is an
    // idle with a HIGHER generation; a same-generation idle never is).
    const buffered: { mode: ControlMode; generation: number }[] = [];
    let nativeReady = false;
    const offState = subscribeOverlayEvents({
      onControlStateChanged: (snap) => {
        const mode = snap.mode as ControlMode;
        if (!nativeReady) {
          buffered.push({ mode, generation: snap.runGeneration });
          if (buffered.length > 50) buffered.shift();
          return;
        }
        void bridge.onNativeState(mode, snap.runGeneration);
      },
      onSettingsUpdated: refreshComputerTarget
    });
    // Both subscriptions are installed before either initial snapshot starts.
    let disposed = false;
    let initializing = false;
    const tick = async () => {
      if (disposed || initializing) return;
      if (!nativeReady) {
        initializing = true;
        try {
          const snap = await getControlState();
          if (disposed) return;
          await bridge.initializeNativeState(snap.mode, snap.runGeneration);
          nativeReady = true;
          for (const ev of buffered.splice(0)) {
            void bridge.onNativeState(ev.mode, ev.generation);
          }
        } catch {
          // Retry initialization on the next heartbeat without changing control.
        } finally {
          initializing = false;
        }
      }
      if (disposed || !nativeReady) return;
      await retiringRef.current; // a Stop on the previous bridge issues its cancels first
      if (disposed) return;
      void bridge.restoreRuns();
      void bridge.retryDeferredStarts();
      await bridge.heartbeat();
      // events are notifications only and never replayed: re-read the snapshot
      void bridge.refreshControlState();
    };
    void tick();
    const beat = window.setInterval(() => void tick(), HEARTBEAT_MS);
    return () => {
      disposed = true;
      alive = false;
      registerBridge(null);
      bridge.dispose();
      retiringRef.current = bridge.drain();
      offEvents();
      offState();
      window.clearInterval(beat);
    };
  }, [client, onDaemonEvent, registerBridge, showPanelAction, ingestControlSnapshot, issueSnapshotTicket]);

  return null;
}
