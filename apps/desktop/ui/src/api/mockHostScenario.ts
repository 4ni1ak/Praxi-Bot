import { isMockMode } from "./mockMode.ts";

/**
 * Review states of a selected host (mock mode only): `?host=empty|bots|offline|dropped`.
 * `dropped`: the bots are known but the link to the server just went down.
 */
export type MockHostScenario = "empty" | "bots" | "offline" | "dropped";

export const MOCK_HOST_ID = "mock-host";

export function mockHostScenario(): MockHostScenario | null {
  if (!isMockMode() || typeof window === "undefined") {
    return null;
  }
  const v = new URLSearchParams(window.location.search).get("host");
  return v === "empty" || v === "bots" || v === "offline" || v === "dropped" ? v : null;
}
