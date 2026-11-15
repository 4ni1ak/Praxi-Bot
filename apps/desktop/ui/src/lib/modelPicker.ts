import type { ProvidersDto } from "../api/types.ts";
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
          displayName: metadata?.display_name ?? model,
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
