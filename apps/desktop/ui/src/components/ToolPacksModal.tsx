import {
  BookOpenText,
  Check,
  CircleCheck,
  CircleDashed,
  CircleX,
  Clapperboard,
  ExternalLink,
  FileText,
  Globe,
  Loader2,
  type LucideIcon,
  Plus,
  ScanText,
  SquareTerminal,
  Wrench
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { ApiError } from "../api/client";
import { isMockMode } from "../api/mockMode";
import type { HostPackInfo, HostPacks, HostSetupJob, PackAction, PackPlan } from "../api/types";
import { openExternal } from "../lib/externalLink";
import { humanLine } from "../lib/humanError";
import {
  isPackId,
  jobFailure,
  licenseIsMixed,
  missingBase,
  NOTED_PACKS,
  needsAck,
  type RowState,
  reasonKey,
  rowStates,
  runnableItems,
  safeSourceUrl,
  sizeParts,
  sortedPacks,
  statusOf,
  totalMb
} from "../lib/toolPacks";
import { useDaemon } from "../state/DaemonContext";
import dialog from "./Dialog.module.css";
import { Callout, DialogFooter, DialogHeader } from "./DialogParts";
import { Modal } from "./Modal";
import styles from "./ToolPacksModal.module.css";

const POLL_MS = 1500;

const ICONS: Record<string, LucideIcon> = {
  office: FileText,
  docs: BookOpenText,
  ocr: ScanText,
  media: Clapperboard,
  dev: SquareTerminal,
  scrape: Globe
};

type Phase = "loading" | "list" | "planning" | "preview" | "running" | "done" | "failed" | "nologin";

/**
 * "Tools" of one server: what is installed, and add or remove packs in three calm steps. Pick, look
 * at exactly what would change (a dry run), then confirm. Everything goes through the owner's own
 * SSH login: a bot on the server can never install software itself.
 */
export function ToolPacksModal({
  open,
  hostId,
  planBase = false,
  onClose
}: {
  open: boolean;
  hostId: string;
  /** Opened by the Add server wizard: go straight to the dry-run plan of the basic set. */
  planBase?: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Modal open={open} onClose={onClose} size="md" label={t("toolPacks.title")}>
      {open ? <Content hostId={hostId} planBase={planBase} onClose={onClose} /> : null}
    </Modal>
  );
}

function Size({ mb }: { mb: number }) {
  const { t } = useTranslation();
  const { value, unit } = sizeParts(mb);
  return <>{t("toolPacks.size", { value, unit })}</>;
}

function Content({ hostId, planBase, onClose }: { hostId: string; planBase: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const { client, localApi, hostList } = useDaemon();
  const api = isMockMode() ? client : localApi;
  const name = hostList?.hosts.find((h) => h.id === hostId)?.name ?? t("toolPacks.thisServer");
  const [phase, setPhase] = useState<Phase>("loading");
  const [facts, setFacts] = useState<HostPacks | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [acked, setAcked] = useState(false);
  const [action, setAction] = useState<PackAction>("install");
  const [plan, setPlan] = useState<PackPlan | null>(null);
  const [job, setJob] = useState<HostSetupJob | null>(null);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<number | null>(null);
  const planned1 = useRef(false);

  const stopPolling = useCallback(() => {
    if (timer.current !== null) window.clearInterval(timer.current);
    timer.current = null;
  }, []);
  useEffect(() => stopPolling, [stopPolling]);

  const load = useCallback(async () => {
    if (!api) return;
    setPhase("loading");
    try {
      setFacts(await api.getHostPacks(hostId));
      setPhase("list");
    } catch (e) {
      if (e instanceof ApiError && e.status === 400) setPhase("nologin");
      else {
        setError(humanLine(e));
        setPhase("list");
      }
    }
  }, [api, hostId]);
  useEffect(() => {
    void load();
  }, [load]);

  const packs: HostPackInfo[] = facts ? sortedPacks(facts.packs) : [];
  const toggle = (id: string) => {
    setAcked(false);
    setSelected((s) => (s.includes(id) ? s.filter((x) => x !== id) : [...s, id]));
  };
  const ackNeeded = needsAck(packs, selected);
  const factsProblem = facts?.error ? reasonKey(facts.error) : null;
  const canChange = Boolean(facts) && factsProblem === null;

  const review = async (act: PackAction, ids: string[]) => {
    if (!api || ids.length === 0) return;
    setError(null);
    setAction(act);
    setPhase("planning");
    try {
      setPlan(await api.planHostPacks(hostId, act, ids, act === "install" && acked ? needsAck(packs, ids).map((p) => p.id) : []));
      setPhase("preview");
    } catch (e) {
      setError(humanLine(e));
      setPhase("list");
    }
  };

  // From the wizard: the basic set is preselected and its dry-run plan shown, never installed
  // without the owner's confirmation on that plan.
  useEffect(() => {
    if (!planBase || planned1.current || phase !== "list" || !facts || facts.error || !facts.can_admin) return;
    const ids = missingBase(facts.packs);
    planned1.current = true;
    if (ids.length > 0) void review("install", ids);
  });

  const poll = (id: string) => {
    stopPolling();
    timer.current = window.setInterval(() => {
      if (!api) return;
      void api
        .getHostSetup(id)
        .then((j) => {
          setJob(j);
          if (j.state === "succeeded") {
            stopPolling();
            setPhase("done");
            void api.getHostPacks(hostId).then(setFacts, () => undefined);
          } else if (j.state === "failed" || j.state === "cancelled") {
            stopPolling();
            setPhase("failed");
            void api.getHostPacks(hostId).then(setFacts, () => undefined);
          }
        })
        .catch(() => {
          // This app's own daemon restarting is not the server's problem: keep asking.
        });
    }, POLL_MS);
  };

  const run = async () => {
    if (!api || !plan) return;
    setError(null);
    try {
      const j = await api.applyHostPacks(plan.plan_id);
      setJob(j);
      setPhase("running");
      poll(j.id);
    } catch (e) {
      setError(humanLine(e));
    }
  };

  const stop = () => {
    if (api && job) void api.cancelHostSetup(job.id).catch(() => undefined);
  };

  const back = () => {
    setPlan(null);
    setJob(null);
    setSelected([]);
    setError(null);
    void load();
  };

  const planned = plan ? runnableItems(plan.items).map((i) => i.pack) : [];
  const states = rowStates(job, planned);
  const failure = jobFailure(job);
  const running = phase === "running";
  const basic = facts ? missingBase(facts.packs) : [];
  const verb = action === "install" ? "install" : "remove";

  return (
    <div className={dialog.root} data-testid="tool-packs" data-phase={phase}>
      <DialogHeader
        title={t("toolPacks.titleFor", { name })}
        subtitle={t("toolPacks.lead")}
        leading={<Wrench aria-hidden="true" />}
        onClose={onClose}
        closeDisabled={running}
      />
      <div className={dialog.body}>
        {phase === "loading" || phase === "planning" ? (
          <p className={styles.loading} role="status">
            <Loader2 aria-hidden="true" className={styles.spin} />
            {phase === "loading" ? t("toolPacks.loading") : t("toolPacks.planning")}
          </p>
        ) : null}

        {phase === "nologin" ? (
          <Callout tone="info" title={t("toolPacks.noLogin.title")}>
            <p>{t("toolPacks.noLogin.body")}</p>
          </Callout>
        ) : null}

        {phase === "list" && facts ? (
          <div className={styles.fade}>
            {factsProblem ? (
              <Callout tone="warning" title={t(`toolPacks.reason.${factsProblem}.title`)} role="status">
                <p>{t(`toolPacks.reason.${factsProblem}.body`)}</p>
              </Callout>
            ) : null}
            <ul className={styles.list} aria-label={t("toolPacks.listLabel")}>
              {packs.map((p) => {
                const Icon = ICONS[p.id] ?? Wrench;
                const status = statusOf(p);
                const pickable = canChange && status !== "installed";
                const on = selected.includes(p.id);
                return (
                  <li key={p.id} className={styles.row} data-selected={on} data-state={status} data-testid={`pack-${p.id}`}>
                    <button
                      type="button"
                      className={styles.rowButton}
                      role="checkbox"
                      aria-checked={on}
                      aria-disabled={!pickable}
                      onClick={() => pickable && toggle(p.id)}
                    >
                      <span className={styles.icon}>
                        <Icon aria-hidden="true" />
                      </span>
                      <span className={styles.text}>
                        <b>{isPackId(p.id) ? t(`toolPacks.packs.${p.id}.name`) : p.id}</b>
                        {isPackId(p.id) ? <small>{t(`toolPacks.packs.${p.id}.desc`)}</small> : null}
                        {NOTED_PACKS.includes(p.id) && isPackId(p.id) ? (
                          <small className={styles.noteLine}>{t(`toolPacks.packs.${p.id}.note`)}</small>
                        ) : null}
                      </span>
                    </button>
                    <span className={styles.meta}>
                      <LicenseBits pack={p} />
                      {status === "installed" ? (
                        <span className={styles.chip} data-tone="ok">
                          <Check aria-hidden="true" />
                          {t("toolPacks.status.installed")}
                        </span>
                      ) : (
                        <>
                          {status === "partial" ? (
                            <span className={styles.chip} data-tone="warn">
                              {t("toolPacks.status.partial")}
                            </span>
                          ) : null}
                          <Size mb={p.size_mb} />
                        </>
                      )}
                      {status !== "missing" && p.removable && canChange && facts.can_admin ? (
                        <button
                          type="button"
                          className={styles.link}
                          onClick={() => void review("remove", [p.id])}
                          data-testid={`pack-remove-${p.id}`}
                        >
                          {t("toolPacks.remove")}
                        </button>
                      ) : null}
                    </span>
                  </li>
                );
              })}
            </ul>
            {facts.tools.length > 0 ? (
              <details className={styles.fade}>
                <summary>{t("toolPacks.found", { n: facts.tools.filter((x) => x.present).length })}</summary>
                <div className={styles.tools}>
                  {facts.tools.map((x) => (
                    <span key={x.name} className={`${styles.chip} ${x.present ? "" : styles.toolMuted}`} data-tone={x.present ? "ok" : undefined}>
                      {x.name}
                    </span>
                  ))}
                </div>
              </details>
            ) : null}
            {ackNeeded.length > 0 ? (
              <label className={styles.ack} data-testid="pack-ack">
                <input type="checkbox" checked={acked} onChange={(e) => setAcked(e.target.checked)} />
                <span>
                  {t("toolPacks.ack.text", { license: ackNeeded.map((p) => p.license).join(", ") })}
                  <b>{t("toolPacks.ack.label")}</b>
                </span>
              </label>
            ) : null}
            {error ? (
              <p className={dialog.error} role="alert">
                {error}
              </p>
            ) : null}
          </div>
        ) : null}

        {phase === "preview" && plan ? (
          <div className={styles.fade}>
            <ul className={styles.list} aria-label={t("toolPacks.previewLabel")} data-testid="pack-plan">
              {plan.items.map((i) => {
                const Icon = ICONS[i.pack] ?? Wrench;
                const why = i.blocked ? reasonKey(i.blocked) : null;
                return (
                  <li key={i.pack} className={styles.row} data-state={why ? "blocked" : "ok"} data-testid={`plan-${i.pack}`}>
                    <span className={styles.icon}>
                      <Icon aria-hidden="true" />
                    </span>
                    <span className={styles.text}>
                      <b>{isPackId(i.pack) ? t(`toolPacks.packs.${i.pack}.name`) : i.pack}</b>
                      <small>
                        {why
                          ? t(`toolPacks.reason.${why}.title`)
                          : i.noop
                            ? t("toolPacks.nothingToDo")
                            : t(action === "install" ? "toolPacks.willInstall" : "toolPacks.willRemove")}
                      </small>
                    </span>
                    {!why && !i.noop && action === "install" ? (
                      <span className={styles.meta}>
                        <Size mb={i.size_mb} />
                      </span>
                    ) : null}
                  </li>
                );
              })}
            </ul>
            {plan.items.some((i) => i.blocked) ? (
              <Callout tone="warning" role="status">
                <p>{t(`toolPacks.reason.${reasonKey(plan.items.find((i) => i.blocked)?.blocked)}.body`)}</p>
              </Callout>
            ) : null}
            {plan.runnable && action === "remove" ? (
              <Callout tone="warning" role="status" data-testid="pack-remove-warning">
                <p>{t("toolPacks.removeWarning")}</p>
              </Callout>
            ) : null}
            {plan.runnable ? (
              <Callout tone="info" title={t(action === "install" ? "toolPacks.safeInstall" : "toolPacks.safeRemove")}>
                <p>{t(action === "install" ? "toolPacks.safeInstallBody" : "toolPacks.safeRemoveBody")}</p>
              </Callout>
            ) : null}
            {error ? (
              <p className={dialog.error} role="alert">
                {error}
              </p>
            ) : null}
          </div>
        ) : null}

        {phase === "running" || phase === "done" || phase === "failed" ? (
          <ol className={styles.list} aria-live="polite" aria-label={t("toolPacks.progress")} data-testid="pack-progress">
            {planned.map((id) => {
              const state = states[id] ?? "pending";
              const Icon = ICONS[id] ?? Wrench;
              return (
                <li key={id} className={styles.row} data-state={state} data-selected={state === "running"}>
                  <span className={styles.icon}>
                    <StateIcon state={state} fallback={Icon} />
                  </span>
                  <span className={styles.text}>
                    <b>{isPackId(id) ? t(`toolPacks.packs.${id}.name`) : id}</b>
                    <small>{t(`toolPacks.rowState.${verb}.${state}`)}</small>
                  </span>
                </li>
              );
            })}
          </ol>
        ) : null}

        {phase === "done" ? (
          <Callout tone="success" title={t(action === "install" ? "toolPacks.doneInstall" : "toolPacks.doneRemove")} role="status">
            {action === "install" ? <p>{t("toolPacks.doneInstallBody")}</p> : null}
          </Callout>
        ) : null}

        {phase === "failed" ? (
          <Callout tone="danger" title={t(`toolPacks.reason.${reasonKey(failure)}.title`)} role="alert">
            <p>{t(`toolPacks.reason.${reasonKey(failure)}.body`)}</p>
          </Callout>
        ) : null}
        {running ? <p className={styles.note}>{t("toolPacks.dontClose")}</p> : null}
      </div>
      <DialogFooter>
        {phase === "list" ? (
          <>
            {basic.length > 0 && canChange && facts?.can_admin ? (
              <Button variant="ghost" onClick={() => setSelected(basic)} data-testid="pack-basic">
                <Plus aria-hidden="true" />
                {t("toolPacks.basicSet")}
              </Button>
            ) : null}
            <Button variant="ghost" onClick={onClose}>
              {t("common.close")}
            </Button>
            <Button
              variant="primary"
              disabled={selected.length === 0 || (ackNeeded.length > 0 && !acked)}
              onClick={() => void review("install", selected)}
              data-testid="pack-review"
            >
              {selected.length > 0
                ? t("toolPacks.review", { size: sizeLabel(t, totalMb(packs, selected)) })
                : t("toolPacks.reviewNone")}
            </Button>
          </>
        ) : null}
        {phase === "nologin" ? (
          <Button variant="primary" onClick={onClose}>
            {t("common.close")}
          </Button>
        ) : null}
        {phase === "preview" ? (
          <>
            <Button variant="ghost" onClick={back}>
              {t("toolPacks.back")}
            </Button>
            <Button variant="primary" disabled={!plan?.runnable} onClick={() => void run()} data-testid="pack-confirm">
              {action === "install" ? t("toolPacks.confirmInstall") : t("toolPacks.confirmRemove")}
            </Button>
          </>
        ) : null}
        {running ? (
          <Button variant="ghost" onClick={stop}>
            {t("toolPacks.stop")}
          </Button>
        ) : null}
        {phase === "done" ? (
          <Button variant="primary" onClick={back}>
            {t("common.close")}
          </Button>
        ) : null}
        {phase === "failed" ? (
          <>
            <Button variant="ghost" onClick={onClose}>
              {t("common.close")}
            </Button>
            <Button variant="primary" onClick={back} data-testid="pack-retry">
              {t("toolPacks.tryAgain")}
            </Button>
          </>
        ) : null}
      </DialogFooter>
    </div>
  );
}

/** Licence chip and source link of one pack: shown for every pack so nothing is installed unseen. */
function LicenseBits({ pack }: { pack: HostPackInfo }) {
  const { t } = useTranslation();
  const href = safeSourceUrl(pack.source_url);
  if (!pack.license) return null;
  const mixed = licenseIsMixed(pack.license);
  return (
    <>
      <span
        className={styles.chip}
        data-tone={pack.needs_ack ? "warn" : undefined}
        title={pack.version ? `${pack.license} · ${t("toolPacks.versionLabel", { version: pack.version })}` : pack.license}
        data-testid={`license-${pack.id}`}
      >
        {mixed ? t("toolPacks.licenseMixed") : pack.license}
      </span>
      {href ? (
        <button
          type="button"
          className={styles.link}
          onClick={() => openExternal(href)}
          aria-label={`${t("toolPacks.source")}: ${isPackId(pack.id) ? t(`toolPacks.packs.${pack.id}.name`) : pack.id}`}
          title={t("toolPacks.source")}
          data-testid={`source-${pack.id}`}
        >
          <ExternalLink aria-hidden="true" className={styles.linkIcon} />
        </button>
      ) : null}
    </>
  );
}

function sizeLabel(t: (k: string, o?: Record<string, unknown>) => string, mb: number): string {
  const { value, unit } = sizeParts(mb);
  return t("toolPacks.size", { value, unit });
}

function StateIcon({ state, fallback: Fallback }: { state: RowState; fallback: LucideIcon }) {
  if (state === "running") return <Loader2 aria-hidden="true" className={styles.spin} />;
  if (state === "done") return <CircleCheck aria-hidden="true" />;
  if (state === "failed") return <CircleX aria-hidden="true" />;
  return state === "pending" ? <CircleDashed aria-hidden="true" /> : <Fallback aria-hidden="true" />;
}
