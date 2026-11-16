import { Check, Copy, Info, Server } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AdminCliEngine, AdminState } from "../../api/types";
import { humanError } from "../../lib/humanError";
import { useDaemon } from "../../state/DaemonContext";
import { DetectedAccountsBlock } from "./DetectedAccountsBlock";
import styles from "./AiPage.module.css";

/** A command to run on the server, one click to copy. */
function CopyBlock({ text }: { text: string }) {
  const { t } = useTranslation();
  const [done, setDone] = useState(false);
  return (
    <span className={styles.ctl}>
      <code className={styles.code}>{text}</code>
      <button
        type="button"
        className={styles.textBtn}
        onClick={() => {
          void navigator.clipboard
            ?.writeText(text)
            .then(() => {
              setDone(true);
              window.setTimeout(() => setDone(false), 1500);
            })
            .catch(() => {});
        }}
      >
        {done ? <Check size={12} aria-hidden="true" /> : <Copy size={12} aria-hidden="true" />}
        {done ? t("errors.copied") : t("home.copy")}
      </button>
    </span>
  );
}

function useHostAdmin() {
  const { client, home, workLocalNow, activeComputerId } = useDaemon();
  const onHome = home.kind === "host" && !workLocalNow && activeComputerId === home.hostId;
  const [state, setState] = useState<AdminState | null>(null);
  const [engines, setEngines] = useState<AdminCliEngine[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  useEffect(() => {
    if (!onHome || !client) {
      return undefined;
    }
    let live = true;
    client.adminState().then(
      (s) => live && setState(s),
      (e) => live && setProblem(humanError(e).title)
    );
    client.adminCliEngines().then(
      (s) => live && setEngines(s),
      () => live && setEngines([])
    );
    return () => {
      live = false;
    };
  }, [onHome, client]);
  return { onHome, name: home.kind === "host" ? home.name : "", state, engines, problem };
}

/**
 * What the server still needs from its owner before this desktop can set it up, and what it keeps
 * to itself by design. Shown only while the home is a host.
 */
export function HostAdminNote() {
  const { t } = useTranslation();
  const { onHome, name, state, problem } = useHostAdmin();
  if (!onHome) {
    return null;
  }
  const who = name || t("computerPicker.host");
  return (
    <section className={styles.sec} data-testid="host-admin-note">
      <div className={`${styles.card} ${styles.item}`}>
        <Server size={18} aria-hidden="true" />
        <div className={styles.text}>
          <span>{t("home.admin.title")}</span>
          <span className={styles.sub}>{t("home.admin.byDesign", { name: who })}</span>
          {problem ? <span className={`${styles.sub} ${styles.subWarn}`}>{problem}</span> : null}
          {state && !state.granted ? (
            <>
              <span className={`${styles.sub} ${styles.subWarn}`}>{t("home.admin.notGranted", { name: who })}</span>
              {state.confirm_command ? <CopyBlock text={state.confirm_command} /> : null}
            </>
          ) : null}
          {state && !state.secrets_available ? (
            <>
              <span className={`${styles.sub} ${styles.subWarn}`}>{t("home.admin.noSecrets", { name: who })}</span>
              {state.secrets_hint ? <span className={styles.sub}>{state.secrets_hint}</span> : null}
            </>
          ) : null}
        </div>
      </div>
    </section>
  );
}

/** The server's coding CLIs, as installed THERE: read-only, with the commands to run on the server. */
function HostEnginesBlock() {
  const { t } = useTranslation();
  const { name, engines } = useHostAdmin();
  const who = name || t("computerPicker.host");
  return (
    <section className={styles.sec} aria-labelledby="home-engines-h" data-testid="host-engines">
      <div className={styles.head}>
        <h3 id="home-engines-h">{t("home.engines.title", { name: who })}</h3>
      </div>
      <div className={styles.card}>
        {engines === null ? (
          <p className={styles.hint}>{t("home.engines.loading")}</p>
        ) : engines.length === 0 ? (
          <p className={styles.hint}>{t("home.engines.none")}</p>
        ) : (
          <ul className={styles.list}>
            {engines.map((e) => (
              <li key={e.id} className={styles.item} data-testid={`host-engine-${e.id}`}>
                <div className={styles.text}>
                  <span>{e.name}</span>
                  <span className={styles.sub}>
                    {e.installed
                      ? e.signed_in === false
                        ? t("home.engines.signIn")
                        : t("home.engines.ready")
                      : t("home.engines.notInstalled")}
                    {e.installed && e.signed_in !== false && e.version ? ` · ${e.version}` : ""}
                  </span>
                  {!e.installed && e.install_command ? <CopyBlock text={e.install_command} /> : null}
                  {e.installed && e.signed_in === false ? <CopyBlock text={e.login} /> : null}
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
      <p className={styles.hint}>
        <Info size={12} aria-hidden="true" /> {t("home.engines.note", { name: who })}
      </p>
    </section>
  );
}

/** "Found on this computer" locally; the server's own engines when the home is a host. */
export function EnginesCard() {
  const { home, workLocalNow, activeComputerId } = useDaemon();
  const onHome = home.kind === "host" && !workLocalNow && activeComputerId === home.hostId;
  return onHome ? (
    <>
      <HostAdminNote />
      <HostEnginesBlock />
    </>
  ) : (
    <DetectedAccountsBlock />
  );
}
