import { Server } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useDaemon } from "../state/DaemonContext";
import styles from "./HomeHint.module.css";

/** A quiet "on <server>" tag: shown only while the bots' home is a host and the app is on it. */
export function HomeHint() {
  const { t } = useTranslation();
  const { home, workLocalNow } = useDaemon();
  if (home.kind !== "host" || workLocalNow) {
    return null;
  }
  return (
    <span className={styles.hint} data-testid="home-hint">
      <Server aria-hidden="true" />
      {t("home.hint", { name: home.name || t("computerPicker.host") })}
    </span>
  );
}
