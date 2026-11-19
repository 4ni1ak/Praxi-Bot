/** RunState strings returned by the daemon, distinct from tool-item "pending". */
export function isProcessingRunState(state: string): boolean {
  return state === "queued" || state === "running" || state === "waiting_approval";
}
