import { useCallback, useEffect, useRef, useState } from "react";
import type { ProvidersDto } from "../api/types";
import { createScopedLoader } from "../lib/scopedLoader";
import { useDaemon } from "./DaemonContext";
import { humanLine } from "../lib/humanError";

const listeners = new Set<() => void>();

/** Notify every `useProviders()` subscriber to refetch `GET /v1/providers`. */
export function notifyProvidersChanged() {
  for (const listener of listeners) {
    listener();
  }
}

/**
 * `GET /v1/providers` of the SELECTED computer, loaded once and on `reload()`; `null` until
 * known. Switching computers clears it and drops answers of the previous target.
 */
export function useProviders(): {
  dto: ProvidersDto | null;
  error: string | null;
  reload: () => Promise<void>;
} {
  const { client } = useDaemon();
  const [dto, setDto] = useState<ProvidersDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const loader = useRef(
    createScopedLoader<ProvidersDto>(setDto, (e) =>
      setError(humanLine(e))
    )
  );

  const reload = useCallback(async () => {
    if (!client) {
      loader.current.switchTo(null);
      return;
    }
    if (await loader.current.load(client, () => client.providers())) {
      setError(null);
    }
  }, [client]);

  useEffect(() => {
    void reload();
    const onChanged = () => {
      void reload();
    };
    listeners.add(onChanged);
    return () => {
      listeners.delete(onChanged);
    };
  }, [reload]);

  return { dto, error, reload };
}
