/** Puts text on the clipboard; resolves false (never throws) when the platform refuses. */
export async function copyText(
  text: string,
  clipboard: Pick<Clipboard, "writeText"> | undefined = typeof navigator === "undefined" ? undefined : navigator.clipboard
): Promise<boolean> {
  if (!clipboard || !text.trim()) {
    return false;
  }
  try {
    await clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
