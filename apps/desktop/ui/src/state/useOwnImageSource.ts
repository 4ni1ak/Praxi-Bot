import { useEffect, useState } from "react";
import { sampleImageSource, type ToneSource } from "../lib/backdropTone.ts";

/** Wait this long after the last resize before the own image is sampled again. */
export const RESAMPLE_DEBOUNCE_MS = 150;

export interface Sampled {
  url: string;
  width: number;
  height: number;
  source: ToneSource | null;
}

/** A sample counts only for the image and the window size it was taken at; anything else is the worst case. */
export function usableSample(sampled: Sampled | null, url: string, now: { width: number; height: number }): ToneSource | null {
  return sampled && sampled.url === url && sampled.width === now.width && sampled.height === now.height ? sampled.source : null;
}

/**
 * The pixels of the own background image as `background-size: cover` shows them at the current
 * window size.  The crop depends on the window, so the image is sampled again (debounced) when the
 * window changes, and the old sample stops counting as soon as the window differs from the size it
 * was taken at: until the new sample lands the result is `null` (the surface model's worst case).
 */
export function useOwnImageSource(url: string): ToneSource | null {
  const read = () => (typeof window === "undefined" ? { width: 0, height: 0 } : { width: window.innerWidth, height: window.innerHeight });
  // `now` follows every resize event; `size` (what gets sampled) only after the window settles.
  const [now, setNow] = useState(read);
  const [size, setSize] = useState(read);
  const [sampled, setSampled] = useState<Sampled | null>(null);

  useEffect(() => {
    if (!url) return undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const onResize = () => {
      clearTimeout(timer);
      // The sample stops counting right away (see the return below) ...
      setNow(read());
      // ... and is taken again once the window has settled.
      timer = setTimeout(() => setSize(read()), RESAMPLE_DEBOUNCE_MS);
    };
    window.addEventListener("resize", onResize);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("resize", onResize);
    };
  }, [url]);

  useEffect(() => {
    if (!url) return undefined;
    let live = true;
    void sampleImageSource(url, size.width, size.height).then((source) => {
      if (live) setSampled({ url, width: size.width, height: size.height, source });
    });
    return () => {
      live = false;
    };
  }, [url, size.width, size.height]);

  // Valid only for this image at the window's current size.
  return usableSample(sampled, url, now);
}
