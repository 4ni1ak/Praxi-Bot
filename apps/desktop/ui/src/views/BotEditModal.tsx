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
import { botChats } from "../lib/botLanding";
import { type BotDraft, draftFromBot, runtimeTargetLabel } from "../lib/botProfile";
import { type ProfilePart, saveEditor } from "../lib/botSave";
import {
  MAX_ASSIGNED_SKILLS,
  buildPutSkillsPayload,
  exceedsSkillLimit,
  isBotSkillsDirty
} from "../lib/botSkills";
import { botHasAccess, changedApps } from "../lib/mcpApps";
import { useDaemon } from "../state/DaemonContext";
import { useLeaveGuard } from "../state/LeaveGuard";
import { type BotSkillsState, useSkillsDraft } from "../state/useBotSkills";
import { useCustomText } from "../state/useCustomText";
import styles from "./BotEditModal.module.css";

/** The section list turns into a horizontal strip at this width (see the stylesheet). */
const STRIP_QUERY = "(width <= 52em)";

function useHorizontalTabs(): boolean {
  const [strip, setStrip] = useState(
    () => typeof window !== "undefined" && window.matchMedia?.(STRIP_QUERY).matches === true
  );
  useEffect(() => {
    const mq = window.matchMedia?.(STRIP_QUERY);
    if (!mq) return undefined;
    const on = () => setStrip(mq.matches);
    on();
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return strip;
}

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
  const { client, bots, chats, refreshBots } = useDaemon();
  const { providersDto, servers, setServers, allowed, setAllowed, initialApps, setInitialApps } =
    useBotFormDeps(client, bot.id, true);

  // The draft the dialog opened with: later refreshes of the bot list must not make an
  // untouched form look edited (or silently replace what the user typed).
  const [savedDraft, setSavedDraft] = useState<BotDraft>(() => draftFromBot(bot));
  const [draft, setDraft] = useState<BotDraft>(savedDraft);
  // The stored bot the dialog was opened on: the conflict check compares the daemon's copy with it.
  const [baseline, setBaseline] = useState<Bot>(bot);
  // Hand-typed model ids as typed (Advanced); kept here so they survive a section change.
  const { customText, setCustom, forgetFor, reset: resetCustom } = useCustomText(setDraft);
  const [conflict, setConflict] = useState<{ parts: ProfilePart[]; fresh: Bot } | null>(null);
  const pendingLeave = useRef<(() => void) | null>(null);
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

  const patch = (p: Partial<BotDraft>) => {
    setDraft((d) => ({ ...d, ...p }));
    forgetFor(p);
  };

  const skillsDirty = Boolean(
    skills.data && skillsDraft && isBotSkillsDirty(skillsDraft, skills.data)
  );
  const dirtyIn = dirtySections({
    draft,
    initialDraft: savedDraft,
    apps: allowed,
    initialApps,
    skillsDirty,
    customText
  });
  const dirty = dirtyIn.length > 0;
  const profileDirty = dirtyIn.some((s) => s === "identity" || s === "models" || s === "permissions");
  const problems = editorProblems(draft, customText);
  const nameMissing = problems.some((p) => p.problem === "nameMissing");
  const executorMissing = problems.some((p) => p.problem === "executorMissing");

  const requestClose = () => {
    const action = closeAction(dirty, saving || deleting);
    if (action === "close") onClose();
    else if (action === "confirm") {
      pendingLeave.current = null;
      setConfirmClose(true);
    }
  };
  useEffect(() => {
    closeGuard.current = requestClose;
  });
  // Route changes (another bot, a chat, the palette, the panel) ask the same question.
  useLeaveGuard({
    state: () => ({ dirty, busy: saving || deleting }),
    confirm: (proceed) => {
      pendingLeave.current = proceed;
      setConfirmClose(true);
    }
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

  const save = async (overwrite = false) => {
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
      const result = await saveEditor(client, {
        baseline,
        draft,
        openedDraft: savedDraft,
        profileDirty,
        initialApps,
        allowed,
        allBotIds: bots.map((b) => b.id),
        overwrite,
        saveSkills: skillsPayload
          ? async () => {
              const r = await skills.save(skillsPayload);
              return r.ok ? { ok: true } : { ok: false, failure: r.failure, message: r.message };
            }
          : null
      });
      if (result.kind === "conflict") {
        setConflict({ parts: result.changed, fresh: result.fresh });
        return;
      }
      setConflict(null);
      const { bot: saved, failed, notApplied } = result;
      if (result.profileWritten) {
        const next = draftFromBot(saved);
        setBaseline(saved);
        setSavedDraft(next);
        setDraft(next);
        resetCustom();
        await refreshBots().catch(() => {});
      }

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

      const sk = result.skills;
      if (sk && !sk.ok) {
        messages.push(
          sk.failure === "conflict"
            ? t("bots.skills.conflict")
            : sk.failure === "invalid"
              ? t("bots.skills.invalid", { detail: sk.message })
              : t("botEdit.skillsFailed", { detail: sk.message })
        );
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

  /** Drop the profile edits and show what is stored now; skills and app edits stay. */
  const reloadProfile = () => {
    if (!conflict) return;
    const next = draftFromBot(conflict.fresh);
    setBaseline(conflict.fresh);
    setSavedDraft(next);
    setDraft(next);
    resetCustom();
    setConflict(null);
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

  const horizontalTabs = useHorizontalTabs();
  const chatCount = botChats(chats, bot.id).length;
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
          aria-orientation={horizontalTabs ? "horizontal" : "vertical"}
          aria-label={t("botEdit.sections")}
          inert={saving || deleting}
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
          inert={saving || deleting}
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
              customText={customText}
              onCustomText={setCustom}
              computerTarget={runtimeTargetLabel(baseline.config, "computer")}
              browserTarget={runtimeTargetLabel(baseline.config, "browser")}
              chatCount={chatCount}
              onDelete={() => {
                setDeleteError(null);
                setConfirmDelete(true);
              }}
            />
          ) : null}
          {conflict ? (
            <Callout tone="warning" title={t("botEdit.conflict.title")} role="alert">
              <p>
                {t("botEdit.conflict.body", {
                  parts: conflict.parts.map((p) => t(`botEdit.conflict.part.${p}`)).join(", ")
                })}
              </p>
              <div className={styles.noticeActions}>
                <Button size="sm" variant="secondary" data-testid="bot-edit-reload" onClick={reloadProfile}>
                  {t("botEdit.conflict.reload")}
                </Button>
                <Button
                  size="sm"
                  variant="secondary"
                  data-testid="bot-edit-overwrite"
                  disabled={saving}
                  onClick={() => void save(true)}
                >
                  {t("botEdit.conflict.overwrite")}
                </Button>
              </div>
            </Callout>
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
        onCancel={() => {
          pendingLeave.current = null;
          setConfirmClose(false);
        }}
        onConfirm={() => {
          const proceed = pendingLeave.current;
          pendingLeave.current = null;
          setConfirmClose(false);
          onClose();
          proceed?.();
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
