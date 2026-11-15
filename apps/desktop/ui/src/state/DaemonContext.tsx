import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState
} from "react";
import { ApiClient } from "../api/client";
import { type Connection, connectDaemon } from "../api/connection";
import { subscribeEvents } from "../api/events";
import { MOCK_BASE_URL } from "../api/mockMode";
import type {
  Bot,
  Chat,
  GlobalEnvelope,
  Health,
  HostLinkInfo,
  HostLinkList,
  Run
} from "../api/types";
import {
  deriveHostState,
  type HostUiState,
  liveRunIds
} from "../lib/hostState";
import { markRemoteBaseUrl } from "../lib/liveViewState";
import {
  applyEvent,
  EMPTY_EVENTS,
  type EventsState,
  markAnswered,
  type NeedsYouEntry,
  needsYou,
  reconcile
} from "./timeline";

export type Phase = "connecting" | "ready" | "error";

/** The id of this computer in the computer selector. */
export const LOCAL_COMPUTER = "local";

/** One entry of the computer selector. */
export interface ComputerEntry {
  id: string;
  kind: "local" | "host";
  name: string;
  /** Channel state of a host as the local daemon reports it. */
  link: HostLinkInfo | null;
}

/** Runs in these states can still produce approvals / questions. */
const LIVE_RUN_STATES = new Set(["pending", "running", "waiting_approval"]);

type Action =
  | { type: "reset" }
  | { type: "event"; env: GlobalEnvelope }
  | { type: "answered"; runId: string; callId: string; answer: string }
  | {
      type: "reconcile";
      approvals: ReadonlySet<string>;
      live: ReadonlySet<string>;
      asOfMs: number;
    };

function reducer(state: EventsState, action: Action): EventsState {
  switch (action.type) {
    case "reset":
      return EMPTY_EVENTS;
    case "event":
      return applyEvent(state, action.env);
    case "answered":
      return markAnswered(state, action.runId, action.callId, action.answer);
    case "reconcile":
      return reconcile(state, action.approvals, action.live, action.asOfMs);
  }
}

export interface DaemonContextValue {
  phase: Phase;
  error: string | null;
  conn: Connection | null;
  client: ApiClient | null;
  health: Health | null;
  eventsConnected: boolean;
  bots: Bot[];
  chats: Chat[];
  runs: Run[];
  events: EventsState;
  needs: NeedsYouEntry[];
  refreshBots: () => Promise<void>;
  refreshChats: () => Promise<void>;
  refreshRuns: () => Promise<void>;
  recordAnswer: (runId: string, callId: string, answer: string) => void;
  retry: () => void;
  restartDaemon: () => Promise<void>;
  /** Listen to the raw daemon events (live and replayed); returns the unsubscribe. */
  onDaemonEvent: (listener: (env: GlobalEnvelope) => void) => () => void;
  /** Which computer the whole UI talks to: `local` or a paired host's key. */
  activeComputerId: string;
  /** This computer plus every paired host. */
  computers: ComputerEntry[];
  /** Name of the active host (null while this computer is active). */
  activeHostName: string | null;
  /** State of the connection to the active host; null while this computer is active. */
  hostState: HostUiState | null;
  /** Both versions, for the version-mismatch message. */
  versions: { local: string | null; host: string | null };
  /** Why the last attempt to open the channel failed, if it did. */
  hostError: string | null;
  /** Switch the whole UI (API, events, live view) to this computer or a host. */
  selectComputer: (id: string) => Promise<void>;
  /** Pair with a host by ticket; resolves to the new entry. */
  pairHost: (ticket: string) => Promise<HostLinkInfo>;
  /** Stop every live run on the active host (listed scope, never a sweep). */
  stopHostRuns: () => Promise<number>;
}

const Ctx = createContext<DaemonContextValue | null>(null);

export function useDaemon(): DaemonContextValue {
  const v = useContext(Ctx);
  if (!v) {
    throw new Error("useDaemon outside DaemonProvider");
  }
  return v;
}

export function DaemonProvider({ children }: { children: ReactNode }) {
  const [phase, setPhase] = useState<Phase>("connecting");
  const [error, setError] = useState<string | null>(null);
  const [localConn, setLocalConn] = useState<Connection | null>(null);
  const [activeId, setActiveId] = useState<string>(LOCAL_COMPUTER);
  const [hostConn, setHostConn] = useState<Connection | null>(null);
  const [hostError, setHostError] = useState<string | null>(null);
  const [hostList, setHostList] = useState<HostLinkList | null>(null);
  const conn = activeId === LOCAL_COMPUTER ? localConn : hostConn;
  const [health, setHealth] = useState<Health | null>(null);
  const [eventsConnected, setEventsConnected] = useState(false);
  const [bots, setBots] = useState<Bot[]>([]);
  const [chats, setChats] = useState<Chat[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  const [events, dispatch] = useReducer(reducer, EMPTY_EVENTS);
  const [attempt, setAttempt] = useState(0);

  const [client, setClient] = useState<ApiClient | null>(null);
  useEffect(() => {
    if (!conn) {
      setClient(null);
      return undefined;
    }
    if (conn.base_url !== MOCK_BASE_URL) {
      setClient(new ApiClient(conn));
      return undefined;
    }
    // Mock code is only reachable in dev builds or with VITE_MOCK=1.
    if (!(import.meta.env.DEV || import.meta.env.VITE_MOCK === "1")) {
      setClient(null);
      return undefined;
    }
    let live = true;
    void import("../api/mock").then((m) => {
      if (live) setClient(new m.MockApiClient());
    });
    return () => {
      live = false;
    };
  }, [conn]);
  const clientRef = useRef<ApiClient | null>(null);
  clientRef.current = client;
  // The local daemon's own client: it owns the pairing store and the channels,
  // so it is used for the computer list even while a host is the active one.
  const localClient = useMemo(
    () =>
      localConn && localConn.base_url !== MOCK_BASE_URL
        ? new ApiClient(localConn)
        : null,
    [localConn]
  );
  const localClientRef = useRef<ApiClient | null>(null);
  localClientRef.current = localClient;
  const activeIdRef = useRef(activeId);
  activeIdRef.current = activeId;
  const listeners = useRef(new Set<(env: GlobalEnvelope) => void>());
  const onDaemonEvent = useCallback(
    (listener: (env: GlobalEnvelope) => void) => {
      listeners.current.add(listener);
      return () => {
        listeners.current.delete(listener);
      };
    },
    []
  );

  // 1. find / start the daemon
  useEffect(() => {
    let cancelled = false;
    setPhase("connecting");
    connectDaemon()
      .then((c) => {
        if (!cancelled) {
          setLocalConn(c);
          setPhase("ready");
          setError(null);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
          setPhase("error");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [attempt]);

  const refreshBots = useCallback(async () => {
    const c = clientRef.current;
    if (c) {
      setBots(await c.listBots());
    }
  }, []);
  const refreshChats = useCallback(async () => {
    const c = clientRef.current;
    if (c) {
      setChats(await c.listChats());
    }
  }, []);
  const refreshRuns = useCallback(async () => {
    const c = clientRef.current;
    if (c) {
      setRuns(await c.listRuns());
    }
  }, []);

  // 2. initial data + event stream (replayed from the start: `Last-Event-ID: 0`)
  useEffect(() => {
    if (!client) {
      return undefined;
    }
    void Promise.all([refreshBots(), refreshChats(), refreshRuns()]).catch(
      () => {}
    );
    if (client.baseUrl === MOCK_BASE_URL) {
      setEventsConnected(true);
      if (!(import.meta.env.DEV || import.meta.env.VITE_MOCK === "1")) {
        return undefined;
      }
      let live = true;
      void import("../api/mock").then((m) => {
        if (!live) return;
        m.mockReviewEvents().forEach((event, i) =>
          dispatch({ type: "event", env: { v: 1, seq: i + 1, gseq: i + 1, ts_ms: event.ts_ms, event: event.event } })
        );
      });
      return () => {
        live = false;
      };
    }
    return subscribeEvents(client, {
      lastEventId: 0,
      onEvent: (env) => {
        dispatch({ type: "event", env });
        for (const l of listeners.current) {
          l(env);
        }
      },
      onConnection: setEventsConnected
    });
  }, [client, refreshBots, refreshChats, refreshRuns]);

  // 3. health indicator + reconciliation with the daemon's own pending lists
  useEffect(() => {
    if (!client) {
      return undefined;
    }
    let stopped = false;
    const tick = async () => {
      const startedAt = Date.now();
      try {
        const [h, approvals, runList] = await Promise.all([
          client.health(),
          client.listApprovals(),
          client.listRuns()
        ]);
        if (stopped) {
          return;
        }
        setHealth(h);
        setRuns(runList);
        dispatch({
          type: "reconcile",
          approvals: new Set(approvals.map((a) => a.call_id)),
          live: new Set(
            runList.filter((r) => LIVE_RUN_STATES.has(r.state)).map((r) => r.id)
          ),
          asOfMs: startedAt - 1500
        });
      } catch {
        if (!stopped) {
          setHealth(null);
        }
      }
    };
    void tick();
    const id = window.setInterval(() => void tick(), 3000);
    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [client]);

  // 4. keep the lists fresh when runs start / finish
  const lastRunEvents = useRef(0);
  useEffect(() => {
    const total = Object.keys(events.runs).length;
    const finished = Object.values(events.runs).filter(
      (r) => r.finishedMs !== undefined
    ).length;
    const signature = total * 10_000 + finished;
    if (signature !== lastRunEvents.current && client) {
      lastRunEvents.current = signature;
      void Promise.all([refreshChats(), refreshRuns()]).catch(() => {});
    }
  }, [events.runs, client, refreshChats, refreshRuns]);

  // 5. paired hosts and their channel state (polled from this computer's daemon)
  const refreshHosts = useCallback(async () => {
    const c = localClientRef.current;
    if (!c) {
      return;
    }
    try {
      setHostList(await c.listHosts());
    } catch {
      // keep the last list; the next poll retries
    }
  }, []);
  useEffect(() => {
    if (!localClient) {
      return undefined;
    }
    void refreshHosts();
    const id = window.setInterval(() => void refreshHosts(), 4000);
    return () => window.clearInterval(id);
  }, [localClient, refreshHosts]);

  // The proxy to a host listens on loopback but its user is a paired peer.
  useEffect(() => {
    return hostConn ? markRemoteBaseUrl(hostConn.base_url) : undefined;
  }, [hostConn]);

  const resetData = useCallback(() => {
    setBots([]);
    setChats([]);
    setRuns([]);
    setHealth(null);
    setEventsConnected(false);
    dispatch({ type: "reset" });
  }, []);

  const selectComputer = useCallback(
    async (id: string) => {
      if (id === activeIdRef.current) {
        return;
      }
      resetData();
      setHostConn(null);
      setHostError(null);
      activeIdRef.current = id;
      setActiveId(id);
      if (id === LOCAL_COMPUTER) {
        return;
      }
      const lc = localClientRef.current;
      if (!lc) {
        setHostError("local_daemon_unavailable");
        return;
      }
      try {
        const c = await lc.connectHost(id);
        if (activeIdRef.current !== id) {
          return;
        }
        setHostConn({
          base_url: c.base_url,
          token: c.token,
          started_by_app: false,
          fake_provider: false,
          data_dir: ""
        });
        void refreshHosts();
      } catch (e) {
        if (activeIdRef.current === id) {
          setHostError(e instanceof Error ? e.message : String(e));
        }
      }
    },
    [refreshHosts, resetData]
  );

  const pairHost = useCallback(
    async (ticket: string) => {
      const c = localClientRef.current;
      if (!c) {
        throw new Error("local_daemon_unavailable");
      }
      const info = await c.pairHost(ticket);
      await refreshHosts();
      return info;
    },
    [refreshHosts]
  );

  const stopHostRuns = useCallback(async () => {
    const c = clientRef.current;
    if (!c || activeIdRef.current === LOCAL_COMPUTER) {
      return 0;
    }
    // Listed scope with what the host itself reports as live right now.
    const ids = liveRunIds(await c.listRuns());
    if (ids.length === 0) {
      return 0;
    }
    await c.stopListedRuns(crypto.randomUUID(), ids);
    return ids.length;
  }, []);

  const computers = useMemo<ComputerEntry[]>(
    () => [
      { id: LOCAL_COMPUTER, kind: "local", name: "", link: null },
      ...(hostList?.hosts ?? []).map((h) => ({
        id: h.id,
        kind: "host" as const,
        name: h.name,
        link: h
      }))
    ],
    [hostList]
  );
  const activeLink =
    activeId === LOCAL_COMPUTER
      ? null
      : (hostList?.hosts.find((h) => h.id === activeId) ?? null);
  const hostState = useMemo<HostUiState | null>(() => {
    if (activeId === LOCAL_COMPUTER) {
      return null;
    }
    if (hostError && !hostConn) {
      return { kind: "offline" };
    }
    // Until the first list arrives the host is simply being connected.
    const link =
      hostList === null
        ? { id: activeId, name: "", state: "connecting" as const }
        : activeLink;
    return deriveHostState({
      link,
      health,
      localVersion: hostList?.local_version ?? null,
      localApiVersion: hostList?.local_api_version ?? null
    });
  }, [activeId, activeLink, health, hostConn, hostError, hostList]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  const restartDaemon = useCallback(async () => {
    setPhase("connecting");
    try {
      const c = await connectDaemon(true);
      setLocalConn(c);
      setPhase("ready");
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }, []);
  const recordAnswer = useCallback(
    (runId: string, callId: string, answer: string) =>
      dispatch({ type: "answered", runId, callId, answer }),
    []
  );

  const needs = useMemo(() => needsYou(events), [events]);

  const value: DaemonContextValue = {
    phase,
    error,
    conn,
    client,
    health,
    eventsConnected,
    bots,
    chats,
    runs,
    events,
    needs,
    refreshBots,
    refreshChats,
    refreshRuns,
    recordAnswer,
    retry,
    restartDaemon,
    onDaemonEvent,
    activeComputerId: activeId,
    computers,
    activeHostName: activeLink?.name ?? null,
    hostState,
    versions: {
      local: hostList?.local_version ?? null,
      host: activeId === LOCAL_COMPUTER ? null : (health?.version ?? null)
    },
    hostError,
    selectComputer,
    pairHost,
    stopHostRuns
  };
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
