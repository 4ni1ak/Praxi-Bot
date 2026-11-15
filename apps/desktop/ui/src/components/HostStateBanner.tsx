import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { LOCAL_COMPUTER, useDaemon } from "../state/DaemonContext";
import styles from "./HostStateBanner.module.css";

/**
 * Says what is going on with the connection to the active host, in plain words:
 * connecting, host off, no longer paired, version mismatch (both versions) or no
 * model key on the host yet. Silent while this computer is active or all is well.
 */
export function HostStateBanner() {
  const { t } = useTranslation();
  const { hostState, activeHostName, versions, selectComputer, activeComputerId } =
    useDaemon();
  if (activeComputerId === LOCAL_COMPUTER || !hostState || hostState.kind === "connected") {
    return null;
  }
  const name = activeHostName ?? t("computerPicker.host");
  let tone: "wait" | "warn" | "bad" = "warn";
  let text: string;
  switch (hostState.kind) {
    case "connecting":
      tone = "wait";
      text = t("hostState.connecting", { name });
      break;
    case "offline":
      tone = "bad";
      text = t("hostState.offline", { name });
      break;
    case "not_paired":
      tone = "bad";
      text = t("hostState.notPaired", { name });
      break;
    case "version_mismatch":
      tone = hostState.blocking ? "bad" : "warn";
      text = t(hostState.blocking ? "hostState.versionBlocking" : "hostState.versionWarning", {
        name,
        local: hostState.local || versions.local || "?",
        host: hostState.host || versions.host || "?"
      });
      break;
    case "no_model_key":
      tone = "warn";
      text = t("hostState.noModelKey", { name });
      break;
  }
  return (
    <div
      className={styles.root}
      data-tone={tone}
      data-kind={hostState.kind}
      role="status"
      data-testid="host-state-banner"
    >
      <i className={styles.dot} aria-hidden="true" />
      <span className={styles.text}>{text}</span>
      <Button
        size="sm"
        variant="ghost"
        onClick={() => void selectComputer(LOCAL_COMPUTER)}
      >
        {t("hostState.useThisComputer")}
      </Button>
    </div>
  );
}
