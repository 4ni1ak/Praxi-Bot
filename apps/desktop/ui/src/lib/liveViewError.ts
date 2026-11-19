/** Stable codes for errors from the daemon, the proxy or the HTTP client. */
export function liveViewErrorCode(error: { code: string; status: number }): string {
  if (error.code === "network" || (error.status === 0 && error.code !== "timeout")) return "network";
  if (error.status === 401) return "unauthorized";
  if (error.status === 502) return "server_restarting";
  if (error.status === 504) return "timeout";
  return error.code;
}
