/**
 * THE ADAPTER of the computer card: everything the card knows about the view comes through
 * `useComputerView()`. It requests a watch session while visible and pauses after three quiet minutes.
 */
import { isMockMode } from "../api/mockMode";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { serverScreenIssue } from "../lib/botComputer";
import { noScreenCode } from "../lib/computerLabel";
import {
  type ComputerView, deriveView, IDLE_PAUSE_MS, measureFps, SCREEN_START_TIMEOUT_MS, type UnavailableReason
} from "../lib/computerView";
import { summarizeTarget } from "../lib/liveViewState";
import { LOCAL_COMPUTER, useDaemon } from "./DaemonContext";
import { useLiveView, useLocalLiveView } from "./LiveViewContext";

export function useComputerView(opts: {
  /** The panel is on screen: closing it ends the view. */
  visible: boolean;
  botId?: string | null;
  /** `mine`: this computer's own screen (shown next to a server home). */
  side: "bot" | "mine";
}): ComputerView {
  const { activeComputerId, hostState } = useDaemon();
  const homeView = useLiveView();
  const localView = useLocalLiveView();
  const view = opts.side === "mine" ? localView : homeView;
  const onHost = opts.side === "bot" && activeComputerId !== LOCAL_COMPUTER;

  const [wanted, setWanted] = useState(false);
  const [startFailure, setStartFailure] = useState<UnavailableReason | null>(null);
  const [pausedIdle, setPausedIdle] = useState(false);
  const [pendingTakeover, setPendingTakeover] = useState(false);
  const lastTouch = useRef(Date.now());
  const [arrivals, setArrivals] = useState<number[]>([]);
  const [liveSinceMs, setLiveSinceMs] = useState<number | null>(null);
  const [fps, setFps] = useState<number | null>(null);

  const { selectedTarget, targetsStatus, availability, frameUrl, state, viewerOpen } = view;
  const summary = selectedTarget ? summarizeTarget(selectedTarget) : null;
  const issue = onHost ? serverScreenIssue(hostState) : null;
  const asked = wanted && opts.visible;
  const derived = useMemo(
    () =>
      deriveView({
        wanted: asked,
        pausedIdle,
        issue,
        targetsStatus,
        hasTarget: Boolean(selectedTarget),
        targetUnavailable: summary && !summary.available ? summary.reason : null,
        startFailure,
        noScreen: noScreenCode(availability),
        permissionRefused: onHost && !issue && targetsStatus === "failed",
        hasFrame: Boolean(frameUrl)
      }),
    [asked, pausedIdle, issue, targetsStatus, selectedTarget, summary, availability, onHost, frameUrl, startFailure]
  );

  // The view session exists only while the owner asked for it and the panel is on screen.
  const setDemand = view.setCardDemand;
  const demanding = asked && !startFailure;
  useEffect(() => {
    if (!demanding) {
      return undefined;
    }
    setDemand(true, opts.botId ?? null);
    return () => setDemand(false, null);
  }, [demanding, opts.botId, setDemand]);

  // The panel closing ends the view (it is not "paused": nobody was left looking at a card).
  useEffect(() => {
    if (!opts.visible && wanted) {
      setWanted(false);
      setStartFailure(null);
    }
  }, [opts.visible, wanted]);
  // Switching to the other computer (or bot) ends the view of this one.
  useEffect(() => {
    setWanted(false);
    setPausedIdle(false);
    setStartFailure(null);
  }, [opts.side, opts.botId, activeComputerId]);

  useEffect(() => {
    if (demanding && view.streamFailure && !frameUrl) {
      setStartFailure(view.streamFailure);
    }
  }, [demanding, view.streamFailure, frameUrl]);

  useEffect(() => {
    if (demanding && view.streamPaused) {
      setWanted(false);
      setPausedIdle(true);
    }
  }, [demanding, view.streamPaused]);

  useEffect(() => {
    if (!demanding || derived.kind !== "starting") {
      return undefined;
    }
    const id = window.setTimeout(() => {
      setStartFailure((failure) => failure ?? "timeout");
    }, SCREEN_START_TIMEOUT_MS);
    return () => window.clearTimeout(id);
  }, [demanding, derived.kind]);

  // Quiet time: no interaction (and no large viewer open) ends the view.
  useEffect(() => {
    if (!demanding) {
      return undefined;
    }
    lastTouch.current = Date.now();
    const id = window.setInterval(() => {
      if (viewerOpen) {
        lastTouch.current = Date.now();
        return;
      }
      if (Date.now() - lastTouch.current >= IDLE_PAUSE_MS) {
        setWanted(false);
        setPausedIdle(true);
      }
    }, 5000);
    return () => window.clearInterval(id);
  }, [demanding, viewerOpen]);

  // Frame rate and live time.
  const live = derived.kind === "live";
  useEffect(() => {
    setLiveSinceMs((prev) => (live ? (prev ?? Date.now()) : null));
    if (!live) {
      setArrivals([]);
      setFps(null);
    }
  }, [live]);
  useEffect(() => {
    if (live && frameUrl) {
      setArrivals((prev) => [...prev.filter((t) => Date.now() - t <= 3000), Date.now()]);
    }
  }, [live, frameUrl]);
  useEffect(() => {
    if (!live) {
      return undefined;
    }
    const id = window.setInterval(() => setFps(measureFps(arrivals, Date.now())), 1000);
    return () => window.clearInterval(id);
  }, [live, arrivals]);

  // "Kontrolü al": the takeover is requested once the large viewer holds a session.
  const requestTakeover = view.requestTakeover;
  useEffect(() => {
    if (pendingTakeover && viewerOpen && state.session) {
      setPendingTakeover(false);
      void requestTakeover();
    }
  }, [pendingTakeover, viewerOpen, state.session, requestTakeover]);

  const refreshTargets = view.refreshTargets;
  const start = useCallback(() => {
    lastTouch.current = Date.now();
    setPausedIdle(false);
    setStartFailure(null);
    setWanted(true);
    void refreshTargets();
  }, [refreshTargets]);
  const stop = useCallback(() => {
    setWanted(false);
    setPausedIdle(false);
    setStartFailure(null);
  }, []);
  const touch = useCallback(() => {
    lastTouch.current = Date.now();
  }, []);
  const openViewer = view.openViewer;
  const targetId = selectedTarget?.target_id;
  const bigView = useCallback(() => openViewer(targetId), [openViewer, targetId]);
  const takeControl = useCallback(() => {
    setPendingTakeover(true);
    openViewer(targetId);
  }, [openViewer, targetId]);

  const browserId = view.targets.find((t) => t.kind === "browser" && (!t.bot_id || t.bot_id === opts.botId))?.target_id;
  const openBrowser = useCallback(() => openViewer(browserId), [openViewer, browserId]);

  const frame = state.latestFrame;
  // Design review (mock mode only): `?cv=starting|paused|offline` shows a state that is hard to catch.
  const forced = isMockMode() ? new URLSearchParams(window.location.search).get("cv") : null;
  const kind = forced === "starting" || forced === "paused" ? forced : forced === "offline" ? "unavailable" : derived.kind;
  return {
    kind,
    paused: forced === "paused" ? "idle" : derived.paused,
    unavailable: forced === "offline" ? "offline" : derived.unavailable,
    frameUrl: live ? frameUrl : null,
    resolution: live && frame ? `${frame.width}×${frame.height}` : null,
    fps: live ? fps : null,
    liveSinceMs,
    start,
    stop,
    bigView,
    takeControl,
    openBrowser,
    touch
  };
}
