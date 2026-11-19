import { X } from "lucide-react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { MessageQueue } from "../lib/messageQueue";
import styles from "./QueuedMessages.module.css";

/**
 * Messages typed while the bot works (design v7): dashed, slightly dimmed bubbles marked
 * "Sirada." with a faint "Simdi gonder" link at the bottom right. Several stack in order; each
 * has a small x on hover. Rendering only; the state machine is `lib/messageQueue.ts`.
 */
export function QueuedMessages({
  queue,
  onSendNow,
  onRemove,
  renderText
}: {
  queue: MessageQueue;
  onSendNow: (id: string) => void;
  onRemove: (id: string) => void;
  renderText?: (text: string) => ReactNode;
}) {
  const { t } = useTranslation();
  if (queue.length === 0) {
    return null;
  }
  return (
    <div className={styles.stack} data-testid="queued-messages">
      {queue.map((m) => (
        <div key={m.id} className={styles.item} data-testid="queued-message">
          <div className={styles.row}>
            <button
              type="button"
              className={styles.remove}
              aria-label={t("queued.remove")}
              title={t("queued.remove")}
              data-testid="queued-remove"
              onClick={() => onRemove(m.id)}
            >
              <X aria-hidden="true" />
            </button>
            <div className={styles.bubble}>{renderText ? renderText(m.text) : m.text}</div>
          </div>
          <button type="button" className={styles.sendNow} data-testid="queued-send-now" onClick={() => onSendNow(m.id)}>
            <b>{t("queued.label")}</b>
            {t("queued.sendNow")}
          </button>
        </div>
      ))}
    </div>
  );
}
