import { useEffect, useState } from "react";
import type { ApiClient } from "../api/client";
import { changesSectionVisible } from "../lib/chatLogic";
import { RunChangesSection } from "./RunChangesSection";

/**
 * Owner rule (244-D): no empty sections. Asks the daemon once whether the run
 * changed any file and renders the Changes section only if it did. While the
 * answer is unknown, and when it fails, nothing is shown. Once the section has
 * been shown it stays (undoing every file must not make it vanish mid-action).
 */
export function RunChangesGate({
  runId,
  client,
  restoreSignal,
  safetyCheckpointIds
}: {
  runId: string;
  client: ApiClient | null;
  restoreSignal: number;
  safetyCheckpointIds?: string[];
}) {
  const [count, setCount] = useState<number | null>(null);
  const [seen, setSeen] = useState(false);

  useEffect(() => {
    if (!client) {
      return undefined;
    }
    let live = true;
    client
      .getRunChanges(runId, 0)
      .then((res) => {
        if (live) {
          setCount(res.changes.length);
          if (res.changes.length > 0) {
            setSeen(true);
          }
        }
      })
      .catch(() => {
        if (live) {
          setCount(null);
        }
      });
    return () => {
      live = false;
    };
  }, [client, runId, restoreSignal]);

  if (!seen && !changesSectionVisible(count)) {
    return null;
  }
  return (
    <RunChangesSection
      runId={runId}
      client={client}
      restoreSignal={restoreSignal}
      safetyCheckpointIds={safetyCheckpointIds}
    />
  );
}
