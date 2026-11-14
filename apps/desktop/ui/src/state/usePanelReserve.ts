import { useEffect, useState } from "react";
import { PANEL_SETTLE_MS } from "../lib/shellLayout";

/**
 * Whether the right panel still takes room from the rail's fit: true while it is open and for
 * `PANEL_SETTLE_MS` after it started closing (its width/margin transition), so the rail does not
 * grow back into the panel's space mid-slide and squeeze the chat (card 233).
 */
export function usePanelReserve(open: boolean): boolean {
  const [settling, setSettling] = useState(open);
  useEffect(() => {
    if (open) {
      setSettling(true);
      return undefined;
    }
    const timer = setTimeout(() => setSettling(false), PANEL_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [open]);
  return open || settling;
}
