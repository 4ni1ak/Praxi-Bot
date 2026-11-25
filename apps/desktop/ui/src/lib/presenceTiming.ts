/**
 * How long a leaving surface (dialog, popover, menu, toast) stays mounted so its exit animation can
 * play: a little over `--motion-duration-exit` (120 ms) so the last frame is never cut.
 */
export const EXIT_MS = 160;

/** How long to keep a closed surface on screen: nothing at all when motion is reduced. */
export function exitDelayMs(reduced: boolean, ms: number = EXIT_MS): number {
  return reduced ? 0 : ms;
}
