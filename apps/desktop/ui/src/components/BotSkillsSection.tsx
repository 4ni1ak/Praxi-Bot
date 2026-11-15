import { Search, Sparkles, TriangleAlert, X } from "lucide-react";
import { useCallback, useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Card } from "ui";
import { ApiError, type ApiClient } from "../api/client";
import type { BotSkills } from "../api/types";
import {
  MAX_ASSIGNED_SKILLS,
  buildPutSkillsPayload,
  exceedsSkillLimit,
  createBotSkillsDraft,
  filterAvailableSkills,
  type BotSkillsDraft,
  removeMissingDraftSkill,
  summarizeBotSkills,
  switchSkillMode,
  toggleDraftSkill
} from "../lib/botSkills";
import formStyles from "./Form.module.css";
import { Modal } from "./Modal";
import { Segmented } from "./SettingsUi";
import { useToast } from "./Toast";
import styles from "./BotSkillsSection.module.css";

/**
 * Per-bot shared skill assignment summary card + "Choose skills" dialog.
 *
 * Skill names and descriptions originate from untrusted `SKILL.md` files and
 * are rendered strictly as plain JSX text (never HTML or Markdown).
 */
export function BotSkillsSection({
  botId,
  client
}: {
  botId: string;
  client: ApiClient | null;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const uid = useId();
  const headingId = `${uid}-title`;

  const [data, setData] = useState<BotSkills | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [modalOpen, setModalOpen] = useState(false);
  const [draft, setDraft] = useState<BotSkillsDraft | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!client) {
      return;
    }
    setLoading(true);
    setLoadError(null);
    try {
      const next = await client.getBotSkills(botId);
      setData(next);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [botId, client]);

  useEffect(() => {
    setData(null);
    setModalOpen(false);
    setDraft(null);
    void load();
  }, [load]);

  const openModal = () => {
    if (!data) {
      return;
    }
    setDraft(createBotSkillsDraft(data));
    setSaveError(null);
    setModalOpen(true);
  };

  const closeModal = () => {
    if (saving) {
      return;
    }
    setModalOpen(false);
  };

  const save = async () => {
    if (!client || !data || !draft) {
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      const payload = buildPutSkillsPayload(draft, data.available);
      if (exceedsSkillLimit(payload)) {
        const message = t("bots.skills.tooMany", { max: MAX_ASSIGNED_SKILLS });
        setSaveError(message);
        toast.show(message, "danger");
        return;
      }
      const updated = await client.putBotSkills(botId, payload);
      setData(updated);
      setModalOpen(false);
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) {
        // Someone else saved first: show their state, never overwrite it.
        setModalOpen(false);
        toast.show(t("bots.skills.conflict"), "danger");
        void load();
      } else {
        const message =
          e instanceof ApiError && e.status === 422
            ? t("bots.skills.invalid", { detail: e.message })
            : e instanceof Error
              ? e.message
              : String(e);
        setSaveError(message);
        toast.show(message, "danger");
      }
    } finally {
      setSaving(false);
    }
  };

  const summary = data ? summarizeBotSkills(data) : null;
  const filteredAvailable =
    data && draft ? filterAvailableSkills(data.available, draft.query) : [];

  return (
    <section className={styles.section} aria-labelledby={headingId}>
      <h2 id={headingId} className={styles.heading}>
        {t("bots.skills.heading")}
      </h2>
      <Card padding="md" className={styles.summaryCard}>
        <div className={styles.summaryText}>
          {loadError ? (
            <p className={formStyles.error} role="alert">
              {loadError}
            </p>
          ) : loading && !data ? (
            <p className={styles.summaryMuted}>{t("bots.skills.loading")}</p>
          ) : summary ? (
            <>
              <p className={styles.summaryLine} data-testid="bot-skills-summary">
                {summary.mode === "all"
                  ? t("bots.skills.summaryAll", { count: summary.availableCount })
                  : t("bots.skills.summarySelected", {
                      count: summary.assignedCount,
                      total: summary.availableCount
                    })}
                {summary.ownCount > 0
                  ? ` ${t("bots.skills.summaryLearned", { count: summary.ownCount })}`
                  : ""}
              </p>
              {summary.missingCount > 0 ? (
                <p className={styles.summaryWarn} role="status">
                  <TriangleAlert aria-hidden="true" />
                  {t("bots.skills.summaryMissing", {
                    count: summary.missingCount
                  })}
                </p>
              ) : null}
            </>
          ) : null}
        </div>
        {loadError ? (
          <Button
            variant="secondary"
            disabled={loading || !client}
            onClick={() => void load()}
          >
            {t("health.retry")}
          </Button>
        ) : (
          <Button
            variant="secondary"
            disabled={!data || loading}
            onClick={openModal}
            data-testid="choose-skills-button"
          >
            <Sparkles />
            {t("bots.skills.chooseButton")}
          </Button>
        )}
      </Card>

      <Modal
        open={modalOpen && data !== null && draft !== null}
        onClose={closeModal}
        label={t("bots.skills.modalTitle")}
        size="lg"
      >
        {data && draft ? (
          <div className={styles.modalRoot}>
            <div className={styles.modalHeader}>
              <h2 className={styles.modalTitle}>
                {t("bots.skills.modalTitle")}
              </h2>
              <button
                type="button"
                className={styles.close}
                aria-label={t("common.close")}
                disabled={saving}
                onClick={closeModal}
              >
                <X />
              </button>
            </div>

            <p className={formStyles.hint}>{t("bots.skills.boundaryHint")}</p>

            <div className={styles.controls}>
              <Segmented
                label={t("bots.skills.modeLabel")}
                value={draft.mode}
                onChange={(nextMode) =>
                  setDraft((cur) =>
                    cur ? switchSkillMode(cur, nextMode, data.available) : cur
                  )
                }
                options={[
                  {
                    value: "all",
                    label: t("bots.skills.modeAll"),
                    testId: "skills-mode-all"
                  },
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
                  onChange={(e) => {
                    const query = e.target.value;
                    setDraft((cur) => (cur ? { ...cur, query } : cur));
                  }}
                />
              </label>
            </div>

            <div className={styles.body}>
              {draft.mode === "selected" && draft.missing.length > 0 ? (
                <div className={styles.missingBox} role="status">
                  <p className={styles.missingHeader}>
                    <TriangleAlert aria-hidden="true" />
                    {t("bots.skills.missingTitle")}
                  </p>
                  <p className={formStyles.hint}>
                    {t("bots.skills.missingHint")}
                  </p>
                  <ul className={styles.missingList}>
                    {draft.missing.map((name) => (
                      <li key={name} className={styles.missingRow}>
                        <span className={styles.missingName}>{name}</span>
                        <button
                          type="button"
                          className={styles.removeBtn}
                          onClick={() =>
                            setDraft((cur) =>
                              cur ? removeMissingDraftSkill(cur, name) : cur
                            )
                          }
                        >
                          {t("bots.skills.removeMissing")}
                        </button>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}

              {data.available.length === 0 ? (
                <p className={formStyles.hint}>
                  {t("bots.skills.emptyCatalog")}
                </p>
              ) : filteredAvailable.length === 0 ? (
                <p className={formStyles.hint}>
                  {t("bots.skills.emptySearch")}
                </p>
              ) : (
                <ul className={styles.list}>
                  {filteredAvailable.map((skill) => {
                    const checked =
                      draft.mode === "all" || draft.selected.has(skill.name);
                    const disabled = draft.mode === "all";
                    return (
                      <li key={skill.name}>
                        <label
                          className={[
                            styles.item,
                            disabled ? styles.itemDisabled : ""
                          ]
                            .filter(Boolean)
                            .join(" ")}
                        >
                          <input
                            type="checkbox"
                            className={styles.check}
                            checked={checked}
                            disabled={disabled}
                            onChange={(e) => {
                              const nextChecked = e.target.checked;
                              setDraft((cur) =>
                                cur
                                  ? toggleDraftSkill(cur, skill.name, nextChecked)
                                  : cur
                              );
                            }}
                          />
                          <span className={styles.itemText}>
                            <span className={styles.itemTitleRow}>
                              <span className={styles.itemName}>
                                {skill.name}
                              </span>
                              <span className={styles.scopePill}>
                                {t(`bots.skills.scope.${skill.scope}`)}
                              </span>
                            </span>
                            {skill.description ? (
                              <span className={styles.itemDesc}>
                                {skill.description}
                              </span>
                            ) : null}
                          </span>
                        </label>
                      </li>
                    );
                  })}
                </ul>
              )}

              {data.own.length > 0 ? (
                <details className={styles.ownBox}>
                  <summary className={styles.ownSummary}>
                    {t("bots.skills.ownTitle", { count: data.own.length })}
                  </summary>
                  <p className={styles.ownHint}>{t("bots.skills.ownHint")}</p>
                  <ul className={styles.ownList}>
                    {data.own.map((o) => (
                      <li key={o.name}>
                        {o.version ? `${o.name} (${o.version})` : o.name}
                      </li>
                    ))}
                  </ul>
                </details>
              ) : null}

              {saveError ? (
                <p className={formStyles.error} role="alert">
                  {saveError}
                </p>
              ) : null}
            </div>

            <div className={styles.footer}>
              <Button variant="ghost" disabled={saving} onClick={closeModal}>
                {t("common.cancel")}
              </Button>
              <Button
                variant="primary"
                disabled={saving}
                onClick={() => void save()}
                data-testid="save-skills-button"
              >
                {t("common.save")}
              </Button>
            </div>
          </div>
        ) : null}
      </Modal>
    </section>
  );
}
