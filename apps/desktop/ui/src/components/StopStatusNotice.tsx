import {
  CircleCheck,
  CircleHelp,
  CloudOff,
  FileWarning,
  LoaderCircle,
  OctagonAlert,
  TimerOff,
  X,
  type LucideIcon
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { useStopStatus } from "../state/appStop";
import {
  type StopKind,
  stopDetail,
  stopTitleKey
} from "../state/stopStatus";
import styles from "./StopStatusNotice.module.css";

const ICONS: Record<StopKind, LucideIcon> = {
  stopping: LoaderCircle,
  unreachable: CloudOff,
  partial: OctagonAlert,
  expired: TimerOff,
  unconfirmable: CircleHelp,
  stopped: CircleCheck
};

/**
 * What happened to the user's latest Stop (`describeStop` decides; this renders).
 * `main`: a strip under the title bar. `panel`: a compact card in the right panel.
 */
export function StopStatusNotice({ variant }: { variant: "main" | "panel" }) {
  const { t } = useTranslation();
  const { status, dismiss } = useStopStatus();
  if (!status) {
    return null;
  }
  const Icon = ICONS[status.kind];
  const detail = stopDetail(status);
  const progress = status.total > 0 ? status.stopped / status.total : 0;
  return (
    <div className={styles.wrap} data-variant={variant}>
      <div
        className={styles.notice}
        data-tone={status.tone}
        data-kind={status.kind}
        data-testid={`stop-status-${variant}`}
        role={status.tone === "danger" ? "alert" : "status"}
        aria-live="polite"
      >
        <span className={styles.icon} data-spin={status.kind === "stopping" ? "true" : undefined}>
          <Icon aria-hidden="true" />
        </span>
        <span className={styles.text}>
          <strong className={styles.title}>{t(stopTitleKey(status))}</strong>
          <span className={styles.detail}>{t(detail.key, detail.params)}</span>
          {status.kind === "partial" && status.error ? (
            <span className={styles.error} title={status.error}>
              {status.error}
            </span>
          ) : null}
          {!status.audited ? (
            <span className={styles.audit}>
              <FileWarning aria-hidden="true" />
              <span>{t("stopStatus.notAudited")}</span>
            </span>
          ) : null}
        </span>
        {status.total > 0 && status.kind !== "stopped" ? (
          <span
            className={styles.meter}
            role="progressbar"
            aria-label={t("stopStatus.progressLabel")}
            aria-valuemin={0}
            aria-valuemax={status.total}
            aria-valuenow={status.stopped}
          >
            <span
              className={styles.meterFill}
              style={{ inlineSize: `${Math.round(progress * 100)}%` }}
            />
          </span>
        ) : null}
        {status.dismissible ? (
          <button
            type="button"
            className={styles.dismiss}
            aria-label={t("stopStatus.dismiss")}
            title={t("stopStatus.dismiss")}
            onClick={dismiss}
          >
            <X aria-hidden="true" />
          </button>
        ) : null}
      </div>
    </div>
  );
}
