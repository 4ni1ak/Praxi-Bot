/**
 * What the UI asks the host for. The host never upscales and keeps a hard cap of 1920x1080, so
 * the open viewer asks for the cap and receives the screen's native size up to it; the browser
 * then draws that image 1:1 or reduces it, never stretches a small one. The small card only
 * needs enough pixels for a high-density display.
 */
export interface SessionRequest {
  maxWidth: number;
  maxHeight: number;
  quality: number;
}

/** Hard cap shared with the host (`MAX_OUTPUT_WIDTH` x `MAX_OUTPUT_HEIGHT`). */
export const VIEWER_MAX_WIDTH = 1920;
export const VIEWER_MAX_HEIGHT = 1080;

export function sessionRequest(input: { viewerOpen: boolean; owning: boolean }): SessionRequest {
  if (!input.viewerOpen) {
    // The card: a small picture, a modest budget.
    return { maxWidth: 640, maxHeight: 400, quality: 70 };
  }
  // Moving content is encoded at this quality (4:4:4, the host steps it down to fit a slow link); the host adds one lossless frame once the
  // screen has been still for a moment. The host clamps to its own ceilings.
  return {
    maxWidth: VIEWER_MAX_WIDTH,
    maxHeight: VIEWER_MAX_HEIGHT,
    quality: 80
  };
}
