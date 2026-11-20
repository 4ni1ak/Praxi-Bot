import { useState } from "react";
import { ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { callIdsOf, type BatchRow } from "../lib/approvalBatch";
import { humanLine } from "../lib/humanError";
import { toolSentence } from "../lib/runSentence";
import { summarizeInput } from "./ChatCards";
import styles from "./ChatCards.module.css";
import formStyles from "./Form.module.css";

interface Props {
  rows: BatchRow[];
  /** Answer every call id with one decision (each is a once-only approval or a denial). */
  onDecide: (callIds: string[], decision: "approve" | "deny") => Promise<void>;
}

/**
 * Several plain approvals of one run in one card, one toggle per distinct call. A repeated call
 * is one row and is answered together. Approvals here are always "once": a standing choice needs
 * the single card, where the user sees exactly what would be remembered.
 */
export function ApprovalBatchCard({ rows, onDecide }: Props) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<Set<string>>(() => new Set(rows.map((r) => r.key)));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const toggle = (key: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const decide = async (decision: "approve" | "deny") => {
    setBusy(true);
    setError(null);
    try {
      const approve = callIdsOf(rows, selected);
      const deny = callIdsOf(rows, new Set(rows.filter((r) => !selected.has(r.key)).map((r) => r.key)));
      if (decision === "deny") {
        await onDecide([...approve, ...deny], "deny");
      } else {
        if (approve.length > 0) await onDecide(approve, "approve");
        if (deny.length > 0) await onDecide(deny, "deny");
      }
    } catch (e) {
      setError(humanLine(e));
      setBusy(false);
    }
  };

  return (
    <div className={styles.approval} role="group" aria-label={t("chat.approval.batch.title", { count: rows.length })} data-testid="approval-batch">
      <span className={styles.approvalIcon}>
        <ShieldCheck size={22} aria-hidden />
      </span>
      <div className={styles.approvalMain}>
        <div className={styles.approvalHead}>
          <span className={styles.approvalTitle}>{t("chat.approval.batch.title", { count: rows.length })}</span>
        </div>
        <ul className={styles.todoList}>
          {rows.map((row) => {
            const first = row.items[0];
            if (!first) return null;
            const name = toolSentence(t, first);
            return (
              <li key={row.key}>
                <label className={styles.bodyText}>
                  <input
                    type="checkbox"
                    checked={selected.has(row.key)}
                    disabled={busy}
                    aria-label={t("chat.approval.batch.select", { name })}
                    onChange={() => toggle(row.key)}
                  />{" "}
                  {name}
                  {row.items.length > 1 ? ` ${t("chat.approval.batch.repeated", { count: row.items.length })}` : ""}
                </label>
                <div className={styles.approvalMeta}>
                  <span className={styles.approvalTool}>{first.name}</span>
                  <span className={styles.inputSummary}>{summarizeInput(first.input, 120)}</span>
                </div>
              </li>
            );
          })}
        </ul>
        <div className={styles.actions}>
          <Button size="sm" variant="primary" disabled={busy || selected.size === 0} onClick={() => void decide("approve")}>
            {t("chat.approval.batch.allowSelected", { count: callIdsOf(rows, selected).length })}
          </Button>
          <Button size="sm" variant="ghost" className={styles.denyButton} disabled={busy} onClick={() => void decide("deny")}>
            {t("chat.approval.batch.denyAll")}
          </Button>
        </div>
        {error ? <p className={formStyles.error} role="alert">{error}</p> : null}
      </div>
    </div>
  );
}
