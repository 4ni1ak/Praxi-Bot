import { WifiOff } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useConnectivity } from "../state/useConnectivity";
import styles from "./ConnectivityNotice.module.css";

export function ConnectivityNotice() {
  const { t } = useTranslation();
  const { state } = useConnectivity();
  if (state !== "offline" && state !== "host_unreachable") return null;
  return <div className={styles.notice} role="status" data-testid="connectivity-notice">
    <WifiOff aria-hidden="true" />
    <span>{t(state === "offline" ? "offline.title" : "offline.host")} {t(state === "offline" ? "offline.detail" : "offline.hostDetail")}</span>
  </div>;
}
