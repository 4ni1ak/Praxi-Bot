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
import { MOCK_HOST_ID, mockHostScenario } from "../api/mockHostScenario";
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
  giveUpConnecting,
  type HostStateMemory,
  type HostUiState,
  liveRunIds,
  settleHostState
} from "../lib/hostState";
import { type Home, homeFromInfo, LOCAL_HOME, wantedComputer } from "../lib/home";
import { markRemoteBaseUrl } from "../lib/liveViewState";
import { orderBots } from "../lib/botOrder";
import { useBotOrder } from "../store/botOrderStore";
import {
  applyEvent,
  EMPTY_EVENTS,
  type EventsState,
  markAnswered,
  type NeedsYouEntry,
  needsYou,
  reconcile
} from "./timeline";
import { humanLine, type ServiceProblem } from "../lib/humanError";

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
// "pending" is not a daemon run state; "queued" runs can still ask (card 237 PR C).
const LIVE_RUN_STATES = new Set(["queued", "running", "waiting_approval"]);

type Action =
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
  /** Why the local service is not answering (null while it is); the UI words it, see `humanService`. */
  serviceProblem: ServiceProblem | null;
  /** Seconds until the next automatic retry while the service is down. */
  retryInSeconds: number | null;
  /** Listen to the raw daemon events (live and replayed); returns the unsubscribe. */
  onDaemonEvent: (listener: (env: GlobalEnvelope) => void) => () => void;
  /** Where the bots live: the adopted host (the app follows it by itself) or this computer. */
  home: Home;
  /** Explicit, temporary "work on this computer for now" while a host is the home. */
  workLocalNow: boolean;
  setWorkLocalNow: (on: boolean) => void;
  /** Re-read which host is the home (after "make this server my bots' home"). */
  refreshHome: () => Promise<void>;
  /** This computer's own connection, whatever computer the views show. */
  localConn: Connection | null;
  /** The lists shown are the last known ones: the home is not answering. */
  homeStale: boolean;
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
  /** The open channel to the selected host (a guarded loopback proxy), if any. */
  hostConn: Connection | null;
  /** Paired hosts and the versions of this computer's daemon, as last polled. */
  hostList: HostLinkList | null;
  /** Switch the whole UI (API, events, live view) to this computer or a host. */
  selectComputer: (id: string) => Promise<void>;
  /** This computer's own daemon client (the "Add server" wizard talks to it, never to a host). */
  localApi: ApiClient | null;
  /** Re-read the paired hosts now (after the wizard paired one). */
  refreshHosts: () => Promise<void>;
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

/** Everything a session reads from one daemon: its lists, events and health. */
type DaemonData = Pick<
  DaemonContextValue,
  | "client"
  | "health"
  | "eventsConnected"
  | "bots"
  | "chats"
  | "runs"
  | "events"
  | "needs"
  | "refreshBots"
  | "refreshChats"
  | "refreshRuns"
  | "recordAnswer"
  | "onDaemonEvent"
>;

/**
 * Client, event stream, health and lists for ONE daemon connection. The local
 * daemon and (while selected) a paired host each run their own copy, so the local
 * safety machinery never depends on which computer the views are showing.
 */
function useDaemonData(conn: Connection | null): DaemonData {
  const [health, setHealth] = useState<Health | null>(null);
  const [eventsConnected, setEventsConnected] = useState(false);
  const [rawBots, setBots] = useState<Bot[]>([]);
  const botOrder = useBotOrder();
  const bots = useMemo(() => orderBots(rawBots, botOrder), [rawBots, botOrder]);
  const [chats, setChats] = useState<Chat[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  const [events, dispatch] = useReducer(reducer, EMPTY_EVENTS);

  const [client, setClient] = useState<ApiClient | null>(null);
  useEffect(() => {
    if (!conn) {
      setClient(null);
      return undefined;
    }
    if (conn.base_url !== MOCK_BASE_URL) {
      const made = new ApiClient(conn);
      setClient(made);
      // Switching computers (or leaving the host) drops this connection's requests in flight.
      return () => made.dispose();
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

  // initial data + event stream (replayed from the start: `Last-Event-ID: 0`)
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

  // health indicator + reconciliation with the daemon's own pending lists
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

  // keep the lists fresh when runs start / finish
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

  const recordAnswer = useCallback(
    (runId: string, callId: string, answer: string) =>
      dispatch({ type: "answered", runId, callId, answer }),
    []
  );
  const needs = useMemo(() => needsYou(events), [events]);

  return {
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
    onDaemonEvent
  };
}

/**
 * THIS computer's daemon: always connected, whichever computer the views show. It
 * also owns the iroh client side (paired hosts, pairing, the channel to a host),
 * so the local heartbeat, Stop and approval overlays keep working while a host is
 * selected.
 */
export function DaemonProvider({ children }: { children: ReactNode }) {
  const [phase, setPhase] = useState<Phase>("connecting");
  const [error, setError] = useState<string | null>(null);
  const [localConn, setLocalConn] = useState<Connection | null>(null);
  const [activeId, setActiveId] = useState<string>(LOCAL_COMPUTER);
  const [hostConn, setHostConn] = useState<Connection | null>(null);
  const [hostError, setHostError] = useState<string | null>(null);
  const [hostList, setHostList] = useState<HostLinkList | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [home, setHome] = useState<Home>(LOCAL_HOME);
  const [workLocalNow, setWorkLocalNow] = useState(false);
  const data = useDaemonData(localConn);

  // The local daemon's own client: it owns the pairing store and the channels.
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

  // Review states of a selected host in mock mode (`?host=empty|bots|offline`).
  useEffect(() => {
    const scenario = mockHostScenario();
    if (!scenario || !localConn) {
      return;
    }
    activeIdRef.current = MOCK_HOST_ID;
    setActiveId(MOCK_HOST_ID);
    setHome({ kind: "host", hostId: MOCK_HOST_ID, name: "Praxi host" });
    setHostList({
      local_version: "0.1.0-mock",
      local_api_version: "1",
      hosts: [
        {
          id: MOCK_HOST_ID,
          name: "Praxi host",
          state: scenario === "offline" || scenario === "dropped" ? "offline" : "connected"
        } as HostLinkInfo
      ]
    } as HostLinkList);
    // Review: `?temp=local` shows the explicit "work on this computer for now" mode.
    if (new URLSearchParams(window.location.search).get("temp") === "local") {
      activeIdRef.current = LOCAL_COMPUTER;
      setActiveId(LOCAL_COMPUTER);
      setWorkLocalNow(true);
      return;
    }
    if (scenario === "offline") {
      setHostError("host did not answer");
    } else {
      setHostConn(localConn);
    }
  }, [localConn]);

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

  // Automatic retry with backoff (2 s, 4 s, ... 15 s) while the local service does not answer.
  const failures = useRef(0);
  const firstTry = useRef(Date.now());
  const [retryInSeconds, setRetryInSeconds] = useState<number | null>(null);
  useEffect(() => {
    if (phase === "ready") {
      failures.current = 0;
      setRetryInSeconds(null);
      return undefined;
    }
    if (phase !== "error") {
      return undefined;
    }
    failures.current += 1;
    const delay = Math.min(2000 * 2 ** (failures.current - 1), 15000);
    const at = Date.now() + delay;
    setRetryInSeconds(Math.ceil(delay / 1000));
    const tick = window.setInterval(() => setRetryInSeconds(Math.max(1, Math.ceil((at - Date.now()) / 1000))), 500);
    const go = window.setTimeout(() => setAttempt((n) => n + 1), delay);
    return () => {
      window.clearInterval(tick);
      window.clearTimeout(go);
    };
  }, [phase]);

  // 2. paired hosts and their channel state (polled from this computer's daemon)
  const refreshHosts = useCallback(async () => {
    const c = localClientRef.current;
    if (!c) {
      return;
    }
    try {
      const list = await c.listHosts();
      setHostList(list);
      // Which host (if any) the owner made the home of the bots.
      const info = await c.homeInfo().catch(() => null);
      if (info) {
        setHome(homeFromInfo(info.host_id, list.hosts));
      }
    } catch {
      // keep the last list; the next poll retries
    }
  }, []);
  const refreshHome = refreshHosts;
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

  const selectComputer = useCallback(
    async (id: string) => {
      if (id === activeIdRef.current) {
        return;
      }
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
          setHostError(humanLine(e));
        }
      }
    },
    [refreshHosts]
  );

  // The app follows the home by itself: at startup, after a reconnect and when the owner adopts a
  // host. Only the explicit "work on this computer for now" mode keeps it on this computer.
  const wanted = wantedComputer(home, workLocalNow, LOCAL_COMPUTER);
  useEffect(() => {
    if (mockHostScenario()) {
      return;
    }
    if (wanted !== activeIdRef.current) {
      void selectComputer(wanted);
    }
  }, [wanted, selectComputer]);
  // A home that does not answer is retried every few seconds until the channel is back.
  useEffect(() => {
    if (mockHostScenario() || wanted === LOCAL_COMPUTER || activeId !== wanted || hostConn) {
      return undefined;
    }
    const id = window.setInterval(() => {
      const lc = localClientRef.current;
      if (!lc || activeIdRef.current !== wanted) {
        return;
      }
      lc.connectHost(wanted)
        .then((c) => {
          if (activeIdRef.current === wanted) {
            setHostError(null);
            setHostConn({ base_url: c.base_url, token: c.token, started_by_app: false, fake_provider: false, data_dir: "" });
          }
        })
        .catch(() => {});
    }, 5000);
    return () => window.clearInterval(id);
  }, [wanted, activeId, hostConn]);

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

  const value: DaemonContextValue = {
    ...data,
    phase,
    error,
    conn: localConn,
    retry,
    restartDaemon,
    serviceProblem:
      phase !== "error"
        ? null
        : activeId !== LOCAL_COMPUTER
          ? "wrongTarget"
          : Date.now() - firstTry.current < 20000
            ? "starting"
            : "down",
    retryInSeconds,
    home,
    workLocalNow,
    setWorkLocalNow,
    refreshHome,
    localConn,
    homeStale: false,
    activeComputerId: activeId,
    computers,
    activeHostName: null,
    hostState: null,
    versions: { local: hostList?.local_version ?? null, host: null },
    hostError,
    hostConn,
    hostList,
    selectComputer,
    localApi: localClient,
    refreshHosts,
    pairHost,
    stopHostRuns: async () => 0
  };
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/**
 * The views' daemon while a paired host is selected: the host's lists, events and
 * health through the guarded channel, replacing the context value for the views
 * only. Mount it per host (`key`); the local `DaemonProvider` above stays live.
 */
export function HostSession({ children }: { children: ReactNode }) {
  const local = useDaemon();
  const { hostConn, hostList, hostError, activeComputerId } = local;
  const data = useDaemonData(hostConn);
  const link =
    hostList?.hosts.find((h) => h.id === activeComputerId) ?? null;
  const rawHostState = useMemo<HostUiState>(() => {
    if (hostError && !hostConn) {
      return { kind: "offline" };
    }
    // Until the first list arrives the host is simply being connected.
    const l =
      hostList === null
        ? { id: activeComputerId, name: "", state: "connecting" as const }
        : link;
    return deriveHostState({
      link: l,
      health: data.health,
      localVersion: hostList?.local_version ?? null,
      localApiVersion: hostList?.local_api_version ?? null
    });
  }, [activeComputerId, data.health, hostConn, hostError, hostList, link]);
  // One stable state per connection: no flicker between connecting / connected / offline.
  const memory = useRef<HostStateMemory | null>(null);
  const [, setTick] = useState(0);
  memory.current = settleHostState(memory.current, rawHostState, Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setTick((n) => n + 1), 1000);
    return () => window.clearInterval(id);
  }, []);
  const hostState = giveUpConnecting(memory.current, Date.now());

  const clientRef = useRef(data.client);
  clientRef.current = data.client;
  if (data.client) {
    data.client.hostName = link?.name ?? local.activeHostName ?? "";
  }
  const stopHostRuns = useCallback(async () => {
    const c = clientRef.current;
    if (!c) {
      return 0;
    }
    // Listed scope with what the host itself reports as live right now.
    const ids = liveRunIds(await c.listRuns());
    if (ids.length === 0) {
      return 0;
    }
    await c.stopRuns({ stop_id: crypto.randomUUID(), run_ids: ids, scope: "listed" });
    return ids.length;
  }, []);

  // The home stopped answering: keep showing the last known bots and chats, greyed out.
  const lastLists = useRef({ bots: data.bots, chats: data.chats });
  if (data.bots.length > 0) {
    lastLists.current = { bots: data.bots, chats: data.chats };
  }
  const stale = hostState.kind === "offline" && data.bots.length === 0 && lastLists.current.bots.length > 0;

  const value: DaemonContextValue = {
    ...local,
    ...data,
    ...(stale ? lastLists.current : {}),
    homeStale: stale,
    phase: "ready",
    error: null,
    conn: hostConn,
    activeHostName: link?.name ?? null,
    hostState,
    versions: {
      local: hostList?.local_version ?? null,
      host: data.health?.version ?? null
    },
    stopHostRuns
  };
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
