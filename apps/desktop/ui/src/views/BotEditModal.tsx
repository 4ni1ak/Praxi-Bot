import {
  AppWindow,
  BrainCircuit,
  Plus,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  UserRound
} from "lucide-react";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  type MutableRefObject,
  useEffect,
  useId,
  useRef,
  useState
} from "react";
import { useTranslation } from "react-i18next";
import { BotAvatar, Button } from "ui";
import type { Bot } from "../api/types";
import { useBotFormDeps } from "../components/BotAdvancedFields";
import {
  AdvancedFields,
  AppFields,
  IdentityFields,
  ModelFields,
  PermissionFields
} from "../components/BotEditorFields";
import {
  Callout,
  ConfirmDialog,
  DialogFooter,
  DialogHeader,
  dialogStyles
} from "../components/DialogParts";
import { Modal } from "../components/Modal";
import { SkillsEditor } from "../components/SkillsEditor";
import { useToast } from "../components/Toast";
import {
  BOT_EDITOR_SECTIONS,
  type BotEditorSection,
  closeAction,
  dirtySections,
  editorProblems,
  stepSection
} from "../lib/botEditor";
import { type BotDraft, configFromDraft, draftFromBot } from "../lib/botProfile";
import {
  MAX_ASSIGNED_SKILLS,
  buildPutSkillsPayload,
  exceedsSkillLimit,
  isBotSkillsDirty
} from "../lib/botSkills";
import { botHasAccess, changedApps, saveBotAndApps } from "../lib/mcpApps";
import { useDaemon } from "../state/DaemonContext";
import { type BotSkillsState, useSkillsDraft } from "../state/useBotSkills";
import styles from "./BotEditModal.module.css";

const SECTION_ICONS: Record<BotEditorSection, typeof UserRound> = {
  identity: UserRound,
  models: BrainCircuit,
  permissions: ShieldCheck,
  skills: Sparkles,
  apps: AppWindow,
  advanced: SlidersHorizontal
};

/**
 * "Edit bot" as a dialog (owner feedback 244, point 5): sections on the left, one scrolling
 * body, Cancel / Save always in view.  Replaces the full-page form.
 */
export function BotEditModal({
  open,
  bot,
  skills,
  initialSection,
  onClose,
  onDeleted,
  onAddSkill
}: {
  open: boolean;
  bot: Bot | undefined;
  skills: BotSkillsState;
  initialSection?: BotEditorSection;
  onClose: () => void;
  onDeleted: (botId: string) => void;
  onAddSkill: () => void;
}) {
  const { t } = useTranslation();
  const closeGuard = useRef<() => void>(onClose);
  const shown = open && bot !== undefined;
  return (
    <Modal open={shown} onClose={() => closeGuard.current()} label={t("botForm.editTitle")} size="lg">
      {shown && bot ? (
        <BotEditDialog
          key={bot.id}
          bot={bot}
          skills={skills}
          initialSection={initialSection}
          closeGuard={closeGuard}
          onClose={onClose}
          onDeleted={onDeleted}
          onAddSkill={onAddSkill}
        />
      ) : null}
    </Modal>
  );
}

function BotEditDialog({
  bot,
  skills,
  initialSection,
  closeGuard,
  onClose,
  onDeleted,
  onAddSkill
}: {
  bot: Bot;
  skills: BotSkillsState;
  initialSection?: BotEditorSection;
  /** What the Modal calls on Esc / backdrop press. */
  closeGuard: MutableRefObject<() => void>;
  onClose: () => void;
  onDeleted: (botId: string) => void;
  onAddSkill: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const uid = useId();
  const { client, bots, conn, refreshBots } = useDaemon();
  const { providersDto, servers, setServers, allowed, setAllowed, initialApps, setInitialApps } =
    useBotFormDeps(client, bot.id, Boolean(conn?.fake_provider), true);

  // The draft the dialog opened with: later refreshes of the bot list must not make an
  // untouched form look edited (or silently replace what the user typed).
  const [savedDraft, setSavedDraft] = useState<BotDraft>(() => draftFromBot(bot));
  const [draft, setDraft] = useState<BotDraft>(savedDraft);
  const [section, setSection] = useState<BotEditorSection>(initialSection ?? "identity");
  const [skillsDraft, setSkillsDraft] = useSkillsDraft(skills.data);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmClose, setConfirmClose] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const tabRefs = useRef<Partial<Record<BotEditorSection, HTMLButtonElement | null>>>({});

  const patch = (p: Partial<BotDraft>) => setDraft((d) => ({ ...d, ...p }));

  const skillsDirty = Boolean(
    skills.data && skillsDraft && isBotSkillsDirty(skillsDraft, skills.data)
  );
  const dirtyIn = dirtySections({
    draft,
    initialDraft: savedDraft,
    apps: allowed,
    initialApps,
    skillsDirty
  });
  const dirty = dirtyIn.length > 0;
  const problems = editorProblems(draft);
  const nameMissing = problems.some((p) => p.problem === "nameMissing");
  const executorMissing = problems.some((p) => p.problem === "executorMissing");

  const requestClose = () => {
    const action = closeAction(dirty, saving || deleting);
    if (action === "close") onClose();
    else if (action === "confirm") setConfirmClose(true);
  };
  useEffect(() => {
    closeGuard.current = requestClose;
  });

  const goTo = (next: BotEditorSection) => {
    setSection(next);
    bodyRef.current?.scrollTo({ top: 0 });
  };

  const onTabKey = (e: ReactKeyboardEvent<HTMLElement>) => {
    const next = stepSection(section, e.key);
    if (!next) return;
    e.preventDefault();
    goTo(next);
    tabRefs.current[next]?.focus();
  };

  const save = async () => {
    if (!client || saving) return;
    const first = problems[0];
    if (first) {
      // Show the reason instead of a dead button.
      goTo(first.section);
      return;
    }
    const skillsPayload =
      skillsDirty && skills.data && skillsDraft
        ? buildPutSkillsPayload(skillsDraft, skills.data.available)
        : null;
    if (skillsPayload && exceedsSkillLimit(skillsPayload)) {
      goTo("skills");
      setError(t("bots.skills.tooMany", { max: MAX_ASSIGNED_SKILLS }));
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const config = configFromDraft(draft, bot);
      const input = { name: draft.name.trim(), config };
      const { bot: saved, failed, notApplied } = await saveBotAndApps(
        client,
        bot,
        input,
        initialApps,
        allowed,
        bots.map((b) => b.id)
      );
      setSavedDraft(draft);
      await refreshBots().catch(() => {});

      const messages: string[] = [];
      if (failed.length > 0 || notApplied.length > 0) {
        const fresh = await client.listMcpServers().catch(() => null);
        const problem = new Set([...failed, ...notApplied]);
        const actual = (name: string): boolean | undefined => {
          const srv = fresh?.find((x) => x.name === name);
          return srv ? botHasAccess(srv, saved.id) : undefined;
        };
        if (fresh) setServers(fresh);
        setAllowed((prev) => {
          const next = { ...prev };
          for (const name of notApplied) next[name] = actual(name) ?? prev[name] ?? false;
          return next;
        });
        setInitialApps((prev) => {
          const next = { ...prev };
          for (const name of changedApps(prev, allowed)) {
            if (notApplied.includes(name)) {
              next[name] = actual(name) ?? prev[name] ?? false;
            } else if (!problem.has(name)) {
              next[name] = allowed[name] ?? false;
            }
          }
          return next;
        });
        if (failed.length > 0) {
          messages.push(t("botForm.apps.partialFail", { names: failed.join(", ") }));
        }
        if (notApplied.length > 0) {
          messages.push(t("botForm.apps.notApplied", { names: notApplied.join(", ") }));
        }
      } else {
        setInitialApps(allowed);
      }

      if (skillsPayload) {
        const result = await skills.save(skillsPayload);
        if (!result.ok) {
          messages.push(
            result.failure === "conflict"
              ? t("bots.skills.conflict")
              : result.failure === "invalid"
                ? t("bots.skills.invalid", { detail: result.message })
                : t("botEdit.skillsFailed", { detail: result.message })
          );
        }
      }

      if (messages.length > 0) {
        setError(messages.join(" "));
        return;
      }
      toast.show(t("botEdit.saved", { name: saved.name }), "success");
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!client || deleting) return;
    setDeleting(true);
    setDeleteError(null);
    try {
      await client.deleteBot(bot.id);
      await refreshBots().catch(() => {});
      toast.show(t("botEdit.deleted", { name: bot.name }), "info");
      onDeleted(bot.id);
    } catch (e) {
      setDeleteError(e instanceof Error ? e.message : String(e));
      setDeleting(false);
    }
  };

  const panelId = `${uid}-panel`;
  const tabId = (s: BotEditorSection) => `${uid}-tab-${s}`;

  return (
    <div className={styles.shell} data-testid="bot-edit-dialog">
      <nav className={styles.nav} aria-label={t("botForm.editTitle")}>
        <div className={styles.navBot}>
          <BotAvatar
            bot={{
              id: bot.id,
              name: draft.name,
              shape: draft.shape,
              color: draft.color,
              avatar: draft.avatar
            }}
            size="sm"
            decorative
          />
          <span className={styles.navBotName}>{draft.name.trim() || bot.name}</span>
        </div>
        <div
          className={styles.tabs}
          role="tablist"
          aria-orientation="vertical"
          aria-label={t("botEdit.sections")}
          onKeyDown={onTabKey}
        >
          {BOT_EDITOR_SECTIONS.map((s) => {
            const Icon = SECTION_ICONS[s];
            const active = s === section;
            const blocked = problems.some((p) => p.section === s);
            const changed = dirtyIn.includes(s);
            return (
              <button
                key={s}
                ref={(el) => {
                  tabRefs.current[s] = el;
                }}
                type="button"
                role="tab"
                id={tabId(s)}
                aria-selected={active}
                aria-controls={panelId}
                tabIndex={active ? 0 : -1}
                data-testid={`bot-edit-tab-${s}`}
                className={[styles.tab, active ? styles.tabActive : ""].filter(Boolean).join(" ")}
                onClick={() => goTo(s)}
              >
                <Icon aria-hidden="true" />
                <span className={styles.tabLabel}>{t(`botEdit.section.${s}`)}</span>
                {blocked ? (
                  <span className={`${styles.mark} ${styles.markProblem}`} title={t("botEdit.needsAttention")}>
                    <span className={dialogStyles.visuallyHidden}>{t("botEdit.needsAttention")}</span>
                  </span>
                ) : changed ? (
                  <span className={styles.mark} title={t("botEdit.unsaved")}>
                    <span className={dialogStyles.visuallyHidden}>{t("botEdit.unsaved")}</span>
                  </span>
                ) : null}
              </button>
            );
          })}
        </div>
      </nav>

      <div className={dialogStyles.root}>
        <DialogHeader
          title={t(`botEdit.section.${section}`)}
          subtitle={t(`botEdit.sectionHint.${section}`)}
          closeDisabled={saving || deleting}
          onClose={requestClose}
          actions={
            section === "skills" ? (
              <Button size="sm" variant="secondary" onClick={onAddSkill}>
                <Plus aria-hidden="true" />
                {t("addSkill.open")}
              </Button>
            ) : undefined
          }
        />
        <div
          ref={bodyRef}
          className={dialogStyles.body}
          role="tabpanel"
          id={panelId}
          aria-labelledby={tabId(section)}
          data-testid={`bot-edit-panel-${section}`}
        >
          {section === "identity" ? (
            <IdentityFields botId={bot.id} draft={draft} patch={patch} nameMissing={nameMissing} />
          ) : null}
          {section === "models" ? (
            <ModelFields
              draft={draft}
              patch={patch}
              providersDto={providersDto}
              executorMissing={executorMissing}
            />
          ) : null}
          {section === "permissions" ? <PermissionFields draft={draft} patch={patch} /> : null}
          {section === "skills" ? (
            skills.error && !skills.data ? (
              <Callout tone="danger" title={t("bots.skills.loadFailed")} role="alert">
                {skills.error}
              </Callout>
            ) : !skills.data || !skillsDraft ? (
              <p className={dialogStyles.hint}>{t("bots.skills.loading")}</p>
            ) : (
              <SkillsEditor data={skills.data} draft={skillsDraft} onChange={setSkillsDraft} />
            )
          ) : null}
          {section === "apps" ? (
            <AppFields
              servers={servers}
              allowed={allowed}
              onToggle={(name, checked) => setAllowed((a) => ({ ...a, [name]: checked }))}
            />
          ) : null}
          {section === "advanced" ? (
            <AdvancedFields
              draft={draft}
              patch={patch}
              onDelete={() => {
                setDeleteError(null);
                setConfirmDelete(true);
              }}
            />
          ) : null}
          {error ? (
            <Callout tone="danger" role="alert">
              {error}
            </Callout>
          ) : null}
        </div>
        <DialogFooter
          noteTone={problems.length > 0 ? "warning" : "info"}
          note={
            problems[0] ? (
              t(`botEdit.problem.${problems[0].problem}`)
            ) : dirty ? (
              <>
                <i className={dialogStyles.dirtyDot} aria-hidden="true" />
                {t("botEdit.unsaved")}
              </>
            ) : undefined
          }
        >
          <Button variant="ghost" disabled={saving || deleting} onClick={requestClose}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="primary"
            disabled={saving || deleting || !dirty}
            data-testid="bot-edit-save"
            onClick={() => void save()}
          >
            {saving ? t("botEdit.saving") : t("common.save")}
          </Button>
        </DialogFooter>
      </div>

      <ConfirmDialog
        open={confirmClose}
        title={t("botEdit.discardTitle")}
        body={t("botEdit.discardBody")}
        confirmLabel={t("botEdit.discard")}
        cancelLabel={t("botEdit.keepEditing")}
        tone="danger"
        onCancel={() => setConfirmClose(false)}
        onConfirm={() => {
          setConfirmClose(false);
          onClose();
        }}
      />
      <ConfirmDialog
        open={confirmDelete}
        title={t("botEdit.deleteConfirmTitle", { name: bot.name })}
        body={t("botEdit.deleteConfirmBody")}
        confirmLabel={t("botEdit.delete")}
        tone="danger"
        busy={deleting}
        error={deleteError}
        onCancel={() => setConfirmDelete(false)}
        onConfirm={() => void remove()}
      />
    </div>
  );
}
