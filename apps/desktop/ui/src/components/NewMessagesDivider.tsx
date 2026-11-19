import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { ArrowDown } from "lucide-react";
import styles from "./NewMessagesDivider.module.css";

/** A hairline with a label above the first unseen bot message; reports once the user has scrolled past it. */
export function NewMessagesDivider({ onPassed }: { onPassed: () => void }) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement | null>(null);
  const seen = useRef(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting) seen.current = true;
      else if (seen.current && entry.boundingClientRect.bottom < (entry.rootBounds?.top ?? 0) + 1) onPassed();
    });
    io.observe(el);
    return () => io.disconnect();
  }, [onPassed]);
  return (
    <div ref={ref} className={styles.divider} role="separator" data-testid="new-messages-divider">
      <span className={styles.label}><ArrowDown aria-hidden="true" />{t("chat.newMessages")}</span>
    </div>
  );
}
