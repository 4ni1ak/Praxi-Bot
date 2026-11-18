import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { formatElapsed, type RunBarModel, type RunStopUi } from "../lib/runBar";
import { StopStatusNotice } from "./StopStatusNotice";
import styles from "./RunStatusLine.module.css";

function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) {
      return undefined;
    }
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}

export interface RunStatusLineProps {
  /** `null` while nothing runs. */
  model: RunBarModel | null;
  /** Bot name, shown only when the run belongs to another bot than this chat. */
  botName?: string;
  /** The current step as one sentence ("Searching the web"). */
  sentence: string | null;
  /** The step that waits for the owner, as a sentence; set only while an approval is pending. */
  approvalStep: string | null;
  startedMs: number | null;
  /** This chat's Stop progress (the one stop function of the chat decides it). */
  runStop: RunStopUi | null;
  /** The chat's one stop function: the second press while a Stop is unconfirmed is the hard stop. */
  onStop: () => void;
  humanDriving: boolean;
  onPause: () => void;
  onResume: () => void;
  onWheel: () => void;
}

/**
 * The run state of the chat (design v7): one slim, boxless line above the composer - a pulse, the
 * current step as a sentence, the elapsed time. Amber and "waiting for your approval" while an
 * approval is pending. Stopping is the round button of the composer; this line only says
 * "Stopping..." once it was pressed. The few desktop-control actions (pause / resume / take the
 * wheel) stay as small quiet links, and only while the bot drives the desktop.
 */
export function RunStatusLine({
  model,
  botName,
  sentence,
  approvalStep,
  startedMs,
  runStop: stop,
  onStop,
  humanDriving,
  onPause,
  onResume,
  onWheel
}: RunStatusLineProps) {
  const { t } = useTranslation();
  const now = useNow(model !== null && startedMs !== null);
  const lastStart = useRef(startedMs);
  if (startedMs !== null) {
    lastStart.current = startedMs;
  }
  const elapsed = lastStart.current !== null ? formatElapsed(now - lastStart.current) : null;
  const approval = model?.kind === "waiting";
  const text =
    stop?.phase === "stopping"
      ? t("runStatus.stopping")
      : stop?.phase === "unconfirmed"
        ? t(stop.unreachable ? "chat.runBar.stopUnreachable" : "chat.runBar.stopUnconfirmed")
        : stop?.phase === "failed"
        ? (stop.error ?? t("stopStatus.title.partial"))
        : approval
          ? approvalStep
            ? t("runStatus.waiting", { step: approvalStep })
            : t("chat.runBar.waiting")
          : model?.kind === "paused"
            ? t("chat.runBar.paused")
            : model?.kind === "human"
              ? t("chat.runBar.human")
              : (sentence ?? t("runStatus.working"));
  return (
    <div className={styles.wrap}>
      <StopStatusNotice variant="main" />
      {model ? (
        <div
          className={styles.status}
          data-kind={stop?.phase === "failed" ? "failed" : approval ? "approval" : "run"}
          data-testid="run-status"
          role="status"
          aria-live="polite"
        >
          <span className={styles.pulse} aria-hidden="true" />
          <span className={styles.text}>{botName ? `${botName} · ${text}` : text}</span>
          <span className={styles.actions}>
            {stop?.phase === "unconfirmed" || stop?.phase === "failed" ? (
              <button type="button" className={styles.link} onClick={onStop} data-testid="force-stop">
                {t("chat.runBar.forceStop")}
              </button>
            ) : null}
            {model.canResume ? (
              <button type="button" className={styles.link} onClick={onResume}>
                {t("widget.resume")}
              </button>
            ) : null}
            {model.canPause ? (
              <button type="button" className={styles.link} onClick={onPause}>
                {t("widget.pause")}
              </button>
            ) : null}
            {model.canWheel ? (
              <button type="button" className={styles.link} onClick={onWheel}>
                {humanDriving ? t("widget.giveBack") : t("widget.takeTheWheel")}
              </button>
            ) : null}
          </span>
          {elapsed ? (
            <span className={styles.time} aria-hidden="true" title={t("chat.runBar.elapsed", { time: elapsed })}>
              {elapsed}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
