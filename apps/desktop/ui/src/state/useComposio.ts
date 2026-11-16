import { useCallback, useEffect, useRef, useState } from "react";
import type { ComposioApp, ComposioCatalog, ComposioStatus, ComposioToolkit } from "../api/types";
import { appendPage, pendingSlugs, upsertApp } from "../lib/composioApps";
import { openComposioLink } from "../lib/externalLink";
import { useDaemon } from "./DaemonContext";
import { humanLine } from "../lib/humanError";

/** How often a pending sign-in is checked, and when we stop asking. */
const POLL_MS = 2000;
const POLL_GIVE_UP_MS = 10 * 60 * 1000;
const SEARCH_DEBOUNCE_MS = 250;
/** While the daemon refreshes its catalog cache, the list is re-read now and then. */
const REFRESH_WATCH_MS = 3000;

export type LoginPhase = "idle" | "starting" | "waiting" | "failed";

/**
 * Composio state of the Apps modal: link status, connected apps, the catalog
 * (served instantly by the daemon from the bundled popular apps or its disk
 * cache), a poll for pending sign-ins and the one-time browser login.  Nothing
 * here ever sees an API key.
 */
export function useComposio(active: boolean, query: string, group: string) {
  const { client } = useDaemon();
  const [status, setStatus] = useState<ComposioStatus | null>(null);
  const [apps, setApps] = useState<ComposioApp[]>([]);
  const [items, setItems] = useState<ComposioToolkit[]>([]);
  const [meta, setMeta] = useState<Pick<ComposioCatalog, "source" | "refreshing" | "error"> | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [login, setLogin] = useState<LoginPhase>("idle");
  const seq = useRef(0);
  const onLinked = useRef<(() => void) | null>(null);

  const reloadApps = useCallback(async () => {
    if (!client) {
      return;
    }
    try {
      setApps(await client.composioApps());
    } catch (e) {
      setError(humanLine(e));
    }
  }, [client]);

  const reloadStatus = useCallback(async () => {
    if (!client) {
      return null;
    }
    try {
      const s = await client.composioStatus();
      setStatus(s);
      return s;
    } catch (e) {
      setError(humanLine(e));
      return null;
    }
  }, [client]);

  const loadCatalog = useCallback(
    async (search: string, grp: string, from: string | null) => {
      if (!client) {
        return;
      }
      const mine = ++seq.current;
      setLoading(true);
      try {
        const page = await client.composioToolkits({
          search: search.trim() || undefined,
          group: grp === "all" ? undefined : grp,
          cursor: from ?? undefined
        });
        if (mine !== seq.current) {
          return;
        }
        setItems((prev) => (from ? appendPage(prev, page.items) : page.items));
        setMeta({ source: page.source, refreshing: page.refreshing, error: page.error });
        setCursor(page.next_cursor ?? null);
        setError(null);
      } catch (e) {
        if (mine === seq.current) {
          setError(humanLine(e));
        }
      } finally {
        if (mine === seq.current) {
          setLoading(false);
        }
      }
    },
    [client]
  );

  // Status and apps when the modal opens.
  useEffect(() => {
    if (!active) {
      return;
    }
    void (async () => {
      await reloadStatus();
      await reloadApps();
    })();
  }, [active, reloadApps, reloadStatus]);

  // The catalog follows search and chip (debounced).  The daemon answers from memory.
  useEffect(() => {
    if (!active) {
      return undefined;
    }
    const id = setTimeout(() => void loadCatalog(query, group, null), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(id);
  }, [active, query, group, status?.configured, loadCatalog]);

  // While the daemon refreshes the cache, pick the result up when it lands.
  const refreshing = meta?.refreshing === true;
  useEffect(() => {
    if (!active || !refreshing) {
      return undefined;
    }
    const id = setTimeout(() => void loadCatalog(query, group, null), REFRESH_WATCH_MS);
    return () => clearTimeout(id);
  }, [active, refreshing, query, group, items, loadCatalog]);

  // Poll pending sign-ins until they settle.
  const pending = pendingSlugs(apps).join(",");
  useEffect(() => {
    if (!active || !client || pending === "") {
      return undefined;
    }
    const started = Date.now();
    const id = setInterval(() => {
      if (Date.now() - started > POLL_GIVE_UP_MS) {
        clearInterval(id);
        return;
      }
      for (const slug of pending.split(",")) {
        void client
          .composioApp(slug)
          .then((a) => setApps((prev) => upsertApp(prev, a)))
          .catch((e: unknown) => setError(humanLine(e)));
      }
    }, POLL_MS);
    return () => clearInterval(id);
  }, [active, client, pending]);

  // The one-time browser login: open the page, poll until Composio says linked.
  useEffect(() => {
    if (!active || !client || login !== "waiting") {
      return undefined;
    }
    const started = Date.now();
    const id = setInterval(() => {
      if (Date.now() - started > POLL_GIVE_UP_MS) {
        setLogin("failed");
        return;
      }
      void client
        .composioLoginStatus()
        .then(async (r) => {
          if (r.state === "linked") {
            setLogin("idle");
            await reloadStatus();
            const next = onLinked.current;
            onLinked.current = null;
            next?.();
          } else if (r.state === "failed") {
            setLogin("failed");
          }
        })
        .catch(() => undefined);
    }, POLL_MS);
    return () => clearInterval(id);
  }, [active, client, login, reloadStatus]);

  /** Start the one-time login; `then` runs once Composio is linked. */
  const startLogin = useCallback(
    async (then?: () => void): Promise<boolean> => {
      if (!client) {
        return false;
      }
      setLogin("starting");
      onLinked.current = then ?? null;
      try {
        const r = await client.composioLoginStart();
        if (!(await openComposioLink(r.login_url, true))) {
          setLogin("failed");
          return false;
        }
        setLogin("waiting");
        return true;
      } catch {
        setLogin("failed");
        return false;
      }
    },
    [client]
  );

  const cancelLogin = useCallback(() => {
    onLinked.current = null;
    setLogin("idle");
  }, []);

  return {
    status,
    apps,
    items,
    meta,
    hasMore: cursor !== null,
    loading,
    error,
    login,
    startLogin,
    cancelLogin,
    retryCatalog: () => loadCatalog(query, group, null),
    loadMore: () => loadCatalog(query, group, cursor),
    setApps,
    reloadApps,
    reloadStatus
  };
}
