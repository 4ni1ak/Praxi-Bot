import { useEffect, useState } from "react";
import { EXIT_MS, exitDelayMs } from "./presenceTiming.ts";

export { EXIT_MS, exitDelayMs };

/** True when animations are off: the OS preference or the app's own "Reduce motion" switch. */
export function motionIsReduced(): boolean {
  if (typeof document === "undefined") {
    return false;
  }
  return (
    document.documentElement.hasAttribute("data-reduce-motion") ||
    (typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches)
  );
}

/**
 * Presence for exit animations.  `mounted` stays true for one exit while `open` is false, `closing`
 * is true during that time (the element sets `data-state="closing"`, becomes inert and plays its exit
 * keyframes).  With reduced motion the surface leaves in the same render, so nothing lingers.
 * Opening again while it is still leaving cancels the exit.
 */
export function usePresence(open: boolean, ms: number = EXIT_MS): { mounted: boolean; closing: boolean } {
  const [lingering, setLingering] = useState(open);
  if (open && !lingering) {
    setLingering(true);
  }
  useEffect(() => {
    if (open || !lingering) {
      return undefined;
    }
    const id = window.setTimeout(() => setLingering(false), exitDelayMs(motionIsReduced(), ms));
    return () => window.clearTimeout(id);
  }, [open, lingering, ms]);
  const mounted = open || (lingering && !motionIsReduced());
  return { mounted, closing: !open && mounted };
}
