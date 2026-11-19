import { Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import styles from "./FinishSetupHint.module.css";

/** A quiet pill shown after "Skip for now": one click returns to the first-run flow. Never blocks anything. */
export function FinishSetupHint({ onClick }: { onClick: () => void }) {
  const { t } = useTranslation();
  return (
    <button type="button" className={styles.hint} data-testid="finish-setup-hint" onClick={onClick}>
      <Sparkles aria-hidden="true" />
      {t("firstRun.finishHint")}
    </button>
  );
}
