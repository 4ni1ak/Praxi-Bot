import { useState, type ReactNode } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import styles from "./ApprovalDock.module.css";

/** One thing the dock can show: a single approval card, or the card that answers several at once. */
export interface DockPage {
  key: string;
  /** A line above the card when it belongs elsewhere (another chat of the bot). */
  caption?: string | null;
  /** `active`: this is the page on screen (it alone answers the Enter / Esc keys). */
  render: (active: boolean) => ReactNode;
  /** Which keys answer this page: Enter may not allow a card the owner has to read. Default both. */
  keys?: "both" | "deny" | "none";
}

/**
 * The one place where a run's approvals are asked, docked directly above the composer (the
 * composer squares its top corners while this is open). Compact: one card at a time, a counter
 * with previous / next when several wait. It is a self-contained piece: the host passes the pages,
 * it owns nothing about how a card decides. After the answer the run's own step line is the trace.
 */
export function ApprovalDock({ pages }: { pages: readonly DockPage[] }) {
  const { t } = useTranslation();
  const [index, setIndex] = useState(0);
  if (pages.length === 0) {
    return null;
  }
  const at = Math.min(index, pages.length - 1);
  const page = pages[at]!;
  return (
    <section
      className={styles.dock}
      role="region"
      aria-label={t("approvalDock.label")}
      data-testid="approval-dock"
      data-approval-dock=""
      data-count={pages.length}
    >
      {pages.length > 1 ? (
        <div className={styles.stack} data-testid="approval-dock-counter">
          <span className={styles.counter} role="status">
            {t("approvalDock.counter", { current: at + 1, total: pages.length })}
          </span>
          <button
            type="button"
            className={styles.step}
            disabled={at === 0}
            aria-label={t("approvalDock.previous")}
            title={t("approvalDock.previous")}
            onClick={() => setIndex(at - 1)}
          >
            <ChevronLeft size={16} aria-hidden="true" />
          </button>
          <button
            type="button"
            className={styles.step}
            disabled={at === pages.length - 1}
            aria-label={t("approvalDock.next")}
            title={t("approvalDock.next")}
            onClick={() => setIndex(at + 1)}
          >
            <ChevronRight size={16} aria-hidden="true" />
          </button>
        </div>
      ) : null}
      {page.caption ? <p className={styles.caption}>{page.caption}</p> : null}
      <div className={styles.page} key={page.key}>
        {page.render(true)}
      </div>
      {page.keys === "none" ? null : (
        <p className={styles.keys} aria-hidden="true">
          {page.keys === "deny" ? null : (
            <span>
              <kbd>↵</kbd> {t("approvalDock.keyAllow")}
            </span>
          )}
          <span>
            <kbd>esc</kbd> {t("approvalDock.keyDeny")}
          </span>
        </p>
      )}
    </section>
  );
}
