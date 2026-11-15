import {
  Eye,
  MonitorSmartphone,
  PenLine,
  ShieldCheck,
  SlidersHorizontal,
  Trash2,
  X,
  Zap
} from "lucide-react";
import { useEffect, useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { BotAvatar, Button } from "ui";
import {
  type McpServer,
  PERMISSION_CATEGORIES,
  type PermissionLevel,
  type ProvidersDto
} from "../api/types";
import {
  addFallback,
  customRefIssue,
  fallbackList,
  removeFallback
} from "../lib/botEditor";
import {
  type BotDraft,
  formatModelRef,
  type ModelRef,
  parseModelRef,
  PRESET_CHOICES,
  type PresetChoice
} from "../lib/botProfile";
import { brandIcon } from "../lib/brandIcons";
import { modelGroups } from "../lib/modelPicker";
import { modelDisplayName, modelLine } from "../lib/modelName";
import { BotAvatarUploadField } from "./BotAdvancedFields";
import styles from "./BotEditorFields.module.css";
import { BrandMark } from "./BrandMark";
import { CharacterColorRow, CharacterShapeGrid } from "./CharacterPicker";
import { Callout, dialogStyles, Field } from "./DialogParts";
import { ModelPicker } from "./ModelPicker";
import { Segmented, SettingGroup, SettingRow, Switch } from "./SettingsUi";

type Patch = (p: Partial<BotDraft>) => void;

const LEVELS: readonly PermissionLevel[] = ["deny", "ask", "allow"] as const;

const PRESET_ICONS: Record<PresetChoice, typeof Eye> = {
  "read-only": Eye,
  "approve-destructive": ShieldCheck,
  full: Zap,
  "read-and-draft": PenLine,
  custom: SlidersHorizontal
};

const displayName = (_provider: string, model: string) => modelDisplayName(model);

/** Identity: the face, the name and how the bot behaves. */
export function IdentityFields({
  botId,
  draft,
  patch,
  nameMissing
}: {
  botId: string;
  draft: BotDraft;
  patch: Patch;
  nameMissing: boolean;
}) {
  const { t } = useTranslation();
  const uid = useId();
  return (
    <>
      <div className={styles.identityHead}>
        <BotAvatar
          bot={{
            id: botId,
            name: draft.name,
            shape: draft.shape,
            color: draft.color,
            avatar: draft.avatar
          }}
          size="lg"
          decorative
        />
        <Field
          label={t("botForm.name")}
          htmlFor={`${uid}-name`}
          error={nameMissing ? t("botForm.nameRequired") : undefined}
        >
          <input
            id={`${uid}-name`}
            className={`${dialogStyles.input} ${styles.nameInput}`}
            value={draft.name}
            maxLength={60}
            data-autofocus
            data-testid="bot-edit-name"
            aria-invalid={nameMissing ? true : undefined}
            onChange={(e) => patch({ name: e.target.value })}
          />
        </Field>
      </div>

      <section className={dialogStyles.block}>
        <h3 className={dialogStyles.blockTitle}>{t("botEdit.look")}</h3>
        <CharacterShapeGrid
          value={draft.shape}
          color={draft.color}
          onChange={(shape) => patch({ shape })}
        />
        <CharacterColorRow
          value={draft.color}
          centered={false}
          onChange={(color) => patch({ color })}
        />
        <BotAvatarUploadField avatar={draft.avatar} onChange={(avatar) => patch({ avatar })} />
      </section>

      <Field
        label={t("botForm.persona")}
        htmlFor={`${uid}-persona`}
        hint={t("botEdit.personaHint")}
      >
        <textarea
          id={`${uid}-persona`}
          className={dialogStyles.textarea}
          rows={5}
          value={draft.persona}
          placeholder={t("botForm.personaPlaceholder")}
          onChange={(e) => patch({ persona: e.target.value })}
        />
      </Field>
    </>
  );
}

/** Models: one searchable, provider-grouped picker per role - no raw ids, no native selects. */
export function ModelFields({
  draft,
  patch,
  providersDto,
  executorMissing
}: {
  draft: BotDraft;
  patch: Patch;
  providersDto: ProvidersDto | null;
  executorMissing: boolean;
}) {
  const { t } = useTranslation();
  const executorRef = formatModelRef(draft.executor);
  const plannerRef = formatModelRef(draft.planner);
  const classifierRef = formatModelRef(draft.classifier);
  const executorGroups = useMemo(
    () => modelGroups(providersDto, executorRef),
    [providersDto, executorRef]
  );
  // Planner / classifier / fallback run over HTTP providers only (as before).
  const plannerGroups = useMemo(
    () => modelGroups(providersDto, plannerRef, { includeCli: false }),
    [providersDto, plannerRef]
  );
  const classifierGroups = useMemo(
    () => modelGroups(providersDto, classifierRef, { includeCli: false }),
    [providersDto, classifierRef]
  );
  const fallbackGroups = useMemo(
    () => modelGroups(providersDto, undefined, { includeCli: false }),
    [providersDto]
  );
  const fallbacks = fallbackList(draft.fallback);
  const noProviders = providersDto !== null && executorGroups.length === 0;

  const optional = (
    role: "planner" | "classifier",
    ref: string,
    groups: ReturnType<typeof modelGroups>
  ) => (
    <SettingRow
      title={t(`botForm.role.${role}`)}
      description={t(`botEdit.roleHint.${role}`)}
    >
      <ModelPicker
        groups={groups}
        value={ref}
        labelFor={displayName}
        reviewOpen={false}
        testId={`bot-edit-${role}`}
        autoOption={{
          label: t("botEdit.sameAsMain"),
          selected: ref === "",
          onPick: () => patch({ [role]: { provider: "", model: "" } } as Partial<BotDraft>)
        }}
        onPick={(next) => patch({ [role]: parseModelRef(next) } as Partial<BotDraft>)}
      />
    </SettingRow>
  );

  return (
    <>
      {noProviders ? (
        <Callout tone="warning" title={t("botEdit.noProvidersTitle")}>
          {t("botEdit.noProvidersBody")}
        </Callout>
      ) : null}
      <SettingGroup>
        <SettingRow
          title={t("botEdit.mainModel")}
          description={
            executorMissing ? t("botForm.executorRequired") : t("botEdit.roleHint.executor")
          }
        >
          <ModelPicker
            groups={executorGroups}
            value={executorRef}
            labelFor={displayName}
            reviewOpen={false}
            placeholder={t("botEdit.pickModel")}
            testId="bot-edit-executor"
            onPick={(next) => patch({ executor: parseModelRef(next) })}
          />
        </SettingRow>
        {optional("planner", plannerRef, plannerGroups)}
        {optional("classifier", classifierRef, classifierGroups)}
      </SettingGroup>

      <section className={dialogStyles.block}>
        <div className={dialogStyles.blockHead}>
          <h3 className={dialogStyles.blockTitle}>{t("botForm.role.fallback")}</h3>
          <p className={dialogStyles.blockLead}>{t("botEdit.fallbackHint")}</p>
        </div>
        <div className={styles.fallbacks}>
          {fallbacks.map((ref, i) => {
            const line = modelLine(ref);
            return (
              <span key={ref} className={styles.fallbackChip} title={ref}>
                <span className={styles.fallbackOrder}>{i + 1}</span>
                {line.brand ? <BrandMark name={line.brand} size={14} /> : null}
                <span className={styles.fallbackName}>{line.name || ref}</span>
                <button
                  type="button"
                  className={styles.chipRemove}
                  title={t("botEdit.removeFallback", { name: line.name || ref })}
                  aria-label={t("botEdit.removeFallback", { name: line.name || ref })}
                  onClick={() => patch({ fallback: removeFallback(draft.fallback, ref) })}
                >
                  <X aria-hidden="true" />
                </button>
              </span>
            );
          })}
          <ModelPicker
            groups={fallbackGroups}
            value=""
            labelFor={displayName}
            reviewOpen={false}
            placement="bottom-start"
            placeholder={t("botEdit.addFallback")}
            testId="bot-edit-fallback"
            onPick={(next) => patch({ fallback: addFallback(draft.fallback, next) })}
          />
        </div>
      </section>
    </>
  );
}

/** Permissions: the daemon's presets as cards, and the per-category matrix for "Custom". */
export function PermissionFields({ draft, patch }: { draft: BotDraft; patch: Patch }) {
  const { t } = useTranslation();
  const group = useId();
  return (
    <>
      <div className={styles.presets} role="radiogroup" aria-label={t("botForm.permissions")}>
        {PRESET_CHOICES.map((p) => {
          const Icon = PRESET_ICONS[p];
          const active = draft.preset === p;
          return (
            <label
              key={p}
              className={[styles.preset, active ? styles.presetActive : ""].filter(Boolean).join(" ")}
              data-testid={`bot-edit-preset-${p}`}
            >
              <input
                type="radio"
                className={dialogStyles.visuallyHidden}
                name={group}
                value={p}
                checked={active}
                onChange={() => patch({ preset: p })}
              />
              <span className={styles.presetIcon}>
                <Icon aria-hidden="true" />
              </span>
              <span className={styles.presetText}>
                <span className={styles.presetName}>{t(`botForm.preset.${p}.name`)}</span>
                <span className={styles.presetHint}>{t(`botForm.preset.${p}.hint`)}</span>
              </span>
              <span className={styles.presetDot} aria-hidden="true" />
            </label>
          );
        })}
      </div>
      {draft.preset === "custom" ? (
        <section className={dialogStyles.block}>
          <div className={dialogStyles.blockHead}>
            <h3 className={dialogStyles.blockTitle}>{t("botEdit.categories")}</h3>
            <p className={dialogStyles.blockLead}>{t("botEdit.categoriesHint")}</p>
          </div>
          <SettingGroup>
            {PERMISSION_CATEGORIES.map((c) => (
              <SettingRow key={c} title={t(`botForm.category.${c}`)}>
                <Segmented
                  label={t(`botForm.category.${c}`)}
                  value={draft.categories[c]}
                  options={LEVELS.map((l) => ({
                    value: l,
                    label: t(`botForm.level.${l}`),
                    testId: `bot-edit-cat-${c}-${l}`
                  }))}
                  onChange={(level) =>
                    patch({ categories: { ...draft.categories, [c]: level } })
                  }
                />
              </SettingRow>
            ))}
          </SettingGroup>
        </section>
      ) : null}
    </>
  );
}

/** Apps: which connected apps this bot may use. */
export function AppFields({
  servers,
  allowed,
  onToggle
}: {
  servers: McpServer[] | null;
  allowed: Record<string, boolean>;
  onToggle: (serverName: string, checked: boolean) => void;
}) {
  const { t } = useTranslation();
  if (servers === null) {
    return <p className={dialogStyles.hint}>{t("botEdit.appsLoading")}</p>;
  }
  if (servers.length === 0) {
    return <Callout title={t("botForm.apps.none")}>{t("botEdit.appsNoneBody")}</Callout>;
  }
  return (
    <>
      <p className={dialogStyles.hint}>{t("botForm.apps.hint")}</p>
      <div data-testid="bot-apps">
        <SettingGroup>
          {servers.map((s) => {
            const brand = brandIcon(s.name);
            const label = brand.known ? brand.name : s.name;
            return (
              <div key={s.name} className={styles.appRow} data-testid={`bot-app-${s.name}`}>
                <BrandMark name={s.name} size={20} />
                <span className={styles.appName}>{label}</span>
                <Switch
                  checked={allowed[s.name] ?? false}
                  label={label}
                  onChange={(checked) => onToggle(s.name, checked)}
                />
              </div>
            );
          })}
        </SettingGroup>
      </div>
    </>
  );
}

function CustomIdField({
  label,
  value,
  required,
  onChange
}: {
  label: string;
  value: ModelRef;
  required: boolean;
  onChange: (ref: ModelRef) => void;
}) {
  const { t } = useTranslation();
  const uid = useId();
  const stored = formatModelRef(value);
  const [text, setText] = useState(stored);
  // A model picked elsewhere (the pickers above) replaces what is typed here.
  useEffect(() => {
    setText((cur) => (formatModelRef(parseModelRef(cur.trim())) === stored ? cur : stored));
  }, [stored]);
  const empty = text.trim() === "";
  const issue = empty && !required ? null : customRefIssue(text);
  return (
    <Field
      label={label}
      htmlFor={uid}
      error={issue ? t(`botEdit.customIssue.${issue}`) : undefined}
    >
      <input
        id={uid}
        className={`${dialogStyles.input} ${dialogStyles.mono}`}
        value={text}
        placeholder={t("botEdit.customPlaceholder")}
        spellCheck={false}
        autoCapitalize="off"
        aria-invalid={issue ? true : undefined}
        onChange={(e) => {
          const next = e.target.value;
          setText(next);
          if (next.trim() === "") {
            // Clearing an optional role is a valid edit; a cleared main model blocks Save.
            onChange({ provider: "", model: "" });
          } else if (!customRefIssue(next)) {
            onChange(parseModelRef(next.trim()));
          }
        }}
      />
    </Field>
  );
}

/** Advanced: the custom model id escape hatch, where the bot runs, and deleting it. */
export function AdvancedFields({
  draft,
  patch,
  onDelete
}: {
  draft: BotDraft;
  patch: Patch;
  /** Absent for a bot that does not exist yet. */
  onDelete?: () => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      <section className={dialogStyles.block}>
        <div className={dialogStyles.blockHead}>
          <h3 className={dialogStyles.blockTitle}>{t("botEdit.customTitle")}</h3>
          <p className={dialogStyles.blockLead}>{t("botEdit.customHint")}</p>
        </div>
        <CustomIdField
          label={t("botEdit.mainModel")}
          value={draft.executor}
          required
          onChange={(executor) => patch({ executor })}
        />
        <div className={styles.pair}>
          <CustomIdField
            label={t("botForm.role.planner")}
            value={draft.planner}
            required={false}
            onChange={(planner) => patch({ planner })}
          />
          <CustomIdField
            label={t("botForm.role.classifier")}
            value={draft.classifier}
            required={false}
            onChange={(classifier) => patch({ classifier })}
          />
        </div>
      </section>

      <section className={dialogStyles.block}>
        <div className={dialogStyles.blockHead}>
          <h3 className={dialogStyles.blockTitle}>{t("botForm.runtime")}</h3>
          <p className={dialogStyles.blockLead}>{t("botForm.runtimeHint")}</p>
        </div>
        <SettingGroup>
          <SettingRow title={t("botForm.computer")}>
            <span className={dialogStyles.chip}>
              <MonitorSmartphone aria-hidden="true" />
              <span>{t("botForm.local")}</span>
            </span>
          </SettingRow>
          <SettingRow title={t("botForm.browser")}>
            <span className={dialogStyles.chip}>
              <MonitorSmartphone aria-hidden="true" />
              <span>{t("botForm.local")}</span>
            </span>
          </SettingRow>
        </SettingGroup>
      </section>

      {onDelete ? (
        <section className={dialogStyles.block}>
          <div className={dialogStyles.blockHead}>
            <h3 className={dialogStyles.blockTitle}>{t("botEdit.dangerTitle")}</h3>
          </div>
          <div className={styles.danger}>
            <div className={styles.dangerText}>
              <p className={styles.dangerTitle}>{t("botEdit.deleteTitle")}</p>
              <p className={dialogStyles.hint}>{t("botEdit.deleteHint")}</p>
            </div>
            <Button variant="secondary" data-testid="bot-edit-delete" onClick={onDelete}>
              <Trash2 aria-hidden="true" />
              {t("botEdit.delete")}
            </Button>
          </div>
        </section>
      ) : null}
    </>
  );
}
