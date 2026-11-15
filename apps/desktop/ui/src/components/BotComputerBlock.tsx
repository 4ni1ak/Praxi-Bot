import { Globe, Maximize2, Monitor } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import {
  isActivelyOwningTakeover,
  summarizeTarget,
  visibleActions
} from "../lib/liveViewState";
import { LOCAL_COMPUTER, useDaemon } from "../state/DaemonContext";
import {
  COMPUTER_FOOTER_KEY,
  computerFooter,
  computerPanel,
  noScreenCode,
  type PanelText
} from "../lib/computerLabel";
import { useLiveView } from "../state/LiveViewContext";
import styles from "./BotComputerBlock.module.css";

function panelTone(
  hasTarget: boolean,
  viewState: string,
  unavailable: boolean
): "watch" | "active" | "acquiring" | "paused" | "idle" {
  if (!hasTarget || unavailable || viewState === "unavailable") {
    return "idle";
  }
  if (viewState === "active") {
    return "active";
  }
  if (viewState === "acquiring") {
    return "acquiring";
  }
  if (viewState === "paused") {
    return "paused";
  }
  return "watch";
}

export function BotComputerBlock() {
  const { t } = useTranslation();
  const { activeComputerId, activeHostName } = useDaemon();
  const onHost = activeComputerId !== LOCAL_COMPUTER;
  const {
    selectedTarget,
    targetsStatus,
    availability,
    state,
    frameUrl,
    panelAction,
    openViewer,
    releaseTakeover,
    retryRelease,
    controlState,
    recoveryPrompt,
    recoverTakeover,
    clearPause,
    isLocalClient
  } = useLiveView();
  const panelActions = visibleActions(state, controlState, isLocalClient).panel;
  // Control can outlive the viewer (a failed hand-back keeps it): never leave the
  // bot paused with only a status pill, always offer Bitirdim here too.
  const releasePending = state.pendingRequest?.kind === "release";
  const showRelease = isActivelyOwningTakeover(state) || releasePending;

  const summary = selectedTarget ? summarizeTarget(selectedTarget) : null;
  const tone = panelTone(
    Boolean(selectedTarget),
    state.viewState,
    Boolean(summary && !summary.available)
  );

  const panel = computerPanel({
    hasTarget: Boolean(selectedTarget),
    available: Boolean(summary?.available),
    reason: summary?.reason ?? null,
    viewState: state.viewState,
    hasFrame: Boolean(frameUrl),
    targetsStatus,
    noScreen: noScreenCode(availability)
  });
  // Header pill and preview come from one model; the reason is a code the
  // daemon serves, worded here in the user's language.
  const say = (text: PanelText) =>
    t(text.key, {
      detail: text.detailKey ? t(text.detailKey) : "",
      defaultValue: t("liveView.reasons.unavailable")
    });

  const KindIcon = selectedTarget?.kind === "browser" ? Globe : Monitor;

  const actionPercent =
    panelAction?.rect && state.latestFrame?.sourceRect
      ? {
          left: `${Math.min(
            95,
            Math.max(
              5,
              ((panelAction.rect.x + panelAction.rect.width / 2) /
                Math.max(1, state.latestFrame.sourceRect.width)) *
                100
            )
          )}%`,
          top: `${Math.min(
            95,
            Math.max(
              5,
              ((panelAction.rect.y + panelAction.rect.height / 2) /
                Math.max(1, state.latestFrame.sourceRect.height)) *
                100
            )
          )}%`
        }
      : null;

  return (
    <section className={styles.block} aria-labelledby="bot-panel-computer">
      <div className={styles.head}>
        <h3 className={styles.label} id="bot-panel-computer">
          {t("botPanel.computer")}
        </h3>
        <span className={styles.where} data-testid="bot-computer-where" data-computer={onHost ? "host" : "local"}>
          {onHost
            ? t("botPanel.runsOnHost", { name: activeHostName ?? t("computerPicker.host") })
            : t("botPanel.runsOnLocal")}
        </span>
        <span className={styles.statusPill} data-testid="bot-computer-status">
          <i className={styles.dot} data-tone={tone} aria-hidden="true" />
          <span>{say(panel.pill)}</span>
        </span>
      </div>

      <button
        type="button"
        className={styles.card}
        data-testid="bot-computer-open"
        aria-label={t("liveView.expand")}
        onClick={() => openViewer(selectedTarget?.target_id)}
      >
        <div className={styles.preview}>
          {frameUrl ? (
            <img
              className={styles.thumb}
              src={frameUrl}
              alt={t("liveView.stageAlt", {
                target: selectedTarget
                  ? t(`liveView.kind.${selectedTarget.kind}`)
                  : t("botPanel.computer")
              })}
              draggable={false}
            />
          ) : (
            <div className={styles.placeholder}>
              <KindIcon aria-hidden="true" />
              <span>{say(panel.preview)}</span>
            </div>
          )}
          {actionPercent ? (
            <span
              className={styles.actionDot}
              style={actionPercent}
              aria-hidden="true"
            />
          ) : null}
          <span className={styles.expandTag} aria-hidden="true">
            <Maximize2 />
          </span>
        </div>

        <div className={styles.footer}>
          <span className={styles.meta}>
            <KindIcon aria-hidden="true" />
            <span className={styles.host}>
              {selectedTarget
                ? `${t(`liveView.kind.${selectedTarget.kind}`)} · ${selectedTarget.host_id}`
                : t("botPanel.computer")}
            </span>
          </span>
          <span className={styles.cap}>
            {t(COMPUTER_FOOTER_KEY[computerFooter(summary, targetsStatus)])}
          </span>
        </div>
      </button>

      {panelActions.includes("clear_pause") && !showRelease ? (
        <div className={styles.release} data-testid="bot-computer-paused-actions">
          {recoveryPrompt?.mode === "reacquire" ? (
            <Button
              variant="primary"
              size="sm"
              data-testid="bot-computer-reacquire"
              onClick={() => void recoverTakeover()}
            >
              {t("liveView.reacquireAction")}
            </Button>
          ) : null}
          <Button
            variant={recoveryPrompt?.mode === "reacquire" ? "secondary" : "primary"}
            size="sm"
            data-testid="bot-computer-clear-pause"
            onClick={() => void clearPause("own")}
          >
            {t("liveView.clearPauseAction")}
          </Button>
        </div>
      ) : null}

      {showRelease ? (
        <div className={styles.release}>
          <Button
            variant="primary"
            size="sm"
            data-testid="bot-computer-release"
            onClick={() =>
              void (releasePending && state.lastError
                ? retryRelease()
                : releaseTakeover(null))
            }
            disabled={releasePending && !state.lastError}
          >
            {releasePending && state.lastError
              ? t("liveView.retryDone")
              : releasePending
                ? t("liveView.retryingDone")
                : t("liveView.doneAction")}
          </Button>
        </div>
      ) : null}
    </section>
  );
}
