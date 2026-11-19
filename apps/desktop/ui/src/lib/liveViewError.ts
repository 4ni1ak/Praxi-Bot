import i18n from "i18next";
/** Stable codes for errors from the daemon, the proxy or the HTTP client. */
export function liveViewErrorCode(error: { code: string; status: number }): string {
  if (error.code === "network" || (error.status === 0 && error.code !== "timeout")) return "network";
  if (error.status === 401) return "unauthorized";
  if (error.status === 502) return "server_restarting";
  if (error.status === 504) return "timeout";
  return error.code;
}

/**
 * The line shown for a live-view failure: a dedicated `liveView.errors.<code>` /
 * `liveView.reasons.<code>` text when one exists, otherwise the already-humanized message
 * (what the host or daemon said, in words); the generic sentence only when there is nothing.
 */
export function liveViewErrorText(error: { code: string; message?: string } | string | null | undefined): string {
  const code = typeof error === "string" ? error : (error?.code ?? "");
  const message = typeof error === "string" ? "" : (error?.message ?? "");
  for (const key of [`liveView.errors.${code}`, `liveView.reasons.${code}`]) {
    if (code && i18n.exists(key)) {
      return String(i18n.t(key));
    }
  }
  return message.trim() || String(i18n.t("liveView.errors.generic"));
}
