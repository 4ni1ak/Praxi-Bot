import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import type { ComposioStatus } from "../api/types";
import formStyles from "../components/Form.module.css";
import { openComposioLink } from "../lib/externalLink";
import { useDaemon } from "../state/DaemonContext";
import styles from "./AppsModal.module.css";

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Advanced: how Praxi is linked to Composio.  The normal link is the browser
 * login (nothing to paste); an owner with their own Composio project can paste
 * its key here, never next to an app.
 */
export function ComposioAdvanced({
  status,
  onChanged
}: {
  status: ComposioStatus | null;
  onChanged: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const { client } = useDaemon();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [own, setOwn] = useState(false);
  if (!status) {
    return null;
  }
  const run = (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    void fn()
      .then(async () => {
        setValue("");
        setOwn(false);
        await onChanged();
      })
      .catch((e: unknown) => setError(errorText(e)))
      .finally(() => setBusy(false));
  };
  return (
    <section className={styles.section} aria-labelledby="composio-adv-h" data-testid="composio-advanced">
      <h3 className={styles.sectionLabel} id="composio-adv-h">
        {t("apps.composio.advanced.title")}
      </h3>
      <p className={formStyles.hint}>{t(`apps.composio.advanced.mode.${status.mode}`)}</p>
      <div className={styles.actions}>
        {status.configured ? (
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            onClick={() => run(() => client?.removeComposioKey() ?? Promise.resolve())}
          >
            {t("apps.composio.advanced.signOut")}
          </Button>
        ) : null}
        <Button size="sm" variant="ghost" aria-expanded={own} onClick={() => setOwn((o) => !o)}>
          {t("apps.composio.advanced.ownKey")}
        </Button>
      </div>
      {own ? (
        <form
          className={styles.panel}
          onSubmit={(e) => {
            e.preventDefault();
            if (value.trim()) run(() => client?.setComposioKey(value.trim()) ?? Promise.resolve());
          }}
        >
          <button type="button" className={styles.link} onClick={() => void openComposioLink(status.key_setup_url, true)}>
            {t("apps.composio.getKey")}
          </button>
          <label className={formStyles.label} htmlFor="composio-key">
            {t("apps.composio.keyLabel")}
          </label>
          <input
            id="composio-key"
            className={formStyles.input}
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={value}
            onChange={(e) => setValue(e.target.value)}
          />
          <p className={formStyles.hint}>{t("apps.composio.keyHint")}</p>
          {error ? (
            <p className={formStyles.error} role="alert">
              {error}
            </p>
          ) : null}
          <Button type="submit" size="sm" variant="primary" disabled={busy || value.trim() === ""}>
            {t("apps.composio.keySave")}
          </Button>
        </form>
      ) : null}
    </section>
  );
}
