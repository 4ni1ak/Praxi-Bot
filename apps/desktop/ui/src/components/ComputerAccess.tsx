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
                ?.setComputerAccess(h.id, enabled, enabled && h.computer_access_always === true)
                .then(() => refreshHosts())
                .catch((e: unknown) =>
                  setError(e instanceof Error ? e.message : String(e))
                );
            }}
          />
        </SettingRow>
      ))}
      {hosts
        .filter((h) => h.computer_access === true)
        .map((h) => (
          <SettingRow
            key={`${h.id}-always`}
            title={t("computerAccess.alwaysRead", { name: h.name })}
            description={t("computerAccess.alwaysReadNote")}
          >
            <Switch
              checked={h.computer_access_always === true}
              label={t("computerAccess.alwaysRead", { name: h.name })}
              onChange={(always) => {
                setError(null);
                void localApi
                  ?.setComputerAccess(h.id, true, always)
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
  const [reading, setReading] = useState<string[]>([]);
  const watching = (hostList?.hosts ?? []).some((h) => h.computer_access);
  useEffect(() => {
    if (!watching || !localApi) {
      setPending([]);
      setReading([]);
      return undefined;
    }
    let stopped = false;
    const tick = () => {
      localApi
        .computerRequests()
        .then((list) => {
          if (stopped) return;
          setPending(list.requests);
          setReading(list.reading);
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
    <>
      {reading.length > 0 ? (
        <div className={styles.looking} role="status" data-testid="computer-looking">
          <i aria-hidden="true" />
          {t("computerAccess.looking", { name: reading.join(", ") })}
        </div>
      ) : null}
      <Modal
        open={Boolean(current)}
        onClose={() => answer(false)}
        label={t("computerAccess.requestTitle")}
        size="sm"
      >
        {current ? (
          <div className={styles.request} data-testid="computer-request">
            <h2 className={styles.title}>
              {current.kind === "session_read"
                ? t("computerAccess.readTitle")
                : t("computerAccess.requestTitle")}
            </h2>
            <p className={styles.hint}>
              {t("computerAccess.requestBody")}{" "}
              <span className={styles.untrusted} title={t("computerAccess.fromServer")}>
                {t("computerAccess.fromServerName", { name: current.host_name })}
              </span>
            </p>
            <dl className={styles.facts}>
              <dt>{t("computerAccess.what")}</dt>
              <dd>{current.action}</dd>
              <dt>{t("computerAccess.where")}</dt>
              <dd>{current.target}</dd>
              <dt>{t("computerAccess.details")}</dt>
              <dd className={styles.payload}>{current.payload}</dd>
            </dl>
            <div className={styles.actions}>
              <Button variant="ghost" onClick={() => answer(false)}>
                {t("computerAccess.deny")}
              </Button>
              <Button variant="primary" onClick={() => answer(true)}>
                {current.kind === "session_read"
                  ? t("computerAccess.allowSession")
                  : t("computerAccess.allowOnce")}
              </Button>
            </div>
          </div>
        ) : null}
      </Modal>
    </>
  );
}
