/**
 * Whether the local service counts as "up", from what `GET /v1/health` answered.
 * Same contract as the daemon and the desktop shell (Rust): the shared fixture
 * `crates/daemon-api/tests/fixtures/health-contract.json` drives the tests of all three.
 *
 * Up = HTTP 200 and `status` is `ok` or `setup_required`. `setup_required` is the healthy
 * first-run state (no provider yet), never a failure.
 */

export type ServiceReadiness =
  | "unreachable"
  | "starting"
  | "degraded"
  | "setup_required"
  | "ok";

export interface HealthReply {
  /** HTTP status, or null when nothing answered. */
  httpStatus: number | null;
  /** Parsed JSON body, or null when there was none. */
  body: unknown;
}

const ACCEPTED_STATUS = ["ok", "setup_required"];

export function serviceReadiness(reply: HealthReply): ServiceReadiness {
  if (reply.httpStatus !== 200) {
    return "unreachable";
  }
  const body = reply.body;
  if (typeof body !== "object" || body === null) {
    return "unreachable";
  }
  const { status, startup } = body as { status?: unknown; startup?: unknown };
  if (typeof status !== "string" || !ACCEPTED_STATUS.includes(status)) {
    return "unreachable";
  }
  if (startup === "degraded") {
    return "degraded";
  }
  if (startup === "starting") {
    return "starting";
  }
  return status === "setup_required" ? "setup_required" : "ok";
}

export function serviceIsUp(reply: HealthReply): boolean {
  return serviceReadiness(reply) !== "unreachable";
}
