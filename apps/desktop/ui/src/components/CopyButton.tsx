import { Check, Copy } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { copyText } from "../lib/copyText";
import styles from "./MessageInfo.module.css";

const COPIED_MS = 1500;

/** "Copy" beside a message: puts the message's own text (Markdown source for replies) on the clipboard. */
export function CopyButton({ text }: { text: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  if (!text.trim()) {
    return null;
  }
  const label = t(copied ? "chat.messageCopied" : "chat.copyMessage");
  return (
    <button
      type="button"
      className={styles.info}
      aria-label={label}
      title={label}
      data-testid="copy-message"
      onClick={() => {
        void copyText(text).then((ok) => {
          if (!ok) {
            return;
          }
          setCopied(true);
          window.clearTimeout(timer.current);
          timer.current = window.setTimeout(() => setCopied(false), COPIED_MS);
        });
      }}
    >
      {copied ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
    </button>
  );
}
