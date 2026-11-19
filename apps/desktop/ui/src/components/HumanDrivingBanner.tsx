import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { useDaemon } from "../state/DaemonContext";
import { useLiveView } from "../state/LiveViewContext";
import styles from "./HumanDrivingBanner.module.css";

/**
 * Whenever the daemon holds a `human_driving` latch that no takeover session owns (the
 * viewer shows its own controls for those), say so and give one click to end it: the bot
 * cannot look at the screen while it is on.
 */
export function HumanDrivingBanner() {
  const { t } = useTranslation();
  const { client } = useDaemon();
  const { controlState, ingestControlSnapshot } = useLiveView();
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  if (!client || controlState?.latched !== "human_driving" || controlState.takeover) {
    return null;
  }
  const giveBack = async () => {
    setBusy(true);
    setFailed(false);
    try {
      ingestControlSnapshot(await client.releaseControl());
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className={styles.banner} role="status" data-testid="human-driving-banner">
      <span>{failed ? t("humanDriving.giveBackFailed") : t("humanDriving.banner")}</span>
      <Button size="sm" variant="primary" disabled={busy} onClick={() => void giveBack()}>
        {t("humanDriving.giveBack")}
      </Button>
    </div>
  );
}
