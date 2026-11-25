import { Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import type { WhatsNewNotes } from "../lib/whatsNew";
import { Modal } from "./Modal";
import styles from "./WhatsNewModal.module.css";

/** Release notes of the version that was just installed; Esc, backdrop click or the button dismisses it. */
export function WhatsNewModal({
  notes,
  onClose
}: {
  notes: WhatsNewNotes | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Modal open={notes !== null} onClose={onClose} label={t("whatsNew.label")} size="sm">
      <div className={styles.root}>
        <div className={styles.head}>
          <Sparkles size={20} className={styles.icon} aria-hidden="true" />
          <h2 className={styles.title}>{notes?.title}</h2>
        </div>
        {notes?.text ? <pre className={styles.notes}>{notes.text}</pre> : null}
        <ul className={styles.list}>
          {notes?.items.map((item) => <li key={item}>{item}</li>)}
        </ul>
        <div className={styles.actions}>
          <Button variant="primary" size="sm" data-autofocus onClick={onClose}>
            {t("whatsNew.dismiss")}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
