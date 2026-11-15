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
  /** Catalog capability (`chat`, `embedding`, ...); null when the catalog does not know the model. */
  kind: string | null;
  deprecated: boolean;
  note: string | null;
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
          recommendedRank: metadata?.recommended_rank ?? null,
          kind: metadata?.kind ?? null,
          deprecated: metadata?.deprecated ?? false,
          note: metadata?.note ?? null
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

/** Most models an aggregator group lists before "Show all". */
export const AGGREGATOR_FEATURED_MAX = 8;

/** CLI models that mean "let the CLI choose"; the picker words them as one default row. */
export function isCliDefault(option: Pick<ModelOption, "provider" | "model">): boolean {
  return option.provider.startsWith("cli:") && (option.model === "default" || option.model === "auto");
}

const CHAT_KINDS = new Set(["chat", "reasoning", "vision_capable"]);
const NOISE_ID =
  /(preview|experimental|(^|[-/_])exp([-_:.]|$)|embed|(^|[-/_])tts([-_:.]|$)|whisper|imagen|dall-e|moderation|rerank|-image|image-|-audio|-realtime|-transcribe|(^|[-/])veo|-(19|20)\d{2}-\d{2}-\d{2}$|-(19|20)\d{6}$|-(0[1-9]|1[0-2])[0-3]\d$)/i;

/**
 * Noise rule ("hide by default", shown only under "Show all" or by search): a model is noise when
 * the catalog says it is not a chat model (`kind` other than chat/reasoning/vision_capable), is
 * `deprecated`, or carries a `preview`/`experimental` note; and, for every model (also those the
 * catalog does not know), when its id looks like a preview/experimental build, an embedding,
 * image, tts, audio or moderation model, or a dated snapshot (`-20250514`, `-2025-05-14`, `-0613`).
 * Recommended models, CLI models and the currently selected model are never noise.
 */
export function isNoiseModel(m: ModelOption, current?: string | null): boolean {
  if (m.recommended || m.ref === current || m.provider.startsWith("cli:")) {
    return false;
  }
  if (m.kind !== null && !CHAT_KINDS.has(m.kind)) {
    return true;
  }
  if (m.deprecated || /preview|experimental|(^|[^a-z])exp([^a-z]|$)/i.test(m.note ?? "")) {
    return true;
  }
  return NOISE_ID.test(m.model);
}

/** Aggregators resell hundreds of models of other makers; the picker never lists them flat. */
const AGGREGATORS = new Set(["openrouter", "together", "litellm", "huggingface", "fireworks", "groq-router"]);
export function isAggregator(provider: string, modelCount: number): boolean {
  return AGGREGATORS.has(provider) || (!provider.startsWith("cli:") && modelCount > 40);
}

/** Model family for de-duplication: vendor prefix, dots, tags and date stamps removed. */
export function modelFamily(model: string): string {
  return model
    .slice(model.lastIndexOf("/") + 1)
    .toLowerCase()
    .replace(/:.*$/, "")
    .replace(/(-latest|-(19|20)\d{2}-\d{2}-\d{2}|-(19|20)\d{6})$/, "")
    .replace(/[._]/g, "-");
}

export interface SourceGroup {
  provider: string;
  cli: boolean;
  aggregator: boolean;
  /** Every model of the source. */
  total: number;
  /** Rows shown when the group is open. */
  featured: ModelOption[];
  /** Rows behind "Show all" (noise, or an aggregator's long tail). */
  rest: ModelOption[];
}

export interface PickerSections {
  /** One group per connected coding CLI (subscription). */
  cli: SourceGroup[];
  /** One group per API provider (billed per use). */
  api: SourceGroup[];
}

/** Split provider groups into the CLI and API sections and apply the noise / aggregator rules. */
export function pickerSections(groups: ModelGroup[], current?: string | null): PickerSections {
  const out: PickerSections = { cli: [], api: [] };
  for (const g of groups) {
    const cli = g.provider.startsWith("cli:");
    const aggregator = isAggregator(g.provider, g.models.length);
    let featured: ModelOption[];
    if (cli) {
      featured = g.models;
    } else if (aggregator) {
      const curated = g.models.filter((m) => m.recommended || m.ref === current);
      featured = (curated.length > 0 ? curated : g.models.filter((m) => !isNoiseModel(m, current)))
        .slice(0, Math.max(AGGREGATOR_FEATURED_MAX, curated.filter((m) => m.ref === current).length));
      if (current && !featured.some((m) => m.ref === current)) {
        const cur = g.models.find((m) => m.ref === current);
        if (cur) featured = [...featured, cur];
      }
    } else {
      featured = g.models.filter((m) => !isNoiseModel(m, current));
    }
    const shown = new Set(featured.map((m) => m.ref));
    const rest = g.models
      .filter((m) => !shown.has(m.ref))
      .sort((a, b) => Number(isNoiseModel(a, current)) - Number(isNoiseModel(b, current)));
    (cli ? out.cli : out.api).push({
      provider: g.provider,
      cli,
      aggregator,
      total: g.models.length,
      featured,
      rest
    });
  }
  return out;
}

/**
 * Up to four recommended models, at most one per family and source.  An aggregator's copy of a
 * model that a CLI engine or a direct provider also recommends is dropped.  Round-robin over the
 * sources so one of them cannot fill the group; the "CLI decides" rows never count.
 */
export function recommendedOptions(groups: ModelGroup[], max = RECOMMENDED_MAX): ModelOption[] {
  const direct = new Set<string>();
  for (const g of groups) {
    if (!isAggregator(g.provider, g.models.length)) {
      for (const m of g.models) {
        if (m.recommended) direct.add(modelFamily(m.model));
      }
    }
  }
  const lists = groups.map((g) => {
    const aggregator = isAggregator(g.provider, g.models.length);
    const seen = new Set<string>();
    return g.models
      .filter((m) => m.recommended && !isCliDefault(m))
      .sort((a, b) => (a.recommendedRank ?? 99) - (b.recommendedRank ?? 99))
      .filter((m) => {
        const family = modelFamily(m.model);
        if ((aggregator && direct.has(family)) || seen.has(family)) return false;
        seen.add(family);
        return true;
      });
  });
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
  if (/(flash|haiku|lite|(^|[-/:])mini|nano|small|luna|:[1-8]b\b)/.test(id)) {
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
