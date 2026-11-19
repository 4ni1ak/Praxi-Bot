/** Helpers of composer dictation (ADR 0021): pure, so they are tested without a microphone. */

/** Append dictated text to what is already typed, separated by one space (or nothing at a line start). */
export function appendDictation(current: string, dictated: string): string {
  const add = dictated.trim();
  if (!add) {
    return current;
  }
  if (!current || /\s$/.test(current)) {
    return current + add;
  }
  return `${current} ${add}`;
}

const MIME_CANDIDATES = ["audio/webm;codecs=opus", "audio/webm", "audio/ogg;codecs=opus", "audio/mp4"];

/** The first recording format this WebView can produce, or undefined (the engine picks). */
export function pickRecordingMime(isSupported: (m: string) => boolean): string | undefined {
  return MIME_CANDIDATES.find((m) => isSupported(m));
}

/** Whether this WebView can record audio at all. */
export function dictationSupported(): boolean {
  return (
    typeof navigator !== "undefined" &&
    !!navigator.mediaDevices?.getUserMedia &&
    typeof MediaRecorder !== "undefined"
  );
}

/** Base64 of a recording, without the data-URL prefix. */
export async function blobToBase64(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(bin);
}
