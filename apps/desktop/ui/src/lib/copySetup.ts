import type { CopyItemResult, CopyPlan, CopyReport, CopyRequest } from "../api/types.ts";

/** One tickable row of the "make this server the home of my bots" dialog. Names only. */
export interface CopyItem {
  /** `kind:id`, stable across renders. */
  key: string;
  kind: "provider" | "bot" | "default";
  id: string;
  label: string;
  /** `key` (a key goes along), `nokey`, or `model` (a bot). */
  detail: "key" | "nokey" | "model";
  model?: string | null;
  /** Why the row cannot be sent (it is shown, disabled). */
  blocked?: string | null;
}

/** The rows the dialog lists, in the order they are sent. */
export function buildCopyItems(plan: CopyPlan): CopyItem[] {
  const items: CopyItem[] = plan.providers.map((p) => ({
    key: `provider:${p.id}`,
    kind: "provider",
    id: p.id,
    label: p.id,
    detail: p.has_key ? "key" : "nokey",
    model: p.model
  }));
  for (const b of plan.bots) {
    items.push({
      key: `bot:${b.id}`,
      kind: "bot",
      id: b.id,
      label: b.name,
      detail: "model",
      model: b.model,
      blocked: b.blocked
    });
  }
  if (plan.default_provider) {
    items.push({
      key: "default:default",
      kind: "default",
      id: plan.default_provider,
      label: plan.default_model
        ? `${plan.default_provider} / ${plan.default_model}`
        : plan.default_provider,
      detail: "nokey"
    });
  }
  return items;
}

/** Everything that can be sent starts ticked. */
export function defaultChecked(items: readonly CopyItem[]): Set<string> {
  return new Set(items.filter((i) => !i.blocked).map((i) => i.key));
}

/** The request for the ticked rows. A key is only ever named by its provider id. */
export function toRequest(items: readonly CopyItem[], checked: ReadonlySet<string>): CopyRequest {
  const on = items.filter((i) => checked.has(i.key) && !i.blocked);
  return {
    providers: on.filter((i) => i.kind === "provider").map((i) => i.id),
    bots: on.filter((i) => i.kind === "bot").map((i) => i.id),
    default_model: on.some((i) => i.kind === "default"),
    set_home: on.length > 0
  };
}

export interface ReportSummary {
  done: number;
  skipped: number;
  failed: number;
  /** The home flag could not be recorded although items moved. */
  homeFailed: boolean;
}

export function summarizeReport(report: CopyReport): ReportSummary {
  const count = (s: string) =>
    report.items.filter((i) => i.kind !== "home" && i.status === s).length;
  return {
    done: count("done"),
    skipped: count("skipped"),
    failed: count("failed"),
    homeFailed: report.items.some((i) => i.kind === "home" && i.status === "failed")
  };
}

/** i18n key suffix for the per-item line of the report. */
export function resultKey(item: CopyItemResult): string {
  return item.status === "done" ? "done" : (item.code ?? item.status);
}
