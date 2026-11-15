/**
 * Connection state of a paired always-on host as the app shows it (card 252).
 * Pure: the inputs are what the local daemon reports about the link
 * (`GET /v1/hosts`) and what the host answered on its public health route.
 */

export type LinkState =
  | "idle"
  | "connecting"
  | "connected"
  | "offline"
  | "unreachable";

export interface HostLinkLike {
  id: string;
  name: string;
  state: LinkState;
}

export interface HostHealthLike {
  status: string;
  version: string;
  api_version: string;
}

export type HostUiState =
  | { kind: "connecting" }
  | { kind: "connected" }
  | { kind: "offline" }
  /** Revoked by the host, removed from the list, or never paired. */
  | { kind: "not_paired" }
  /** The two daemons speak different API versions (blocking) or builds. */
  | { kind: "version_mismatch"; local: string; host: string; blocking: boolean }
  /** Connected and compatible, but the host cannot run a model yet. */
  | { kind: "no_model_key" };

/** Compare `major.minor` only: patch differences never block. */
function sameMinor(a: string, b: string): boolean {
  const key = (v: string) => v.split(".").slice(0, 2).join(".");
  return key(a) === key(b);
}

export function deriveHostState(input: {
  link: HostLinkLike | null;
  health: HostHealthLike | null;
  localVersion: string | null;
  localApiVersion: string | null;
}): HostUiState {
  const { link, health, localVersion, localApiVersion } = input;
  if (!link || link.state === "unreachable") {
    return { kind: "not_paired" };
  }
  if (link.state === "offline") {
    return { kind: "offline" };
  }
  if (link.state !== "connected" || !health) {
    return { kind: "connecting" };
  }
  if (localVersion) {
    const apiDiffers =
      localApiVersion !== null && health.api_version !== localApiVersion;
    if (apiDiffers || !sameMinor(health.version, localVersion)) {
      return {
        kind: "version_mismatch",
        local: localVersion,
        host: health.version,
        blocking: true
      };
    }
    if (health.version !== localVersion) {
      return {
        kind: "version_mismatch",
        local: localVersion,
        host: health.version,
        blocking: false
      };
    }
  }
  if (health.status === "setup_required") {
    return { kind: "no_model_key" };
  }
  return { kind: "connected" };
}

/** Whether the app may send work to the host in this state. */
export function hostAcceptsWork(state: HostUiState): boolean {
  return (
    state.kind === "connected" ||
    (state.kind === "version_mismatch" && !state.blocking)
  );
}

/** Whether chats can run a model there (the host holds a usable key). */
export function hostCanRunModel(state: HostUiState): boolean {
  return hostAcceptsWork(state);
}

/** Runs in these states are live and are what "stop on host" lists. */
const LIVE = new Set(["pending", "running", "waiting_approval"]);

export function liveRunIds(runs: ReadonlyArray<{ id: string; state: string }>): string[] {
  return runs.filter((r) => LIVE.has(r.state)).map((r) => r.id);
}

/**
 * What is mounted for a given active computer. The local safety machinery (the
 * control bridge: heartbeat, native Stop / tray, local events, approval overlays)
 * is ALWAYS connected to this computer's daemon; a host only adds a second
 * session for the views.
 */
export function computerScopePlan(
  activeComputerId: string,
  localId = "local"
): { localBridge: true; hostSession: boolean } {
  return { localBridge: true, hostSession: activeComputerId !== localId };
}
