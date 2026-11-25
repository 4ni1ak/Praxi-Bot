import type { components } from "../api/schema";

/** Observations are made on the selected host against its configured providers. */
export type ServiceConnectivity = components["schemas"]["ServiceConnectivity"];
export type ConnectivityReport = components["schemas"]["ConnectivityReport"];
export type ConnectionState = "unknown" | "online" | "offline" | "host_unreachable";

export function connectionState(report: ConnectivityReport | null): ConnectionState {
  const remote = report?.services.filter((s) => !s.local) ?? [];
  return remote.length === 0 ? "unknown" : remote.some((s) => s.reachable) ? "online" : "offline";
}

/** An available local model must be in THIS bot's explicit executor/fallback chain. */
export function canAnswer(report: ConnectivityReport | null, models: readonly string[]): boolean {
  if (!report) return true; // Unknown is not proof of being offline.
  return models.length === 0 || models.some((model) => {
    const provider = model.split("/")[0];
    const row = report.services.find((s) => s.id === provider);
    return !row || row.reachable;
  });
}
