/** Maps a failed `getUserMedia` call to the i18n key that tells the owner what to do about it. */
export function micErrorKey(err: unknown): "voice.micDenied" | "voice.micMissing" | "voice.micBusy" {
  const name = typeof err === "object" && err !== null && "name" in err ? String((err as { name: unknown }).name) : "";
  switch (name) {
    case "NotFoundError":
    case "OverconstrainedError":
    case "DevicesNotFoundError":
      return "voice.micMissing";
    case "NotReadableError":
    case "AbortError":
    case "TrackStartError":
      return "voice.micBusy";
    default:
      return "voice.micDenied";
  }
}
