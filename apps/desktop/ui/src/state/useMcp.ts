import { useCallback, useEffect, useRef, useState } from "react";
import type { McpCatalogEntry, McpServer } from "../api/types";
import { applyServerEvent, eventNeedsRefetch } from "../lib/mcpApps";
import { useDaemon } from "./DaemonContext";
import { humanLine } from "../lib/humanError";

/**
 * Connected MCP servers + the catalog, kept fresh by the daemon's
 * `mcp_server_changed` events.
 */
export function useMcp(active: boolean): {
  servers: McpServer[];
  catalog: McpCatalogEntry[];
  loading: boolean;
  error: string | null;
  reload: () => Promise<void>;
} {
  const { client, onDaemonEvent } = useDaemon();
  const [servers, setServers] = useState<McpServer[]>([]);
  const [catalog, setCatalog] = useState<McpCatalogEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const serversRef = useRef<McpServer[]>([]);
  serversRef.current = servers;

  const reload = useCallback(async () => {
    if (!client) {
      return;
    }
    setLoading(true);
    try {
      const [s, c] = await Promise.all([
        client.listMcpServers(),
        client.mcpCatalog()
      ]);
      setServers(s);
      setCatalog(c);
      setError(null);
    } catch (e) {
      setError(humanLine(e));
    } finally {
      setLoading(false);
    }
  }, [client]);

  useEffect(() => {
    if (active) {
      void reload();
    }
  }, [active, reload]);

  useEffect(() => {
    if (!active) {
      return undefined;
    }
    return onDaemonEvent((env) => {
      const ev = env.event;
      if (ev.type !== "mcp_server_changed") {
        return;
      }
      // Show the new status at once; the event has no diagnostics, so settled
      // transitions (and servers this list has not seen) refetch the real DTO.
      const next = applyServerEvent(serversRef.current, ev);
      if (next) {
        setServers(next);
      }
      if (eventNeedsRefetch(serversRef.current, ev)) {
        void reload();
      }
    });
  }, [active, onDaemonEvent, reload]);

  return { servers, catalog, loading, error, reload };
}
