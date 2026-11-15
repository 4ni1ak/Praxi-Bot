import type { CliEngineDto, ProvidersDto } from "../api/types.ts";
import { modelDisplayName } from "./modelName.ts";
import { isCliProvider } from "./providerView.ts";
import { matchesQuery } from "./searchText.ts";

export interface ModelOption {
  /** `provider/model`, the form a bot profile stores. */
  ref: string;
  provider: string;
  model: string;
  displayName: string;
  recommended: boolean;
  recommendedRank: number | null;
}

export interface ModelGroup {
  provider: string;
  models: ModelOption[];
}

/**
 * Models offered by the picker, grouped by provider.  Only enabled providers
 * are listed. Legacy responses preserve configured defaults; a metadata-aware
 * response is authoritative about filtered defaults. The `current` ref remains
 * selectable even when unlisted. Enabled CLI providers expose a
 * single `default` model unless `includeCli` is `false`.  The default provider comes first.
 */
export function modelGroups(
  dto: ProvidersDto | null,
  current?: string | null,
  options?: { includeCli?: boolean }
): ModelGroup[] {
  const includeCli = options?.includeCli ?? true;
  const groups = new Map<string, Set<string>>();
  for (const p of dto?.configured ?? []) {
    if (!p.enabled) {
      continue;
    }
    const cli = isCliProvider(p.id, dto);
    if (cli && !includeCli) {
      continue;
    }
    const set = groups.get(p.id) ?? new Set<string>();
    if (p.default_model && (p.model_details === undefined
      || p.model_details.some((m) => m.id === p.default_model))) {
      set.add(p.default_model);
    }
    for (const m of p.models) {
      set.add(m);
    }
    if (cli && set.size === 0) {
      set.add("default");
    }
    groups.set(p.id, set);
  }
  const cur = current?.trim();
  if (cur) {
    const i = cur.indexOf("/");
    const provider = i < 0 ? "" : cur.slice(0, i);
    const model = i < 0 ? cur : cur.slice(i + 1);
    if (provider && (includeCli || !isCliProvider(provider, dto))) {
      const set = groups.get(provider) ?? new Set<string>();
      set.add(model);
      groups.set(provider, set);
    }
  }
  const defaultProvider = dto?.default_provider ?? null;
  return [...groups.entries()]
    .sort(([a], [b]) =>
      a === defaultProvider ? -1 : b === defaultProvider ? 1 : a.localeCompare(b)
    )
    .map(([provider, models]) => ({
      provider,
      models: [...models].map((model) => {
        const metadata = dto?.configured.find((p) => p.id === provider)
          ?.model_details?.find((m) => m.id === model);
        return {
          ref: `${provider}/${model}`,
          provider,
          model,
          displayName: metadata?.display_name ?? modelDisplayName(model),
          recommended: metadata?.recommended ?? false,
          recommendedRank: metadata?.recommended_rank ?? null
        };
      }).sort((a, b) =>
        (a.recommendedRank ?? Number.MAX_SAFE_INTEGER) - (b.recommendedRank ?? Number.MAX_SAFE_INTEGER)
        || a.displayName.localeCompare(b.displayName)
        || a.model.localeCompare(b.model)
      )
    }));
}

/** Keep the options whose provider or model matches `query`; drop empty groups. */
export function filterModelGroups(
  groups: ModelGroup[],
  query: string
): ModelGroup[] {
  return groups
    .map((g) => ({
      provider: g.provider,
      models: g.models.filter((m) => matchesQuery(query, m.provider, m.model, m.displayName))
    }))
    .filter((g) => g.models.length > 0);
}

/** Most models shown in the "Recommended" group. */
export const RECOMMENDED_MAX = 4;

/** CLI models that mean "let the CLI choose"; the picker words them as one default row. */
export function isCliDefault(option: Pick<ModelOption, "provider" | "model">): boolean {
  return option.provider.startsWith("cli:") && (option.model === "default" || option.model === "auto");
}

/**
 * Up to four recommended models of the connected providers and engines: the best-ranked model of
 * each provider first, then the next ranks, so one provider cannot fill the whole group.  The
 * "CLI decides" rows never count as a recommendation.
 */
export function recommendedOptions(groups: ModelGroup[], max = RECOMMENDED_MAX): ModelOption[] {
  const lists = groups.map((g) =>
    g.models
      .filter((m) => m.recommended && !isCliDefault(m))
      .sort((a, b) => (a.recommendedRank ?? 99) - (b.recommendedRank ?? 99))
  );
  const picked: ModelOption[] = [];
  for (let round = 0; picked.length < max && lists.some((l) => l.length > round); round += 1) {
    for (const list of lists) {
      const m = list[round];
      if (m && picked.length < max) {
        picked.push(m);
      }
    }
  }
  return picked;
}

export type ModelHint = "fast" | "balanced" | "strongest";

/** One-line hint for a recommended model, from its id (the catalog carries no hint text). */
export function modelHint(model: string): ModelHint {
  const id = model.toLowerCase();
  if (/(flash|haiku|lite|mini|nano|small|luna|:[1-8]b\b)/.test(id)) {
    return "fast";
  }
  if (/(opus|fable|-pro|carbon|sol\b|:(30|32|70)b)/.test(id)) {
    return "strongest";
  }
  return "balanced";
}

export interface EngineRow {
  id: string;
  name: string;
  /** `connect`: installed, not switched on yet. `install`: not on this computer. */
  action: "connect" | "install";
}

/**
 * Coding CLIs the picker offers below the model list: installed ones that are not enabled yet,
 * then supported ones that are not installed (they open the install flow).
 */
export function engineRows(engines: CliEngineDto[], enabledCliIds: ReadonlySet<string>): EngineRow[] {
  const rows: EngineRow[] = [];
  for (const e of engines) {
    const d = e.detected;
    if (enabledCliIds.has(d.id)) {
      continue;
    }
    if (d.installed) {
      // Only engines Praxi can run (confined, or with the CLI's own tools) can be connected.
      if (!d.runnable && !e.native) {
        continue;
      }
      rows.push({ id: d.id, name: d.display_name, action: "connect" });
    } else if (e.install) {
      rows.push({ id: d.id, name: d.display_name, action: "install" });
    }
  }
  return rows.sort((a, b) => (a.action === b.action ? 0 : a.action === "connect" ? -1 : 1));
}
