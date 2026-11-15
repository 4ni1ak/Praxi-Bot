import { getCurrentWindow } from "@tauri-apps/api/window";
import { CircleCheck, LoaderCircle } from "lucide-react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  BotAvatar,
  Button,
  Card
} from "ui";
import { i18n } from "../i18n";
import { useApplyAppearance } from "../state/useApplyAppearance";
import {
  reduceWidget,
  WIDGET_CONFIRMED_MS,
  WIDGET_STOP_INITIAL,
  type WidgetView
} from "../state/stopStatus";
import {
  type AppSettings,
  DEFAULT_SETTINGS,
  loadSettings,
  loadSettingsSync
} from "../store/settingsStore";
import {
  type ControlSnapshot,
  getControlState,
  giveBackControl,
  INITIAL_CONTROL_SNAPSHOT,
  isTauriRuntime,
  matchesHotkeyEvent,
  pauseBotRun,
  resumeBotRun,
  stopBotRun,
  subscribeOverlayEvents,
  takeTheWheel
} from "./events";
import styles from "./WidgetWindow.module.css";

export function WidgetWindow() {
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings>(() => loadSettingsSync() ?? DEFAULT_SETTINGS);
  // Every read, event and command reply goes through `reduceWidget`: modes only from
  // newer revisions, Stop cursors only forward, a failed read is "could not confirm".
  const [view, setView] = useState<WidgetView<ControlSnapshot>>({
    stop: WIDGET_STOP_INITIAL,
    lastRev: -1,
    control: INITIAL_CONTROL_SNAPSHOT
  });
  const observe = useCallback((snap: ControlSnapshot | null) => {
    const failed = snap === null || snap.stopSeq === undefined;
    setView((prev) => reduceWidget(prev, failed ? null : snap, performance.now()));
  }, []);
  const controlState = view.control;
  const stopState = view.stop;

  // Native only knows whether a Stop is still unconfirmed (the main window acks it
  // once every run it targeted is terminal). The ack emits no event: poll meanwhile.
  useEffect(() => {
    if (stopState.kind === "pending") {
      const id = window.setInterval(() => {
        void getControlState().then(observe);
      }, 1500);
      return () => window.clearInterval(id);
    }
    if (stopState.kind === "confirmed") {
      const id = window.setTimeout(() => observe(null), WIDGET_CONFIRMED_MS + 50);
      return () => window.clearTimeout(id);
    }
    return undefined;
  }, [stopState, observe]);

  useEffect(() => {
    let mounted = true;
    void loadSettings().then((loaded) => {
      if (!mounted) return;
      setSettings(loaded);
      if (loaded.language && loaded.language !== i18n.language) {
        void i18n.changeLanguage(loaded.language);
      }
    });

    void getControlState().then((snap) => {
      if (!mounted) return;
      observe(snap);
    });

    return () => {
      mounted = false;
    };
  }, [observe]);

  useApplyAppearance(settings, undefined, true);

  useEffect(() => {
    return subscribeOverlayEvents({
      onControlStateChanged: (snapshot) => {
        observe(snapshot);
      },
      onSettingsUpdated: (updated) => {
        setSettings(updated);
        if (updated.language && updated.language !== i18n.language) {
          void i18n.changeLanguage(updated.language);
        }
      }
    });
  }, [observe]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (matchesHotkeyEvent(event, controlState.stopHotkey)) {
        event.preventDefault();
        // One press = one Stop (never on key repeat).
        if (!event.repeat) {
          void stopBotRun().then(observe);
        }
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [controlState.stopHotkey, observe]);

  const handleDragMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !isTauriRuntime()) {
      return;
    }
    void getCurrentWindow()
      .startDragging()
      .catch(() => {});
  };

  const handleStop = () => {
    void stopBotRun().then(observe);
  };

  const handlePauseToggle = () => {
    if (controlState.mode === "paused") {
      void resumeBotRun()
        .then(observe)
        .catch(() => {});
    } else if (controlState.mode === "bot_driving") {
      void pauseBotRun()
        .then(observe)
        .catch(() => {});
    }
  };

  const handleWheelToggle = () => {
    if (controlState.mode === "human_driving") {
      void giveBackControl()
        .then(observe)
        .catch(() => {});
    } else {
      void takeTheWheel()
        .then(observe)
        .catch(() => {});
    }
  };

  const bot = controlState.bot ?? {
    botId: "atlas",
    botName: "Atlas",
    botShape: undefined,
    botColor: undefined,
    botInitials: "AT",
    botAvatar: undefined
  };

  const isHumanDriving = controlState.mode === "human_driving";
  const isPaused = controlState.mode === "paused";

  const statusBadgeClass = [
    styles.statusBadge,
    isHumanDriving
      ? styles.statusHuman
      : isPaused
        ? styles.statusPaused
        : styles.statusDriving
  ]
    .filter(Boolean)
    .join(" ");

  const statusDotClass = [
    styles.statusDot,
    isHumanDriving
      ? styles.statusDotHuman
      : isPaused
        ? styles.statusDotPaused
        : ""
  ]
    .filter(Boolean)
    .join(" ");

  const widgetStopKey =
    stopState.kind === "confirmed"
      ? "widget.stopConfirmed"
      : stopState.unknown
        ? "widget.stopUnknown"
        : "widget.stopPending";

  const stepDisplay = isHumanDriving
    ? t("widget.humanDrivingStep")
    : isPaused
      ? controlState.currentStep || t("widget.pausedStep")
      : controlState.currentStep || t("widget.idleStep");

  return (
    <div className={styles.widgetViewport}>
      <Card
        padding="md"
        accentColor={isHumanDriving ? undefined : bot.botColor}
        className={[
          styles.widgetCard,
          isHumanDriving ? styles.widgetCardHuman : ""
        ]
          .filter(Boolean)
          .join(" ")}
      >
        <div
          className={styles.dragHeader}
          data-tauri-drag-region
          onMouseDown={handleDragMouseDown}
          aria-label={t("widget.dragHandleAria")}
        >
          <div className={styles.botIdentity}>
            <BotAvatar bot={{ id: bot.botId, name: bot.botName, shape: bot.botShape, avatar: bot.botAvatar, color: bot.botColor }} size="md" state={controlState.mode === "paused" ? "waiting-approval" : controlState.mode === "idle" ? "idle" : "working"} />
            <div className={styles.botTextMeta}>
              <span className={styles.botName}>{bot.botName}</span>
              <span className={styles.hotkeyHint} title={t("settings.hotkey.description")}>
                {t("widget.hotkeyHint", { hotkey: controlState.stopHotkey })}
              </span>
            </div>
          </div>

          <span className={statusBadgeClass}>
            <span className={statusDotClass} aria-hidden="true" />
            <span>{t(`widget.status.${controlState.mode}`)}</span>
          </span>
        </div>

        <div className={styles.stepContainer} role="status" aria-live="polite">
          {stopState.kind === "none" ? (
            <p className={styles.stepText}>{stepDisplay}</p>
          ) : (
            <p
              className={styles.stopLine}
              data-kind={stopState.kind}
              title={t(widgetStopKey)}
            >
              {stopState.kind === "pending" ? (
                <LoaderCircle aria-hidden="true" className={styles.stopSpin} />
              ) : (
                <CircleCheck aria-hidden="true" />
              )}
              <span>{t(widgetStopKey)}</span>
            </p>
          )}
          {isHumanDriving && controlState.refusedActionsCount > 0 ? (
            <span className={styles.refusedInfo}>
              {t("widget.refusedBadge", {
                count: controlState.refusedActionsCount
              })}
            </span>
          ) : null}
        </div>

        <div className={styles.controlsRow}>
          <div className={styles.primaryControls}>
            <Button variant="danger" size="sm" onClick={handleStop}>
              {t("widget.stop")}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              disabled={isHumanDriving || controlState.mode === "idle"}
              onClick={handlePauseToggle}
            >
              {isPaused ? t("widget.resume") : t("widget.pause")}
            </Button>
          </div>

          <Button
            variant={isHumanDriving ? "primary" : "secondary"}
            size="sm"
            disabled={controlState.mode === "idle"}
            onClick={handleWheelToggle}
          >
            {isHumanDriving ? t("widget.giveBack") : t("widget.takeTheWheel")}
          </Button>
        </div>
      </Card>
    </div>
  );
}
