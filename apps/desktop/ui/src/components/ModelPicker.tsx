import { Check, ChevronDown, Download, Plug, Search, Sparkles } from "lucide-react";
import { useCallback, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { isMockMode } from "../api/mockMode";
import {
  filterModelGroups,
  isCliDefault,
  modelHint,
  recommendedOptions,
  type ModelGroup,
  type ModelOption
} from "../lib/modelPicker";
import { productBrandKey, providerBrand } from "../lib/modelBrand";
import { modelDisplayName } from "../lib/modelName";
import { matchesQuery } from "../lib/searchText";
import { THINKING_LEVELS, useThinkingLevel } from "../lib/thinkingLevel";
import { useCliEngineRows } from "../state/useCliEngineRows";
import { BrandMark } from "./BrandMark";
import styles from "./ModelPicker.module.css";
import { Popover, type PopoverPlacement } from "./Popover";
import { useToast } from "./Toast";

export interface ModelPickerAutoOption {
  label: string;
  sub?: string;
  selected: boolean;
  onPick: () => void;
}

/** "Connect" / "Install" rows for coding CLIs; mounted only where CLI models can be chosen. */
function EngineSection({ query, testId, onInstall }: { query: string; testId: string; onInstall: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const cli = useCliEngineRows(true);
  const typed = query.trim();
  const rows = cli.rows.filter((r) => !typed || matchesQuery(query, r.name, r.id));
  if (rows.length === 0) {
    return null;
  }
  const run = async (row: (typeof rows)[number]) => {
    if (row.action === "install") {
      onInstall();
    }
    try {
      await cli.run(row);
    } catch (e) {
      toast.show(e instanceof Error && e.message ? e.message : t("modelPicker.engineError"), "danger");
    }
  };
  return (
    <div role="group" aria-label={t("modelPicker.engines")} className={styles.group}>
      <div className={styles.groupLabel} aria-hidden="true">
        <span>{t("modelPicker.engines")}</span>
      </div>
      {rows.map((r) => (
        <button
          key={`engine-${r.id}`}
          type="button"
          data-picker-row
          className={styles.option}
          data-testid={`${testId}-${r.action}-${r.id}`}
          onClick={() => void run(r)}
        >
          {r.action === "install" ? <Download aria-hidden="true" /> : <Plug aria-hidden="true" />}
          <span className={styles.optionText}>
            {t(r.action === "install" ? "modelPicker.install" : "modelPicker.connect", { name: r.name })}
          </span>
        </button>
      ))}
    </div>
  );
}

/**
 * Model popover used by the composer, the bot editor and settings.  Closed state of the list:
 * an optional "automatic" row, up to four recommended models by their real names, a collapsed
 * "All models" list (grouped by provider or CLI engine, searchable, raw id only as small
 * secondary text) and, last, rows that connect or install coding CLIs.
 * `value` and the picked option use the `provider/model` form.
 */
export function ModelPicker({
  groups,
  value,
  onPick,
  autoOption,
  disabled = false,
  placeholder,
  reviewOpen = true,
  thinking = false,
  placement = "bottom-end",
  testId = "model-picker",
  engines = false
}: {
  groups: ModelGroup[];
  value: string;
  onPick: (ref: string) => void;
  autoOption?: ModelPickerAutoOption;
  disabled?: boolean;
  placeholder?: string;
  reviewOpen?: boolean;
  /** Show the thinking-level design preview in mock mode; the daemon cannot apply it yet. */
  thinking?: boolean;
  placement?: PopoverPlacement;
  testId?: string;
  /** Offer "Connect"/"Install" rows for coding CLIs (off where CLI models cannot be chosen). */
  engines?: boolean;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(
    () => reviewOpen && isMockMode() && new URLSearchParams(window.location.search).get("screen") === "models"
  );
  const [allOpen, setAllOpen] = useState(
    () => isMockMode() && new URLSearchParams(window.location.search).get("all") === "1"
  );
  const [query, setQuery] = useState(
    () => (isMockMode() ? new URLSearchParams(window.location.search).get("modelq") ?? "" : "")
  );
  const [level, setLevel] = useThinkingLevel();
  const trigger = useRef<HTMLButtonElement | null>(null);
  const shown = useMemo(
    () =>
      filterModelGroups(groups, query).sort((a, b) => {
        const order = [groups[0]?.provider, "openai", "gemini", "ollama", "claude-code"];
        const priority = (id: string) => (order.includes(id) ? order.indexOf(id) : order.length);
        return priority(a.provider) - priority(b.provider);
      }),
    [groups, query]
  );

  // "Other model id": a CLI accepts models its short list does not show.
  const typed = query.trim();
  const otherIds = shown.length === 0 && /^[A-Za-z0-9][A-Za-z0-9._:/-]{0,127}$/.test(typed)
    ? groups.filter((g) => g.provider.startsWith("cli:") && !g.models.some((m) => m.model === typed))
    : [];

  const searching = typed !== "";
  const recommended = useMemo(() => (searching ? [] : recommendedOptions(groups)), [groups, searching]);
  const showAll = searching || allOpen;
  const total = groups.reduce((n, g) => n + g.models.length, 0);

  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
    setAllOpen(false);
  }, []);

  const pick = (ref: string) => {
    close();
    trigger.current?.focus();
    if (autoOption?.selected || ref !== value) {
      onPick(ref);
    }
  };

  const pickAuto = () => {
    if (!autoOption) return;
    close();
    trigger.current?.focus();
    autoOption.onPick();
  };

  const showAuto = Boolean(
    autoOption && (!query.trim() || matchesQuery(query, autoOption.label, autoOption.sub))
  );

  const label = (m: ModelOption) =>
    isCliDefault(m) ? t("modelPicker.cliDefault") : m.displayName;

  const [valueProvider = "", ...rest] = value.split("/");
  const valueModel = rest.join("/");
  const current = groups.find((g) => g.provider === valueProvider)?.models.find((m) => m.ref === value);
  const valueLabel = current ? label(current) : valueModel ? modelDisplayName(valueModel) : "";

  // Arrow keys walk the rows (search box -> rows -> back up to the search box).
  const moveFocus = (e: KeyboardEvent<HTMLElement>) => {
    const dir = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
    if (!dir) return;
    const items = [...e.currentTarget.querySelectorAll<HTMLElement>('[role="option"], [data-picker-row]')];
    const next = items[items.indexOf(document.activeElement as HTMLElement) + dir];
    if (next) {
      e.preventDefault();
      next.focus();
    } else if (dir < 0) {
      e.preventDefault();
      e.currentTarget.closest("[role=dialog]")?.querySelector<HTMLInputElement>("input")?.focus();
    }
  };

  const optionRow = (m: ModelOption, opts: { hint?: boolean; sub?: boolean }) => {
    const active = !autoOption?.selected && m.ref === value;
    const primary = label(m);
    const raw = opts.sub && !isCliDefault(m) && primary !== m.model ? m.model : null;
    return (
      <button
        key={`${opts.hint ? "rec" : "all"}-${m.ref}`}
        type="button"
        role="option"
        aria-selected={active}
        className={[styles.option, active ? styles.optionActive : ""].filter(Boolean).join(" ")}
        onClick={() => pick(m.ref)}
      >
        <BrandMark name={productBrandKey(m.provider, m.model)} size={16} />
        <span className={styles.optionText} title={m.model}>
          {primary}
          {raw ? <span className={styles.rawId}>{raw}</span> : null}
        </span>
        {opts.hint ? (
          <span className={styles.optionSub}>
            {m.provider.startsWith("cli:") ? providerBrand(m.provider).label : t(`modelPicker.hint.${modelHint(m.model)}`)}
          </span>
        ) : null}
        {active ? <Check aria-hidden="true" /> : null}
      </button>
    );
  };

  return (
    <div className={styles.root}>
      <button
        ref={trigger}
        type="button"
        className={styles.trigger}
        disabled={disabled}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={t("modelPicker.title")}
        data-testid={testId}
        onClick={() => (open ? close() : setOpen(true))}
      >
        {autoOption?.selected ? (
          <Sparkles aria-hidden="true" />
        ) : value ? (
          <BrandMark name={productBrandKey(valueProvider, valueModel)} size={15} />
        ) : null}
        <span className={styles.triggerText}>
          {autoOption?.selected
            ? autoOption.label
            : valueLabel ||
              placeholder ||
              t("modelPicker.none")}
        </span>
        <ChevronDown />
      </button>
      <Popover
        open={open}
        anchor={trigger}
        onClose={close}
        placement={placement}
        role="dialog"
        label={t("modelPicker.title")}
        width={320}
        initialFocus="none"
      >
        <label className={styles.search}>
          <Search aria-hidden="true" />
          <input
            value={query}
            placeholder={t("modelPicker.search")}
            aria-label={t("modelPicker.search")}
            autoFocus
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                e.currentTarget
                  .closest("[role=dialog]")
                  ?.querySelector<HTMLElement>('[role="option"], [data-picker-row]')
                  ?.focus();
              } else if (e.key === "Enter") {
                const first = shown[0]?.models[0];
                if (first && query.trim()) {
                  e.preventDefault();
                  pick(first.ref);
                } else if (showAuto && autoOption) {
                  e.preventDefault();
                  pickAuto();
                } else if (first) {
                  e.preventDefault();
                  pick(first.ref);
                }
              }
            }}
          />
        </label>
        <div className={styles.scroll} role="listbox" aria-label={t("modelPicker.title")} onKeyDown={moveFocus}>
          {shown.length === 0 && !showAuto && otherIds.length === 0 && !engines ? (
            <p className={styles.empty}>{t("modelPicker.empty")}</p>
          ) : (
            <>
              {showAuto && autoOption ? (
                <button
                  type="button"
                  role="option"
                  aria-selected={autoOption.selected}
                  className={[styles.option, autoOption.selected ? styles.optionActive : ""].filter(Boolean).join(" ")}
                  onClick={pickAuto}
                >
                  <Sparkles aria-hidden="true" />
                  <span className={styles.optionText}>{autoOption.label}</span>
                  {autoOption.sub ? <span className={styles.optionSub}>{autoOption.sub}</span> : null}
                  {autoOption.selected ? <Check aria-hidden="true" /> : null}
                </button>
              ) : null}
              {otherIds.map((g) => (
                <button
                  key={`other-${g.provider}`}
                  type="button"
                  role="option"
                  aria-selected={false}
                  className={styles.option}
                  onClick={() => pick(`${g.provider}/${typed}`)}
                >
                  <span className={styles.optionText}>
                    {t("modelPicker.otherId", { id: typed, provider: providerBrand(g.provider).label })}
                  </span>
                </button>
              ))}
              {recommended.length > 0 ? (
                <div className={styles.group} role="group" aria-label={t("modelPicker.recommended")}>
                  <div className={styles.groupLabel} aria-hidden="true">
                    <span>{t("modelPicker.recommended")}</span>
                  </div>
                  {recommended.map((m) => optionRow(m, { hint: true }))}
                </div>
              ) : null}
              {!searching && shown.length > 0 ? (
                <button
                  type="button"
                  data-picker-row
                  className={styles.allToggle}
                  aria-expanded={allOpen}
                  data-testid={`${testId}-all`}
                  onClick={() => setAllOpen((v) => !v)}
                >
                  <span>{t("modelPicker.all", { count: total })}</span>
                  <ChevronDown aria-hidden="true" className={allOpen ? styles.chevOpen : undefined} />
                </button>
              ) : null}
              {showAll
                ? shown.map((g) => {
                    const brand = providerBrand(g.provider);
                    return (
                      <div
                        key={g.provider}
                        className={styles.group}
                        role="group"
                        aria-label={`${brand.label}${brand.note ? ` ${brand.note}` : ""}`}
                      >
                        <div className={styles.groupLabel} aria-hidden="true">
                          <BrandMark name={brand.company} size={14} />
                          <span>{brand.label}</span>
                          {brand.note ? <span className={styles.groupNote}>{brand.note}</span> : null}
                        </div>
                        {g.models.map((m) => optionRow(m, { sub: true }))}
                      </div>
                    );
                  })
                : null}
              {engines ? (
                <EngineSection query={query} testId={testId} onInstall={close} />
              ) : null}
            </>
          )}
        </div>
        {thinking && isMockMode() ? (
          <div className={styles.effort}>
            <span className={styles.effortLabel} id="thinking-level-label">
              {t("modelPicker.thinking")}
            </span>
            <div
              className={styles.seg}
              role="radiogroup"
              aria-labelledby="thinking-level-label"
              onKeyDown={(e) => {
                const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
                if (!step) return;
                e.preventDefault();
                const i = THINKING_LEVELS.indexOf(level);
                const next = THINKING_LEVELS[(i + step + THINKING_LEVELS.length) % THINKING_LEVELS.length];
                if (next) {
                  setLevel(next);
                  e.currentTarget.querySelectorAll<HTMLElement>('[role="radio"]')[THINKING_LEVELS.indexOf(next)]?.focus();
                }
              }}
            >
              {THINKING_LEVELS.map((l) => (
                <button
                  key={l}
                  type="button"
                  role="radio"
                  tabIndex={l === level ? 0 : -1}
                  aria-checked={l === level}
                  className={l === level ? styles.segOn : undefined}
                  onClick={() => setLevel(l)}
                >
                  {t(`modelPicker.levels.${l}`)}
                </button>
              ))}
            </div>
          </div>
        ) : null}
      </Popover>
    </div>
  );
}
