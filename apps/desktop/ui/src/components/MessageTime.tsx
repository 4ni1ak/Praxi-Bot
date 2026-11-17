import { useTranslation } from "react-i18next";
import { type MessageTimes, messageTimeLabel } from "../lib/messageTime";
import styles from "./MessageTime.module.css";

/**
 * The quiet time next to a message: HH:mm (or HH:mm:ss in seconds mode),
 * seconds on hover / focus, the full local date-time in the tooltip.
 * Renders nothing when the setting is off.
 */
export function MessageTime({
  ts,
  mode,
  finishedTs
}: {
  ts: number;
  mode: MessageTimes;
  /** Completion time of a streamed reply (tooltip only). */
  finishedTs?: number;
}) {
  const { i18n } = useTranslation();
  const label = messageTimeLabel(ts, mode, i18n.language, finishedTs);
  if (!label) {
    return null;
  }
  const swaps = label.text !== label.hover;
  return (
    <time
      className={styles.time}
      dateTime={new Date(ts).toISOString()}
      title={label.title}
      tabIndex={0}
      data-testid="message-time"
    >
      <span className={swaps ? styles.rest : undefined}>{label.text}</span>
      {swaps ? <span className={styles.hover}>{label.hover}</span> : null}
    </time>
  );
}
