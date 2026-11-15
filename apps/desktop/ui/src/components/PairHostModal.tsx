import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { ApiError } from "../api/client";
import { useDaemon } from "../state/DaemonContext";
import form from "./Form.module.css";
import { Modal } from "./Modal";
import styles from "./PairHostModal.module.css";

/**
 * Pair this computer with an always-on host. The ticket comes from the host
 * (`bot host invite --role desktop`); it is single use, kept only in this field,
 * and never stored or logged by the app.
 */
export function PairHostModal({
  open,
  onClose
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Modal open={open} onClose={onClose} size="sm" label={t("pairHost.title")}>
      {open ? <PairHostContent onClose={onClose} /> : null}
    </Modal>
  );
}

function PairHostContent({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { pairHost, selectComputer } = useDaemon();
  const [ticket, setTicket] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    if (!ticket.trim() || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const info = await pairHost(ticket.trim());
      setTicket("");
      onClose();
      void selectComputer(info.id);
    } catch (e) {
      setError(e instanceof ApiError && e.status === 400 ? e.message : t("pairHost.failed"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.root}>
      <header className={styles.header}>
        <h2 className={styles.title}>{t("pairHost.title")}</h2>
        <p className={styles.lead}>{t("pairHost.lead")}</p>
      </header>
      <ol className={styles.steps}>
        <li>{t("pairHost.step1")}</li>
        <li>
          <code className={styles.code}>bot host invite --role desktop</code>
        </li>
        <li>{t("pairHost.step3")}</li>
      </ol>
      <label className={form.field}>
        <span className={form.label}>{t("pairHost.ticket")}</span>
        <textarea
          className={form.textarea}
          value={ticket}
          autoComplete="off"
          spellCheck={false}
          rows={3}
          data-autofocus
          data-testid="pair-ticket"
          onChange={(e) => setTicket(e.target.value)}
        />
        <p className={form.hint}>{t("pairHost.hint")}</p>
        {error ? (
          <p className={form.error} role="alert">
            {error}
          </p>
        ) : null}
      </label>
      <div className={styles.actions}>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button
          variant="primary"
          disabled={busy || !ticket.trim()}
          onClick={() => void submit()}
          data-testid="pair-submit"
        >
          {busy ? t("pairHost.pairing") : t("pairHost.pair")}
        </Button>
      </div>
    </div>
  );
}
