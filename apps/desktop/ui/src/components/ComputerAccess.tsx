import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { switchesOf, type ApiClient, type ComputerSwitches } from "../api/client";
import type { PcRequestInfo } from "../api/types";
import { humanLine } from "../lib/humanError";
import { useDaemon } from "../state/DaemonContext";
import { Modal } from "./Modal";
import { SettingRow, Switch } from "./SettingsUi";
import styles from "./ComputerAccess.module.css";

/** The owner key of this app's daemon (Tauri command, main window only), sent on owner-only routes. */
export const ownerKey = () => invoke<string>("owner_key");

/**
 * Turn "let this server's bots use this computer" (screen and input) on or off through the same
 * owner-key route as the Settings switch (`setComputerAccess`). The other families keep their state.
 */
export async function setHostComputerAccess(
  api: Pick<ApiClient, "setComputerAccess"> | null,
  host: Parameters<typeof switchesOf>[0] & { id: string },
  enabled: boolean
): Promise<void> {
  if (!api) {
    throw new Error("local_daemon_unavailable");
  }
  const now = switchesOf(host);
  await api.setComputerAccess(host.id, { ...now, screen: enabled, alwaysRead: enabled && now.alwaysRead }, await ownerKey());
}

type Family = "screen" | "browser" | "commands" | "files";
const FAMILIES: Family[] = ["screen", "browser", "commands", "files"];

/**
 * Settings > Computer: what each paired server's bots may do on this computer, one switch per
 * family (screen and input, browser, commands, files). All off by default; only the person at
 * this computer can turn them on (the daemon refuses every paired device on these routes).
 */
export function ComputerAccessCard() {
  const { t } = useTranslation();
  const { hostList, localApi, refreshHosts } = useDaemon();
  const [error, setError] = useState<string | null>(null);
  const hosts = hostList?.hosts ?? [];
  if (hosts.length === 0) {
    return <p className={styles.hint}>{t("computerAccess.noServer")}</p>;
  }
  const change = (id: string, next: ComputerSwitches) => {
    setError(null);
    void ownerKey()
      .then((key) => localApi?.setComputerAccess(id, next, key))
      .then(() => refreshHosts())
      .catch((e: unknown) => setError(humanLine(e)));
  };
  return (
    <div className={styles.stack}>
      <p className={styles.hint}>{t("computerAccess.explain")}</p>
      {hosts.map((h) => {
        const now = switchesOf(h);
        return FAMILIES.map((family) => (
          <SettingRow
            key={`${h.id}-${family}`}
            title={t(`computerAccess.family.${family}`, { name: h.name })}
            description={t(`computerAccess.familyNote.${family}`)}
          >
            <Switch
              checked={now[family]}
              label={t(`computerAccess.family.${family}`, { name: h.name })}
              onChange={(on) =>
                change(h.id, { ...now, [family]: on, alwaysRead: family === "screen" ? on && now.alwaysRead : now.alwaysRead })
              }
            />
          </SettingRow>
        ));
      })}
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
              onChange={(always) => change(h.id, { ...switchesOf(h), alwaysRead: always })}
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
  const watching = (hostList?.hosts ?? []).some(
    (h) => h.computer_access || h.computer_browser || h.computer_commands || h.computer_files
  );
  useEffect(() => {
    if (!watching || !localApi) {
      setPending([]);
      setReading([]);
      return undefined;
    }
    let stopped = false;
    const tick = () => {
      ownerKey()
        .then((key) => localApi.computerRequests(key))
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
    void ownerKey()
      .then((key) => localApi.decideComputerRequest(current.id, allow, key))
      .catch(() => {});
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
