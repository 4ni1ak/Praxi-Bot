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
  planNativeReplay,
  resolveTargetScope,
  waitBounded
} from "./controlBridge";
import { useLiveView } from "./LiveViewContext";

/** Longest the next bridge waits for the retired bridge's queued cancels. */
const RETIRING_WAIT_MS = 3000;

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
  const cancelledRef = useRef(new Set<string>());
  const retiringRef = useRef<Promise<void>>(Promise.resolve());
  const carryRef = useRef<{ stopAt: number | null }>({ stopAt: null });

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
      cancelled: cancelledRef.current,
      carry: carryRef.current,
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
    // Judge the buffered native events against the snapshot (see planNativeReplay)
    // and hand them to the bridge: events older than the snapshot are dropped, the
    // generation walk starts at the lowest buffered generation.
    const applyInitial = async (snap: { mode: string; runGeneration: number }) => {
      const plan = planNativeReplay(
        { mode: snap.mode as ControlMode, generation: snap.runGeneration },
        buffered
      );
      buffered.length = 0;
      await bridge.initializeNativeState(snap.mode as ControlMode, plan.baseline);
      nativeReady = true;
      for (const ev of plan.replay) {
        void bridge.onNativeState(ev.mode, ev.generation);
      }
    };
    const retire = () => {
      bridge.dispose();
      retiringRef.current = bridge.drain();
    };
    const tick = async () => {
      if (disposed || initializing) return;
      if (!nativeReady) {
        initializing = true;
        let snap: { mode: string; runGeneration: number } | null = null;
        try {
          snap = await getControlState();
        } catch {
          // Retry initialization on the next heartbeat without changing control.
        }
        if (disposed) {
          // Torn down while the snapshot was in flight: still carry the buffered
          // Stop (judged against the snapshot) onto the bridge before retiring it.
          if (snap && !nativeReady) {
            try {
              await applyInitial(snap);
            } catch {
              // best effort
            }
          }
          retire();
          return;
        }
        try {
          if (snap) await applyInitial(snap);
        } catch {
          // retry next tick
        } finally {
          initializing = false;
        }
        if (disposed) {
          retire(); // torn down while the buffer was being applied
          return;
        }
      }
      if (disposed || !nativeReady) return;
      // A Stop on the previous bridge issues its cancels first, but a hung retired
      // client must not stop this bridge from ever restoring.
      await waitBounded(retiringRef.current, RETIRING_WAIT_MS);
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
      if (!initializing) retire(); // otherwise `tick` retires it after the snapshot
      offEvents();
      offState();
      window.clearInterval(beat);
    };
  }, [client, onDaemonEvent, registerBridge, showPanelAction, ingestControlSnapshot, issueSnapshotTicket]);

  return null;
}
