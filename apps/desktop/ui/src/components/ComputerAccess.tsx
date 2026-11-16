import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import type { PcRequestInfo } from "../api/types";
import { useDaemon } from "../state/DaemonContext";
import { Modal } from "./Modal";
import { SettingRow, Switch } from "./SettingsUi";
import styles from "./ComputerAccess.module.css";

/**
 * Settings > Computer: "Let my bots use this computer", one switch per paired server. Off by
 * default; only the person at this computer can turn it on (the daemon refuses every paired
 * device on these routes).
 */
export function ComputerAccessCard() {
  const { t } = useTranslation();
  const { hostList, localApi, refreshHosts } = useDaemon();
  const [error, setError] = useState<string | null>(null);
  const hosts = hostList?.hosts ?? [];
  if (hosts.length === 0) {
    return <p className={styles.hint}>{t("computerAccess.noServer")}</p>;
  }
  return (
    <div className={styles.stack}>
      <p className={styles.hint}>{t("computerAccess.explain")}</p>
      {hosts.map((h) => (
        <SettingRow
          key={h.id}
          title={t("computerAccess.allow", { name: h.name })}
          description={t("computerAccess.askNote")}
        >
          <Switch
            checked={h.computer_access === true}
            label={t("computerAccess.allow", { name: h.name })}
            onChange={(enabled) => {
              setError(null);
              void localApi
                ?.setComputerAccess(h.id, enabled)
                .then(() => refreshHosts())
                .catch((e: unknown) =>
                  setError(e instanceof Error ? e.message : String(e))
                );
            }}
          />
        </SettingRow>
      ))}
      {error ? (
        <p className={styles.error} role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/** Polls this computer's daemon for a server asking to act here; the owner answers. */
export function ComputerRequestModal() {
  const { t } = useTranslation();
  const { hostList, localApi } = useDaemon();
  const [pending, setPending] = useState<PcRequestInfo[]>([]);
  const watching = (hostList?.hosts ?? []).some((h) => h.computer_access);
  useEffect(() => {
    if (!watching || !localApi) {
      setPending([]);
      return undefined;
    }
    let stopped = false;
    const tick = () => {
      localApi
        .computerRequests()
        .then((list) => {
          if (!stopped) setPending(list);
        })
        .catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, 1500);
    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [watching, localApi]);
  const current = pending[0];
  const answer = (allow: boolean) => {
    if (!current || !localApi) return;
    setPending((p) => p.filter((r) => r.id !== current.id));
    void localApi.decideComputerRequest(current.id, allow).catch(() => {});
  };
  return (
    <Modal
      open={Boolean(current)}
      onClose={() => answer(false)}
      label={t("computerAccess.requestTitle")}
      size="sm"
    >
      {current ? (
        <div className={styles.request} data-testid="computer-request">
          <h2 className={styles.title}>{t("computerAccess.requestTitle")}</h2>
          <p className={styles.hint}>
            {t("computerAccess.requestBody", { name: current.host_name })}
          </p>
          <p className={styles.detail}>
            <strong>{current.action}</strong> {current.detail}
          </p>
          <div className={styles.actions}>
            <Button variant="ghost" onClick={() => answer(false)}>
              {t("computerAccess.deny")}
            </Button>
            <Button variant="primary" onClick={() => answer(true)}>
              {t("computerAccess.allowOnce")}
            </Button>
          </div>
        </div>
      ) : null}
    </Modal>
  );
}
