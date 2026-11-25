import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { SettingRow } from "../../components/SettingsUi";
import {
  type UpdateInfo,
  type UpdateStatus,
  type UpdateChannel,
  checkForUpdate,
  installUpdate,
  rollbackUpdate,
  setUpdateChannel,
  skipUpdate,
  updateErrorKey,
  updateStatus
} from "../../lib/appUpdate";
import styles from "./UpdatePanel.module.css";

type Phase = "idle" | "checking" | "installing" | "rollingBack" | "saving";

/**
 * Check for updates, read what is new, install with one click (the app checks
 * the signature before it installs anything) and go back to the previous
 * version when the new one does not start properly.
 */
export function UpdatePanel() {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [found, setFound] = useState<UpdateInfo | null>(null);
  const [upToDate, setUpToDate] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void updateStatus(i18n.language).then(setStatus).catch((e: unknown) => setError(t(updateErrorKey(e))));
  }, [i18n.language, t]);

  if (!status) {
    return error ? <p className={styles.error} role="alert">{error}</p> : null;
  }

  const run = async (next: Phase, action: () => Promise<void>) => {
    setPhase(next);
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(t(updateErrorKey(e)));
    } finally {
      try {
        setStatus(await updateStatus(i18n.language));
      } catch (e) {
        setError(t(updateErrorKey(e)));
      }
      setPhase("idle");
    }
  };

  const check = () =>
    run("checking", async () => {
      setFound(null);
      setUpToDate(false);
      const info = await checkForUpdate(i18n.language);
      setFound(info);
      setUpToDate(info === null);
    });

  const busy = phase !== "idle";
  return (
    <>
      <SettingRow title={t("settingsModal.updates.buildId")}>
        <span>{status.buildId || t("settingsModal.updates.unknown")}</span>
      </SettingRow>
      <SettingRow title={t("settingsModal.updates.channel.title")} description={t("settingsModal.updates.channel.description")}>
        <select
          aria-label={t("settingsModal.updates.channel.title")}
          value={status.channel}
          disabled={busy || !status.enabled}
          onChange={(e) => {
            const channel = e.target.value as UpdateChannel;
            void run("saving", async () => {
              await setUpdateChannel(channel);
              setFound(null);
              setUpToDate(false);
            });
          }}
        >
          <option value="stable">{t("settingsModal.updates.channel.stable")}</option>
          <option value="preview" disabled={!status.previewAvailable}>{t("settingsModal.updates.channel.preview")}</option>
        </select>
      </SettingRow>
      {status.enabled && !status.previewAvailable ? <p className={styles.status}>{t("settingsModal.updates.channel.unavailable")}</p> : null}
      {status.rollbackAdvised ? (
        <SettingRow
          title={t("settingsModal.updates.rollback.title")}
          description={t("settingsModal.updates.rollback.description", {
            version: status.currentVersion
          })}
        >
          <Button
            size="sm"
            variant="primary"
            disabled={busy || !status.rollbackAvailable}
            onClick={() => void run("rollingBack", rollbackUpdate)}
          >
            {phase === "rollingBack"
              ? t("settingsModal.updates.rollback.working")
              : t("settingsModal.updates.rollback.action")}
          </Button>
        </SettingRow>
      ) : null}
      <SettingRow
        title={t("settingsModal.updates.check.title")}
        description={t(status.enabled ? "settingsModal.updates.check.description" : "settingsModal.updates.notConfigured")}
        stacked
      >
        <div className={styles.actions}>
          <Button size="sm" variant="secondary" disabled={busy || !status.enabled} onClick={() => void check()}>
            {phase === "checking"
              ? t("settingsModal.updates.check.working")
              : t("settingsModal.updates.check.action")}
          </Button>
          {upToDate && !status.skippedVersion && !error ? (
            <span className={styles.status} role="status">{t("settingsModal.updates.upToDate")}</span>
          ) : null}
          {error ? <span className={styles.error} role="alert">{error}</span> : null}
        </div>
        <p className={styles.status}>
          {status.lastChecked === null
            ? t("settingsModal.updates.neverChecked")
            : t("settingsModal.updates.lastChecked", { time: new Date(status.lastChecked * 1000).toLocaleString(i18n.language) })}
        </p>
        {status.skippedVersion ? <p className={styles.status}>{t("settingsModal.updates.skipped", { version: status.skippedVersion })}</p> : null}
        {found ? (
          <>
            <p className={styles.status}>
              {t("settingsModal.updates.available", { version: found.version })}
            </p>
            {found.notes ? <pre className={styles.notes}>{found.notes}</pre> : null}
            <div className={styles.actions}>
              <Button
                size="sm"
                variant="primary"
                disabled={busy}
                onClick={() => void run("installing", installUpdate)}
              >
                {phase === "installing"
                  ? t("settingsModal.updates.install.working")
                  : t("settingsModal.updates.install.action")}
              </Button>
              <Button size="sm" variant="secondary" disabled={busy} onClick={() => void run("saving", async () => {
                await skipUpdate();
                setFound(null);
                setUpToDate(false);
              })}>
                {t("settingsModal.updates.skip")}
              </Button>
            </div>
            <span className={styles.status}>{t("settingsModal.updates.install.hint")}</span>
          </>
        ) : null}
      </SettingRow>
    </>
  );
}
