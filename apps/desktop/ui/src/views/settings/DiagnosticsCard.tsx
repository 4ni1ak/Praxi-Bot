import {
  CircleCheck,
  CircleMinus,
  CircleX,
  ClipboardCopy,
  LoaderCircle,
  RefreshCw,
  ShieldCheck,
  Stethoscope,
  TriangleAlert,
  Wrench
} from "lucide-react";
import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DoctorCheck, RepairId } from "../../api/types";
import { useToast } from "../../components/Toast";
import {
  type CheckTone,
  INITIAL_DIAGNOSTICS,
  checkTone,
  diagnosticsReducer,
  repairButton,
  sortChecks,
  summarize
} from "../../lib/diagnostics";
import { useDaemon } from "../../state/DaemonContext";
import { notifyProvidersChanged } from "../../state/useProviders";
import styles from "./DiagnosticsCard.module.css";

const TONE_ICON = {
  ok: CircleCheck,
  warn: TriangleAlert,
  fail: CircleX,
  neutral: CircleMinus
} as const;

/** Checks whose "skipped" means the platform lacks the capability. */
const PLATFORM_CHECKS = new Set(["desktop_automation", "capabilities"]);

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Settings > Diagnostics: why the bot is not working, one-click repairs for
 * the allow-listed cases and a redacted support report.  Reading the checks
 * changes nothing; a repair runs only when the user presses its button.
 */
export function DiagnosticsCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const { client } = useDaemon();
  const [state, dispatch] = useReducer(diagnosticsReducer, INITIAL_DIAGNOSTICS);
  const [checkedAt, setCheckedAt] = useState<Date | null>(null);
  const [copying, setCopying] = useState(false);

  const load = useCallback(async () => {
    if (!client) {
      return;
    }
    dispatch({ type: "load_start" });
    try {
      const report = await client.doctor();
      dispatch({ type: "load_ok", checks: report.checks });
      setCheckedAt(new Date());
    } catch (e) {
      dispatch({ type: "load_error", error: errorText(e) });
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const runRepair = async (check: DoctorCheck, repair: RepairId) => {
    if (!client || state.running) {
      return;
    }
    dispatch({ type: "repair_start", repair });
    try {
      const r = await client.doctorRepair(repair);
      dispatch({ type: "repair_done", checkId: check.id, outcome: { ok: true, text: r.detail } });
      toast.show(t("settingsModal.diagnostics.repairDone"), "success");
      if (repair === "reload_providers") {
        notifyProvidersChanged();
      }
      await load();
    } catch (e) {
      const text = errorText(e);
      dispatch({ type: "repair_done", checkId: check.id, outcome: { ok: false, text } });
      toast.show(t("settingsModal.diagnostics.repairFailed"), "danger");
    }
  };

  const copyReport = async () => {
    if (!client) {
      return;
    }
    setCopying(true);
    try {
      const text = await client.doctorReport();
      await navigator.clipboard.writeText(text);
      toast.show(t("settingsModal.diagnostics.reportCopied"), "success");
    } catch (e) {
      toast.show(`${t("settingsModal.diagnostics.reportFailed")} (${errorText(e)})`, "danger");
    } finally {
      setCopying(false);
    }
  };

  const checks = useMemo(() => sortChecks(state.checks), [state.checks]);
  const summary = useMemo(() => summarize(state.checks), [state.checks]);
  const loading = state.phase === "loading";
  const problems = summary.fail + summary.warn;

  const statusLabel = (tone: CheckTone, check: DoctorCheck) =>
    tone === "neutral" && PLATFORM_CHECKS.has(check.id)
      ? t("settingsModal.diagnostics.status.unsupported")
      : t(`settingsModal.diagnostics.status.${tone}`);

  const detailOf = (tone: CheckTone, check: DoctorCheck) =>
    tone === "neutral" && PLATFORM_CHECKS.has(check.id) && check.detail === "not available on this platform"
      ? t("settingsModal.diagnostics.notOnPlatform")
      : check.detail;

  const SummaryIcon = loading && state.checks.length === 0 ? LoaderCircle : summary.overall === "ok" ? ShieldCheck : TONE_ICON[summary.overall];

  return (
    <section className={styles.root} data-testid="diagnostics-card" aria-labelledby="diag-h">
      <div className={`${styles.summary} ${styles[`tone_${summary.overall}`]}`}>
        <span className={styles.summaryIcon} aria-hidden="true">
          <SummaryIcon className={loading && state.checks.length === 0 ? styles.spin : undefined} />
        </span>
        <div className={styles.summaryText}>
          <h3 id="diag-h" className={styles.summaryTitle}>
            {!client
              ? t("settingsModal.diagnostics.title")
              : state.checks.length === 0
                ? t("settingsModal.diagnostics.running")
                : problems === 0
                  ? t("settingsModal.diagnostics.allGood")
                  : t("settingsModal.diagnostics.problems", { count: problems })}
          </h3>
          <span className={styles.summarySub}>
            {checkedAt
              ? t("settingsModal.diagnostics.checkedAt", { time: checkedAt.toLocaleTimeString() })
              : t("settingsModal.diagnostics.description")}
          </span>
        </div>
        <div className={styles.actions}>
          <button
            type="button"
            className={styles.textBtn}
            disabled={!client || loading || state.running !== null}
            onClick={() => void load()}
          >
            <RefreshCw aria-hidden="true" className={loading ? styles.spin : undefined} />
            {t("settingsModal.diagnostics.rerun")}
          </button>
          <button
            type="button"
            className={styles.pill}
            disabled={!client || copying}
            onClick={() => void copyReport()}
            data-testid="diagnostics-copy-report"
          >
            {copying ? <LoaderCircle aria-hidden="true" className={styles.spin} /> : <ClipboardCopy aria-hidden="true" />}
            {t("settingsModal.diagnostics.copyReport")}
          </button>
        </div>
      </div>

      {!client ? (
        <p className={styles.notice}>
          <Stethoscope aria-hidden="true" />
          {t("settingsModal.diagnostics.offline")}
        </p>
      ) : state.phase === "error" ? (
        <p className={styles.errorNotice} role="alert">
          <CircleX aria-hidden="true" />
          {t("settingsModal.diagnostics.loadError")} ({state.error})
        </p>
      ) : checks.length > 0 ? (
        <ul className={styles.list} aria-busy={loading}>
          {checks.map((check) => {
            const tone = checkTone(check.status);
            const Icon = TONE_ICON[tone];
            const button = repairButton(check, state);
            const outcome = state.outcomes[check.id];
            const hint = check.status === "warn" || check.status === "fail"
              ? t(`settingsModal.diagnostics.hints.${check.id}`, check.hint ?? "")
              : "";
            return (
              <li key={check.id} className={styles.item} data-testid={`diagnostics-${check.id}`} data-tone={tone}>
                <span className={`${styles.icon} ${styles[`tone_${tone}`]}`} aria-hidden="true">
                  <Icon />
                </span>
                <div className={styles.text}>
                  <span className={styles.name}>
                    {t(`settingsModal.diagnostics.checks.${check.id}`, check.title)}
                  </span>
                  <span className={styles.detail}>{detailOf(tone, check)}</span>
                  {hint ? <span className={styles.hint}>{hint}</span> : null}
                  {outcome ? (
                    <span className={outcome.ok ? styles.outcomeOk : styles.outcomeBad} role="status">
                      {outcome.text}
                    </span>
                  ) : null}
                </div>
                <div className={styles.ctl}>
                  <span className={`${styles.chip} ${styles[`chip_${tone}`]}`}>{statusLabel(tone, check)}</span>
                  {button ? (
                    <button
                      type="button"
                      className={`${styles.pill} ${styles.pillPri}`}
                      disabled={button.disabled}
                      aria-busy={button.busy}
                      onClick={() => void runRepair(check, button.repair)}
                    >
                      {button.busy ? <LoaderCircle aria-hidden="true" className={styles.spin} /> : <Wrench aria-hidden="true" />}
                      {t(`settingsModal.diagnostics.repair.${button.repair}`)}
                    </button>
                  ) : null}
                </div>
              </li>
            );
          })}
        </ul>
      ) : null}

      <p className={styles.footnote}>
        <ShieldCheck aria-hidden="true" />
        {t("settingsModal.diagnostics.privacy")}
      </p>
    </section>
  );
}
