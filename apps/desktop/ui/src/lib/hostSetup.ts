import type { HostSetupJob, HostSetupStep } from "../api/types";

/** The five rows of the wizard's progress list, in order. */
export const STAGES = ["connect", "check", "install", "pair", "verify"] as const;
export type Stage = (typeof STAGES)[number];

export type StageState =
  | "pending"
  | "running"
  /** Waiting for the user (confirm the host key). */
  | "waiting"
  | "done"
  /** Done, but nothing needed doing ("already installed"). */
  | "skipped"
  | "warn"
  | "failed";

/** Which row a step id of the provisioning engine belongs to. */
export function stageOf(stepId: string): Stage | null {
  if (stepId === "hostkey" || stepId === "connect" || stepId === "prepare") return "connect";
  if (stepId.startsWith("preflight")) return "check";
  if (stepId.startsWith("install")) return "install";
  if (stepId === "pair") return "pair";
  if (stepId.startsWith("health")) return "verify";
  return null;
}

/** The engine step whose success closes a row (rows without one close when the next starts). */
const CLOSER: Partial<Record<Stage, (id: string) => boolean>> = {
  connect: (id) => id === "connect",
  install: (id) => id === "install" || id === "install.run",
  pair: (id) => id === "pair"
};

const ACTIVE = new Set(["running", "awaiting_host_key"]);

export function isActive(job: Pick<HostSetupJob, "state">): boolean {
  return ACTIVE.has(job.state);
}

function stepsOf(job: HostSetupJob, stage: Stage): HostSetupStep[] {
  return job.steps.filter((s) => stageOf(s.id) === stage);
}

/** One honest state per row, derived only from what the engine has reported. */
export function stageStates(job: HostSetupJob): Record<Stage, StageState> {
  const out = {} as Record<Stage, StageState>;
  let seenOpen = false;
  STAGES.forEach((stage, i) => {
    const steps = stepsOf(job, stage);
    const laterStarted = STAGES.slice(i + 1).some((s) => stepsOf(job, s).length > 0);
    const closer = CLOSER[stage];
    const closed =
      laterStarted ||
      (closer ? steps.some((s) => closer(s.id) && s.status !== "fail") : false) ||
      (stage === "verify" && job.state === "succeeded" && steps.length > 0);
    if (steps.some((s) => s.status === "fail")) {
      out[stage] = "failed";
    } else if (seenOpen) {
      out[stage] = "pending";
    } else if (closed) {
      const onlySkips = steps.length > 0 && steps.every((s) => s.status === "skip");
      out[stage] = steps.some((s) => s.status === "warn")
        ? "warn"
        : onlySkips
          ? "skipped"
          : "done";
    } else if (isActive(job)) {
      seenOpen = true;
      out[stage] =
        stage === "connect" && job.state === "awaiting_host_key" ? "waiting" : "running";
    } else {
      // Cancelled or finished without reaching this row.
      seenOpen = true;
      out[stage] = "pending";
    }
  });
  // A failed row ends the run: everything after it never started.
  const failedAt = STAGES.findIndex((s) => out[s] === "failed");
  if (failedAt >= 0) {
    STAGES.slice(failedAt + 1).forEach((s) => {
      out[s] = "pending";
    });
  }
  return out;
}

export type FailureCode =
  | "hostkey_changed"
  | "hostkey_declined"
  | "hostkey_unreadable"
  | "ssh_client"
  | "auth"
  | "network"
  | "needs_sudo"
  | "preflight"
  | "not_installed"
  | "not_host_mode"
  | "service_down"
  | "pair"
  | "health"
  | "generic";

/** The first failed step and a code the UI turns into one human sentence. */
export function failure(job: HostSetupJob): { step: HostSetupStep; code: FailureCode } | null {
  const step = job.steps.find((s) => s.status === "fail");
  if (!step) return null;
  const d = step.detail.toLowerCase();
  let code: FailureCode = "generic";
  switch (step.id) {
    case "hostkey":
      code = d.startsWith("mismatch")
        ? "hostkey_changed"
        : d.startsWith("not accepted")
          ? "hostkey_declined"
          : d.includes("could not read the server")
            ? "hostkey_unreadable"
            : d.includes("no usable") || d.includes("no ssh host key") || d.includes("could not reach")
            ? "network"
            : "generic";
      break;
    case "prepare":
      code = "ssh_client";
      break;
    case "connect":
      code = d.includes("rejected the login")
        ? "auth"
        : d.includes("could not reach") || d.includes("timed out")
          ? "network"
          : "generic";
      break;
    case "preflight.sudo":
      code = "needs_sudo";
      break;
    case "install":
      code = d.includes("not installed")
        ? "not_installed"
        : d.includes("not running in host mode")
          ? "not_host_mode"
          : d.includes("service is not running")
            ? "service_down"
            : "generic";
      break;
    case "pair":
      code = "pair";
      break;
    default:
      code = step.id.startsWith("preflight")
        ? "preflight"
        : step.id.startsWith("health")
          ? "health"
          : "generic";
  }
  return { step, code };
}

/** Plain text for "copy details": the technical record, no secrets in it by construction. */
export function copyDetails(job: HostSetupJob): string {
  const lines = [`target: ${job.target}`, `state: ${job.state}`];
  for (const s of job.steps) {
    lines.push(`[${s.status}] ${s.id}: ${s.detail}`);
    if (s.hint && s.status !== "pass") lines.push(`   -> ${s.hint}`);
  }
  return lines.join("\n");
}

export type WizardView = "form" | "progress" | "done" | "failed";

/** Which page of the modal to show for a job (null job means the form). */
export function wizardView(job: HostSetupJob | null): WizardView {
  if (!job) return "form";
  if (job.state === "succeeded") return "done";
  if (job.state === "failed" || job.state === "cancelled") return "failed";
  return "progress";
}
