import { PauseCircle } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { needsPauseRecovery } from "../lib/liveViewState";
import { useLiveView } from "../state/LiveViewContext";
import styles from "./PausedControlBanner.module.css";

/**
 * Stays on the main screen while the daemon holds a paused takeover of ours,
 * whether or not the viewer or the right panel is open and whatever the native
 * run state says (Stop sets the native mode to idle but never ends the daemon
 * pause). Clearing the pause is `POST /v1/control/recover`; reacquiring needs the
 * screen, so it opens the viewer.
 */
export function PausedControlBanner() {
  const { t } = useTranslation();
  const { state, controlState, clearPause, openViewer } = useLiveView();
  if (!needsPauseRecovery(state, controlState)) {
    return null;
  }
  return (
    <div className={styles.wrap}>
      <div className={styles.banner} role="status" data-testid="paused-control-banner">
        <PauseCircle aria-hidden="true" />
        <span className={styles.text}>
          <strong>{t("liveView.pausedBanner.title")}</strong>
          <span>{t("liveView.pausedBanner.body")}</span>
        </span>
        <span className={styles.actions}>
          <Button
            variant="primary"
            size="sm"
            data-testid="paused-banner-clear"
            onClick={() => void clearPause()}
          >
            {t("liveView.clearPauseAction")}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            data-testid="paused-banner-open"
            onClick={() => openViewer()}
          >
            {t("liveView.openScreen")}
          </Button>
        </span>
      </div>
    </div>
  );
}
