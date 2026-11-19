import { Square } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { formatElapsed, subagentRowState } from "../lib/activityPanel";
import { groupRunItems } from "../lib/thinkingChain";
import type { RunState } from "../state/timeline";
import { Markdown } from "./Markdown";
import { DialogHeader } from "./DialogParts";
import { Modal } from "./Modal";
import { taskTitle, useNow } from "./SubagentList";
import { ThinkingChain } from "./ThinkingChain";
import dialog from "./Dialog.module.css";
import styles from "./SubagentList.module.css";

/**
 * One sub-agent's details in a modal: its task, its steps in the same chain rendering the chat
 * uses, and its result.  Approvals still happen in the chat; this view only says one is waiting.
 */
export function SubagentDetail({
  run,
  onClose,
  onStop
}: {
  run: RunState | undefined;
  onClose: () => void;
  onStop: (id: string) => void;
}) {
  const { t } = useTranslation();
  const state = run ? subagentRowState(run) : "done";
  const live = state === "running" || state === "waiting";
  const now = useNow(live);
  const title = taskTitle(t, run?.task ?? "", 80);
  return (
    <Modal open={run !== undefined} onClose={onClose} label={t("botPanel.subagents.detail.title")} size="md">
      {run ? (
        <div className={dialog.root} data-testid="subagent-detail">
          <DialogHeader
            title={title}
            subtitle={`${t(`botPanel.subagents.state.${state}`)} · ${formatElapsed((run.finishedMs ?? now) - run.startedMs)}`}
            onClose={onClose}
            actions={
              live ? (
                <Button size="sm" variant="danger" onClick={() => onStop(run.id)}>
                  <Square aria-hidden="true" />
                  {t("botPanel.subagents.stop")}
                </Button>
              ) : undefined
            }
          />
          <div className={dialog.body}>
            {run.task ? (
              <section className={styles.detailBlock}>
                <h3 className={styles.label}>{t("botPanel.subagents.detail.task")}</h3>
                <p className={styles.detailText}>{run.task}</p>
              </section>
            ) : null}
            {run.items.length > 0 ? (
              <section className={styles.detailBlock}>
                <h3 className={styles.label}>{t("botPanel.subagents.detail.steps")}</h3>
                {groupRunItems(run.items, run.finishedMs !== undefined, run.startedMs).map((seg, i) =>
                  seg.kind === "chain" ? (
                    <ThinkingChain key={seg.chain.key} chain={seg.chain} />
                  ) : seg.item.kind === "text" ? (
                    <Markdown key={i} text={seg.item.text} />
                  ) : seg.item.kind === "tool" && seg.item.status === "awaiting_approval" ? (
                    <p key={i} className={styles.detailNote}>
                      {t("botPanel.subagents.detail.approval")}
                    </p>
                  ) : null
                )}
              </section>
            ) : null}
            {run.summary ? (
              <section className={styles.detailBlock}>
                <h3 className={styles.label}>{t("botPanel.subagents.detail.result")}</h3>
                <Markdown text={run.summary} />
              </section>
            ) : null}
          </div>
        </div>
      ) : null}
    </Modal>
  );
}
