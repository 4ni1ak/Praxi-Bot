import { Check, ChevronRight, Copy, FileUp, RefreshCw } from "lucide-react";
import { type ChangeEvent, type MutableRefObject, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import type { ApiClient } from "../api/client";
import { closeAction } from "../lib/botEditor";
import { skillDraftDirty } from "../lib/skillDraft";
import { payloadWithSkill } from "../lib/botSkills";
import {
  buildSkillMd,
  checkSkillInstalled,
  dataDirFromDoctor,
  emptySkillDraft,
  issueFor,
  parseSkillMd,
  SKILL_DESCRIPTION_MAX,
  SKILL_MAX_BYTES,
  SKILL_NAME_MAX,
  type CatalogSkill,
  type SkillCheck,
  type SkillDraft,
  skillInstallPath,
  suggestSkillName,
  validateSkillDraft
} from "../lib/skillDraft";
import { useLeaveGuard } from "../state/LeaveGuard";
import type { BotSkillsState } from "../state/useBotSkills";
import {
  Callout,
  ConfirmDialog,
  DialogFooter,
  DialogHeader,
  dialogStyles,
  Field
} from "./DialogParts";
import { Modal } from "./Modal";
import { Segmented } from "./SettingsUi";
import styles from "./Skills.module.css";
import { useToast } from "./Toast";

const COPIED_MS = 1600;

/**
 * Add a skill (owner feedback 244, point 4): write one, or read an existing `SKILL.md`, checked
 * with the daemon's own rules.
 *
 * The daemon cannot store a skill for the app yet (no `POST /v1/skills`), so the last step is
 * honest about it: the finished file, where it belongs, and a check against the real catalog
 * (`GET /v1/skills`, which re-scans the skills folder).  Nothing here pretends to install.
 */
export function AddSkillModal({
  open,
  client,
  botName,
  skills,
  initialStep,
  onClose
}: {
  open: boolean;
  client: ApiClient | null;
  botName: string;
  skills: BotSkillsState;
  /** Review hook (mock mode): open directly on the install step. */
  initialStep?: "compose" | "install";
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const closeGuard = useRef<() => void>(onClose);
  return (
    <Modal open={open} onClose={() => closeGuard.current()} label={t("addSkill.title")} size="md">
      {open ? (
        <AddSkillDialog
          client={client}
          botName={botName}
          skills={skills}
          initialStep={initialStep}
          closeGuard={closeGuard}
          onClose={onClose}
        />
      ) : null}
    </Modal>
  );
}

function useCopied(): [string | null, (key: string, text: string) => void] {
  const [copied, setCopied] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = (key: string, text: string) => {
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopied(key);
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(null), COPIED_MS);
      })
      .catch(() => {});
  };
  return [copied, copy];
}

function AddSkillDialog({
  client,
  botName,
  skills,
  initialStep,
  closeGuard,
  onClose
}: {
  client: ApiClient | null;
  botName: string;
  skills: BotSkillsState;
  initialStep?: "compose" | "install";
  closeGuard: MutableRefObject<() => void>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const uid = useId();
  const fileRef = useRef<HTMLInputElement | null>(null);

  const [step, setStep] = useState<"compose" | "install">(initialStep ?? "compose");
  const [source, setSource] = useState<"write" | "file">("write");
  const [draft, setDraft] = useState<SkillDraft>(() =>
    initialStep === "install"
      ? {
          ...emptySkillDraft(),
          name: t("addSkill.namePlaceholder"),
          description: t("addSkill.descriptionPlaceholder"),
          body: t("addSkill.bodyPlaceholder")
        }
      : emptySkillDraft()
  );
  const [touched, setTouched] = useState(false);
  const [fileName, setFileName] = useState<string | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const [triggersOpen, setTriggersOpen] = useState(false);
  const [confirmClose, setConfirmClose] = useState(false);
  const [dataDir, setDataDir] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [check, setCheck] = useState<{
    state: SkillCheck | "failed";
    message?: string;
    existing?: CatalogSkill;
  } | null>(null);
  const [assigning, setAssigning] = useState(false);
  const [assigned, setAssigned] = useState(false);
  const [copied, copy] = useCopied();

  const patch = (p: Partial<SkillDraft>) => {
    setTouched(true);
    setDraft((d) => ({ ...d, ...p }));
  };

  const existing = skills.data?.available.map((s) => s.name) ?? [];
  const issues = validateSkillDraft(draft, existing);
  const nameIssue = issueFor(issues, "name");
  const descriptionIssue = issueFor(issues, "description");
  const bodyIssue = issueFor(issues, "body");
  const versionIssue = issueFor(issues, "version");
  const suggestion = nameIssue && nameIssue !== "nameTaken" ? suggestSkillName(draft.name) : "";
  const skillName = draft.name.trim();
  // Unsaved until the daemon's catalog lists this very file: moving on to the install step stores
  // nothing, so leaving it still asks.
  const dirty = skillDraftDirty({ touched, installed: check?.state === "installed" });
  const pendingLeave = useRef<(() => void) | null>(null);

  const requestClose = () => {
    const action = closeAction(dirty, checking || assigning);
    if (action === "close") onClose();
    else if (action === "confirm") {
      pendingLeave.current = null;
      setConfirmClose(true);
    }
  };
  useEffect(() => {
    closeGuard.current = requestClose;
  });
  useLeaveGuard({
    state: () => ({ dirty, busy: checking || assigning }),
    confirm: (proceed) => {
      pendingLeave.current = proceed;
      setConfirmClose(true);
    }
  });

  // The real data directory, for the path the user needs (`GET /v1/doctor`, best effort).
  useEffect(() => {
    if (step !== "install" || !client) return undefined;
    let cancelled = false;
    client
      .doctor()
      .then((report) => {
        if (!cancelled) setDataDir(dataDirFromDoctor(report));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [step, client]);

  const onFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    setFileName(file.name);
    setFileError(null);
    if (file.size > SKILL_MAX_BYTES) {
      setFileError(t("addSkill.parse.tooLarge"));
      return;
    }
    try {
      const parsed = parseSkillMd(await file.text());
      if (!parsed.ok) {
        setFileError(t(`addSkill.parse.${parsed.error}`, { detail: parsed.detail ?? "" }));
        return;
      }
      setDraft(parsed.draft);
      setTouched(true);
      setTriggersOpen(
        Boolean(parsed.draft.domains || parsed.draft.apps || parsed.draft.keywords)
      );
    } catch {
      setFileError(t("addSkill.parse.unreadable"));
    }
  };

  const runCheck = async () => {
    if (!client || checking) return;
    setChecking(true);
    try {
      const catalog = await client.listSkills();
      const result = checkSkillInstalled(draft, catalog);
      setCheck(result);
      if (result.state === "installed") {
        // The bot's own view (and "all shared skills" bots) picks it up from the same scan.
        void skills.reload();
      }
    } catch (e) {
      setCheck({ state: "failed", message: e instanceof Error ? e.message : String(e) });
    } finally {
      setChecking(false);
    }
  };

  const assignPayload = skills.data ? payloadWithSkill(skills.data, skillName) : null;
  const assign = async () => {
    if (!assignPayload || assigning) return;
    setAssigning(true);
    const result = await skills.save(assignPayload);
    setAssigning(false);
    if (result.ok) {
      setAssigned(true);
    } else {
      toast.show(
        result.failure === "conflict" ? t("bots.skills.conflict") : result.message,
        "danger"
      );
    }
  };

  const content = buildSkillMd(draft);
  const path = skillInstallPath(dataDir, skillName);

  return (
    <div className={dialogStyles.root} data-testid="add-skill-dialog">
      <DialogHeader
        title={t("addSkill.title")}
        subtitle={step === "compose" ? t("addSkill.subtitle") : t("addSkill.installSubtitle")}
        onClose={requestClose}
      />

      {step === "compose" ? (
        <div className={dialogStyles.body}>
          <div className={styles.sourceRow}>
            <Segmented
              label={t("addSkill.source")}
              value={source}
              onChange={setSource}
              options={[
                { value: "write", label: t("addSkill.sourceWrite"), testId: "add-skill-write" },
                { value: "file", label: t("addSkill.sourceFile"), testId: "add-skill-file" }
              ]}
            />
          </div>

          {source === "file" ? (
            <div className={dialogStyles.block}>
              <div className={styles.fileRow}>
                <input
                  ref={fileRef}
                  type="file"
                  accept=".md,.markdown,text/markdown,text/plain"
                  className={styles.hiddenFile}
                  onChange={(e) => void onFile(e)}
                />
                <Button variant="secondary" onClick={() => fileRef.current?.click()}>
                  <FileUp aria-hidden="true" />
                  {t("addSkill.chooseFile")}
                </Button>
                <span className={styles.fileName}>{fileName ?? t("addSkill.fileHint")}</span>
              </div>
              {fileError ? (
                <Callout tone="danger" title={t("addSkill.parse.title")} role="alert">
                  {fileError}
                </Callout>
              ) : (
                <p className={dialogStyles.hint}>{t("addSkill.fileNote")}</p>
              )}
            </div>
          ) : null}

          <div className={styles.pair}>
            <Field
              label={t("addSkill.name")}
              htmlFor={`${uid}-name`}
              count={skillName.length}
              max={SKILL_NAME_MAX}
              hint={t("addSkill.nameHint")}
              error={
                touched && nameIssue ? (
                  <span className={styles.suggest}>
                    {t(`addSkill.issue.${nameIssue}`)}
                    {suggestion && suggestion !== draft.name ? (
                      <button
                        type="button"
                        className={styles.textButton}
                        onClick={() => patch({ name: suggestion })}
                      >
                        {t("addSkill.useSuggestion", { name: suggestion })}
                      </button>
                    ) : null}
                  </span>
                ) : undefined
              }
            >
              <input
                id={`${uid}-name`}
                className={`${dialogStyles.input} ${dialogStyles.mono}`}
                value={draft.name}
                placeholder={t("addSkill.namePlaceholder")}
                spellCheck={false}
                autoCapitalize="off"
                data-autofocus
                data-testid="add-skill-name"
                aria-invalid={touched && nameIssue ? true : undefined}
                onChange={(e) => patch({ name: e.target.value })}
              />
            </Field>
            <Field
              label={t("addSkill.version")}
              htmlFor={`${uid}-version`}
              error={touched && versionIssue ? t(`addSkill.issue.${versionIssue}`) : undefined}
            >
              <input
                id={`${uid}-version`}
                className={`${dialogStyles.input} ${dialogStyles.mono}`}
                value={draft.version}
                spellCheck={false}
                aria-invalid={touched && versionIssue ? true : undefined}
                onChange={(e) => patch({ version: e.target.value })}
              />
            </Field>
          </div>

          <Field
            label={t("addSkill.description")}
            htmlFor={`${uid}-description`}
            count={[...draft.description.trim()].length}
            max={SKILL_DESCRIPTION_MAX}
            hint={t("addSkill.descriptionHint")}
            error={touched && descriptionIssue ? t(`addSkill.issue.${descriptionIssue}`) : undefined}
          >
            <input
              id={`${uid}-description`}
              className={dialogStyles.input}
              value={draft.description}
              placeholder={t("addSkill.descriptionPlaceholder")}
              data-testid="add-skill-description"
              aria-invalid={touched && descriptionIssue ? true : undefined}
              onChange={(e) => patch({ description: e.target.value })}
            />
          </Field>

          <Field
            label={t("addSkill.body")}
            htmlFor={`${uid}-body`}
            hint={t("addSkill.bodyHint")}
            error={touched && bodyIssue ? t(`addSkill.issue.${bodyIssue}`) : undefined}
          >
            <textarea
              id={`${uid}-body`}
              className={`${dialogStyles.textarea} ${dialogStyles.mono}`}
              rows={8}
              value={draft.body}
              placeholder={t("addSkill.bodyPlaceholder")}
              spellCheck={false}
              data-testid="add-skill-body"
              aria-invalid={touched && bodyIssue ? true : undefined}
              onChange={(e) => patch({ body: e.target.value })}
            />
          </Field>

          <button
            type="button"
            className={styles.disclosure}
            aria-expanded={triggersOpen}
            onClick={() => setTriggersOpen((v) => !v)}
          >
            <ChevronRight aria-hidden="true" />
            {t("addSkill.triggers")}
          </button>
          {triggersOpen ? (
            <div className={dialogStyles.block}>
              <p className={dialogStyles.hint}>{t("addSkill.triggersHint")}</p>
              <Field label={t("addSkill.domains")} htmlFor={`${uid}-domains`}>
                <input
                  id={`${uid}-domains`}
                  className={dialogStyles.input}
                  value={draft.domains}
                  placeholder={t("addSkill.domainsPlaceholder")}
                  spellCheck={false}
                  onChange={(e) => patch({ domains: e.target.value })}
                />
              </Field>
              <Field label={t("addSkill.apps")} htmlFor={`${uid}-apps`}>
                <input
                  id={`${uid}-apps`}
                  className={dialogStyles.input}
                  value={draft.apps}
                  placeholder={t("addSkill.appsPlaceholder")}
                  spellCheck={false}
                  onChange={(e) => patch({ apps: e.target.value })}
                />
              </Field>
              <Field label={t("addSkill.keywords")} htmlFor={`${uid}-keywords`}>
                <input
                  id={`${uid}-keywords`}
                  className={dialogStyles.input}
                  value={draft.keywords}
                  placeholder={t("addSkill.keywordsPlaceholder")}
                  onChange={(e) => patch({ keywords: e.target.value })}
                />
              </Field>
            </div>
          ) : null}
        </div>
      ) : (
        <div className={dialogStyles.body}>
          <Callout title={t("addSkill.boundaryTitle")}>{t("addSkill.boundaryBody")}</Callout>
          <ol className={styles.steps}>
            <li className={styles.step}>
              <div className={styles.stepBody}>
                <p className={styles.stepTitle}>{t("addSkill.stepCopy")}</p>
                <pre className={styles.preview} data-testid="add-skill-preview">{content}</pre>
                <div className={styles.stepActions}>
                  <Button
                    size="sm"
                    variant="secondary"
                    data-testid="add-skill-copy"
                    onClick={() => copy("content", content)}
                  >
                    {copied === "content" ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
                    {copied === "content" ? t("addSkill.copied") : t("addSkill.copyFile")}
                  </Button>
                </div>
              </div>
            </li>
            <li className={styles.step}>
              <div className={styles.stepBody}>
                <p className={styles.stepTitle}>{t("addSkill.stepSave")}</p>
                <div className={styles.path}>
                  <code data-testid="add-skill-path">{path}</code>
                  <button
                    type="button"
                    className={dialogStyles.iconButton}
                    title={t("addSkill.copyPath")}
                    aria-label={t("addSkill.copyPath")}
                    onClick={() => copy("path", path)}
                  >
                    {copied === "path" ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
                  </button>
                </div>
                <p className={dialogStyles.hint}>
                  {dataDir ? t("addSkill.pathHint") : t("addSkill.pathHintRelative")}
                </p>
              </div>
            </li>
            <li className={styles.step}>
              <div className={styles.stepBody}>
                <p className={styles.stepTitle}>{t("addSkill.stepCheck")}</p>
                <div className={styles.stepActions}>
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={checking || !client}
                    data-testid="add-skill-check"
                    onClick={() => void runCheck()}
                  >
                    <RefreshCw aria-hidden="true" />
                    {checking ? t("addSkill.checking") : t("addSkill.check")}
                  </Button>
                </div>
                {check?.state === "installed" ? (
                  <Callout tone="success" title={t("addSkill.found", { name: skillName })} role="status">
                    {assigned
                      ? t("addSkill.assigned", { bot: botName })
                      : assignPayload
                        ? t("addSkill.foundPicked", { bot: botName })
                        : t("addSkill.foundAll", { bot: botName })}
                    {assignPayload && !assigned ? (
                      <div className={styles.stepActions}>
                        <Button
                          size="sm"
                          variant="primary"
                          disabled={assigning}
                          data-testid="add-skill-assign"
                          onClick={() => void assign()}
                        >
                          {t("addSkill.assign", { bot: botName })}
                        </Button>
                      </div>
                    ) : null}
                  </Callout>
                ) : check?.state === "rejected" ? (
                  <Callout tone="danger" title={t("addSkill.rejected")} role="alert">
                    {check.message}
                  </Callout>
                ) : check?.state === "different" ? (
                  <Callout tone="warning" title={t("addSkill.different")} role="alert">
                    {t("addSkill.differentBody", {
                      name: skillName,
                      version: check.existing?.version ?? "?",
                      scope: check.existing?.scope ?? "shared"
                    })}
                  </Callout>
                ) : check?.state === "notFound" ? (
                  <Callout tone="warning" title={t("addSkill.notFound")} role="status">
                    {t("addSkill.notFoundBody")}
                  </Callout>
                ) : check?.state === "failed" ? (
                  <Callout tone="danger" title={t("addSkill.checkFailed")} role="alert">
                    {check.message}
                  </Callout>
                ) : null}
              </div>
            </li>
          </ol>
        </div>
      )}

      <DialogFooter>
        {step === "compose" ? (
          <>
            <Button variant="ghost" onClick={requestClose}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={issues.length > 0}
              data-testid="add-skill-continue"
              onClick={() => {
                setCheck(null);
                setStep("install");
              }}
            >
              {t("addSkill.continue")}
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" disabled={checking || assigning} onClick={() => setStep("compose")}>
              {t("common.back")}
            </Button>
            <Button variant="primary" disabled={checking || assigning} onClick={requestClose}>
              {t("addSkill.done")}
            </Button>
          </>
        )}
      </DialogFooter>

      <ConfirmDialog
        open={confirmClose}
        title={t("botEdit.discardTitle")}
        body={step === "install" ? t("addSkill.discardInstallBody") : t("addSkill.discardBody")}
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
    </div>
  );
}
