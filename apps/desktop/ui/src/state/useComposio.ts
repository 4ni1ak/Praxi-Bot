import { useCallback, useEffect, useRef, useState } from "react";
import type { ComposioApp, ComposioStatus, ComposioToolkit } from "../api/types";
import { appendPage, pendingSlugs, upsertApp } from "../lib/composioApps";
import { useDaemon } from "./DaemonContext";

/** How often a pending sign-in is checked, and when we stop asking. */
const POLL_MS = 2000;
const POLL_GIVE_UP_MS = 10 * 60 * 1000;
const SEARCH_DEBOUNCE_MS = 300;

/**
 * Composio state of the Apps panel: key status, connected apps, the searchable
 * catalog (paged) and a poll for pending sign-ins.  Nothing here holds the key
 * after the paste field submitted it.
 */
export function useComposio(active: boolean, query: string) {
  const { client } = useDaemon();
  const [status, setStatus] = useState<ComposioStatus | null>(null);
  const [apps, setApps] = useState<ComposioApp[]>([]);
  const [popular, setPopular] = useState<ComposioToolkit[]>([]);
  const [items, setItems] = useState<ComposioToolkit[]>([]);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  const reloadApps = useCallback(async () => {
    if (!client) {
      return;
    }
    try {
      setApps(await client.composioApps());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
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
      setError(e instanceof Error ? e.message : String(e));
      return null;
    }
  }, [client]);

  const loadCatalog = useCallback(
    async (search: string, from: string | null) => {
      if (!client) {
        return;
      }
      const mine = ++seq.current;
      setLoading(true);
      try {
        const page = await client.composioToolkits({
          search: search.trim() || undefined,
          cursor: from ?? undefined
        });
        if (mine !== seq.current) {
          return;
        }
        setItems((prev) => (from ? appendPage(prev, page.items) : page.items));
        setPopular(page.popular);
        setCursor(page.next_cursor ?? null);
        setError(null);
      } catch (e) {
        if (mine === seq.current) {
          setError(e instanceof Error ? e.message : String(e));
        }
      } finally {
        if (mine === seq.current) {
          setLoading(false);
        }
      }
    },
    [client]
  );

  // Status and apps when the panel opens.
  useEffect(() => {
    if (!active) {
      return;
    }
    void (async () => {
      const s = await reloadStatus();
      if (s?.configured) {
        await reloadApps();
      }
    })();
  }, [active, reloadApps, reloadStatus]);

  // The catalog follows the search box (debounced) once a key exists.
  useEffect(() => {
    if (!active || !status?.configured) {
      return undefined;
    }
    const id = setTimeout(() => void loadCatalog(query, null), SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(id);
  }, [active, status?.configured, query, loadCatalog]);

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
          .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)));
      }
    }, POLL_MS);
    return () => clearInterval(id);
  }, [active, client, pending]);

  return {
    status,
    apps,
    popular,
    items,
    hasMore: cursor !== null,
    loading,
    error,
    loadMore: () => loadCatalog(query, cursor),
    setApps,
    reloadApps,
    reloadStatus
  };
}
