import { GraduationCap, Search, Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { BotSkills } from "../api/types";
import {
  type BotSkillsDraft,
  filterAvailableSkills,
  removeMissingDraftSkill,
  switchSkillMode,
  toggleDraftSkill
} from "../lib/botSkills";
import { Callout, CheckMark, dialogStyles } from "./DialogParts";
import { Segmented } from "./SettingsUi";
import styles from "./Skills.module.css";

/**
 * Which shared skills a bot may use: "all" or a picked set.  Controlled - the caller owns the
 * draft and decides when to save (the skills dialog, or the Skills section of "Edit bot").
 *
 * Skill names and descriptions come from untrusted `SKILL.md` files and are rendered strictly
 * as plain text (never HTML or Markdown).
 */
export function SkillsEditor({
  data,
  draft,
  onChange
}: {
  data: BotSkills;
  draft: BotSkillsDraft;
  onChange: (next: BotSkillsDraft) => void;
}) {
  const { t } = useTranslation();
  const shown = filterAvailableSkills(data.available, draft.query);
  const locked = draft.mode === "all";

  return (
    <>
      <div className={styles.controls}>
        <Segmented
          label={t("bots.skills.modeLabel")}
          value={draft.mode}
          onChange={(mode) => onChange(switchSkillMode(draft, mode, data.available))}
          options={[
            { value: "all", label: t("bots.skills.modeAll"), testId: "skills-mode-all" },
            {
              value: "selected",
              label: t("bots.skills.modeSelected"),
              testId: "skills-mode-selected"
            }
          ]}
        />
        <label className={styles.find}>
          <Search aria-hidden="true" />
          <input
            type="search"
            value={draft.query}
            placeholder={t("bots.skills.searchPlaceholder")}
            aria-label={t("bots.skills.searchPlaceholder")}
            onChange={(e) => onChange({ ...draft, query: e.target.value })}
          />
        </label>
      </div>

      {draft.mode === "selected" && draft.missing.length > 0 ? (
        <Callout tone="warning" title={t("bots.skills.missingTitle")} role="status">
          {t("bots.skills.missingHint")}
          <ul className={styles.missingList}>
            {draft.missing.map((name) => (
              <li key={name} className={styles.missingRow}>
                <span className={styles.missingName}>{name}</span>
                <button
                  type="button"
                  className={styles.textButton}
                  onClick={() => onChange(removeMissingDraftSkill(draft, name))}
                >
                  {t("bots.skills.removeMissing")}
                </button>
              </li>
            ))}
          </ul>
        </Callout>
      ) : null}

      {data.available.length === 0 ? (
        <p className={styles.empty}>
          <Sparkles aria-hidden="true" />
          {t("bots.skills.emptyCatalog")}
        </p>
      ) : shown.length === 0 ? (
        <p className={styles.empty}>
          <Search aria-hidden="true" />
          {t("bots.skills.emptySearch")}
        </p>
      ) : (
        <ul className={styles.list} data-testid="skills-list">
          {shown.map((skill) => {
            const checked = locked || draft.selected.has(skill.name);
            return (
              <li key={skill.name}>
                <label
                  className={[styles.item, locked ? styles.itemLocked : ""]
                    .filter(Boolean)
                    .join(" ")}
                >
                  <input
                    type="checkbox"
                    className={dialogStyles.visuallyHidden}
                    checked={checked}
                    disabled={locked}
                    onChange={(e) => onChange(toggleDraftSkill(draft, skill.name, e.target.checked))}
                  />
                  <span className={styles.itemMark}>
                    <CheckMark checked={checked} />
                  </span>
                  <span className={styles.itemText}>
                    <span className={styles.itemTop}>
                      <span className={styles.itemName}>{skill.name}</span>
                      <span className={styles.scope}>{t(`bots.skills.scope.${skill.scope}`)}</span>
                    </span>
                    {skill.description ? (
                      <span className={styles.itemDesc}>{skill.description}</span>
                    ) : null}
                  </span>
                </label>
              </li>
            );
          })}
        </ul>
      )}

      {data.own.length > 0 ? (
        <section className={`${dialogStyles.block} ${styles.own}`}>
          <div className={dialogStyles.blockHead}>
            <h3 className={dialogStyles.blockTitle}>
              {t("bots.skills.ownTitle", { count: data.own.length })}
            </h3>
            <p className={dialogStyles.blockLead}>{t("bots.skills.ownHint")}</p>
          </div>
          <ul className={styles.ownList}>
            {data.own.map((o) => (
              <li key={o.name} className={styles.ownRow}>
                <GraduationCap aria-hidden="true" />
                <span className={styles.ownName}>{o.name}</span>
                {o.version ? <span className={styles.ownVersion}>{o.version}</span> : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      <p className={dialogStyles.hint}>{t("bots.skills.boundaryHint")}</p>
    </>
  );
}
