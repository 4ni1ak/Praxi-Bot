import { ExternalLink, LoaderCircle, Pin, PinOff } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { BotAvatar } from "ui";
import { connectDaemon } from "../api/connection";
import { botView } from "../lib/botProfile";
import { chatTitleLabel } from "../lib/chatTitle";
import {
  onLiveTargetUpdate,
  popoutConnect,
  popoutFocusMain,
  popoutSetAlwaysOnTop,
  type PopoutLiveSource,
  type ResolvedWindowRoute
} from "../lib/popout";
import {
  getControlState,
  INITIAL_CONTROL_SNAPSHOT,
  isTauriRuntime,
  stopBotRun,
  subscribeOverlayEvents,
  type ControlSnapshot
} from "../overlay/events";
import { ToastProvider, useToast } from "../components/Toast";
import { WindowControls } from "../components/WindowControls";
import { DaemonProvider, HostSession, LOCAL_COMPUTER, useDaemon } from "../state/DaemonContext";
import { LiveViewProvider, useLiveView, useLocalLiveView } from "../state/LiveViewContext";
import { useApplyAppearance } from "../state/useApplyAppearance";
import { DEFAULT_SETTINGS, loadSettings, loadSettingsSync, type AppSettings } from "../store/settingsStore";
import { i18n } from "../i18n";
import { appRuntime } from "../state/appStop";
import { EngineConsentProvider } from "../state/EngineConsent";
import { stopLiveRuns } from "../lib/runLocation";
import { humanLine } from "../lib/humanError";
import { ChatView } from "../views/ChatView";
import { LiveViewer } from "../views/LiveViewer";
import styles from "./PopoutWindow.module.css";

function usePopoutShellState(initialPinned: boolean) {
  const toast = useToast();
  const { t } = useTranslation();
  const [settings, setSettings] = useState<AppSettings>(() => loadSettingsSync() ?? DEFAULT_SETTINGS);
  useApplyAppearance(settings, undefined, true);

  const pinBusy = useRef(false);
  const [pinned, setPinned] = useState(initialPinned);
  const [control, setControl] = useState<ControlSnapshot>(INITIAL_CONTROL_SNAPSHOT);

  useEffect(() => {
    let mounted = true;
    void loadSettings().then((loaded) => {
      if (!mounted) return;
      setSettings(loaded);
      void i18n.changeLanguage(loaded.language);
    });
    void getControlState()
      .then((snap) => {
        if (mounted) setControl(snap);
      })
      .catch(() => {});
    const unlisten = subscribeOverlayEvents({
      onControlStateChanged: (snap) => setControl(snap),
      onSettingsUpdated: (next) => {
        setSettings((prev) => ({ ...next, windowFrame: prev.windowFrame }));
        void i18n.changeLanguage(next.language);
      }
    });
    return () => {
      mounted = false;
      unlisten();
    };
  }, []);

  const togglePinned = useCallback(async () => {
    if (pinBusy.current) return;
    pinBusy.current = true;
    const next = !pinned;
    setPinned(next);
    if (isTauriRuntime()) {
      try {
        const applied = await popoutSetAlwaysOnTop(next);
        setPinned(applied);
      } catch {
        setPinned(!next);
        toast.show(t("popout.pinFailed"), "danger");
      }
    }
    pinBusy.current = false;
  }, [pinned, t, toast]);

  const focusMain = useCallback(() => {
    if (isTauriRuntime()) {
      void popoutFocusMain().catch(() => toast.show(t("popout.focusFailed"), "danger"));
    } else if (typeof window !== "undefined") {
      window.opener?.focus();
    }
  }, [t, toast]);

  const closeWindow = useCallback(() => {
    if (isTauriRuntime()) {
      void getCurrentWindow().close();
    } else if (typeof window !== "undefined") {
      window.close();
    }
  }, []);

  return {
    settings,
    pinned,
    togglePinned,
    control,
    setControl,
    focusMain,
    closeWindow
  };
}

function PopoutHeader({
  title,
  subtitle,
  avatar,
  pinned,
  customChrome,
  onTogglePin,
  onFocusMain
}: {
  title: string;
  subtitle?: string | null;
  avatar?: ReturnType<typeof botView> | null;
  pinned: boolean;
  customChrome: boolean;
  onTogglePin: () => void;
  onFocusMain: () => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      {customChrome ? <WindowControls /> : null}
      <header className={styles.header} data-tauri-drag-region>
        <div className={styles.headerLeft} data-tauri-drag-region>
          {avatar ? <BotAvatar bot={avatar} size="xs" decorative /> : null}
          <h1 className={styles.title} data-tauri-drag-region data-testid="popout-title">
            {title}
          </h1>
          {subtitle ? <span className={styles.subtitle}>{subtitle}</span> : null}
        </div>
        <div className={styles.headerActions}>
          <button
            type="button"
            className={styles.iconBtn}
            aria-pressed={pinned}
            aria-label={t(pinned ? "popout.unpinOnTop" : "popout.pinOnTop")}
            title={t(pinned ? "popout.unpinOnTop" : "popout.pinOnTop")}
            data-testid="popout-pin-toggle"
            onClick={onTogglePin}
          >
            {pinned ? <PinOff aria-hidden="true" /> : <Pin aria-hidden="true" />}
          </button>
          <button
            type="button"
            className={styles.iconBtn}
            aria-label={t("popout.focusMain")}
            title={t("popout.focusMain")}
            data-testid="popout-focus-main"
            onClick={onFocusMain}
          >
            <ExternalLink aria-hidden="true" />
          </button>
        </div>
      </header>
    </>
  );
}

function PopoutDaemonScope({
  computerId,
  children
}: {
  computerId?: string;
  children: React.ReactNode;
}) {
  const connect = useCallback(
    () => (isTauriRuntime() ? popoutConnect() : connectDaemon()),
    []
  );
  return (
    <DaemonProvider
      connect={connect}
      allowStartupDiagnostics={false}
      initialComputerId={computerId}
    >
      <PopoutHostRouter>{children}</PopoutHostRouter>
    </DaemonProvider>
  );
}

function PopoutHostRouter({ children }: { children: React.ReactNode }) {
  const { activeComputerId } = useDaemon();
  if (activeComputerId !== LOCAL_COMPUTER) {
    return (
      <HostSession key={activeComputerId}>
        {children}
      </HostSession>
    );
  }
  return <>{children}</>;
}

function ChatPopoutBody({
  chatId,
  settings,
  pinned,
  onTogglePin,
  onFocusMain,
  control,
  onControl
}: {
  chatId: string;
  settings: AppSettings;
  pinned: boolean;
  onTogglePin: () => void;
  onFocusMain: () => void;
  control: ControlSnapshot;
  onControl: (next: ControlSnapshot) => void;
}) {
  const { t } = useTranslation();
  const { client, chats, bots, phase, error, onDaemonEvent, eventsConnected } = useDaemon();
  const runtime = appRuntime();
  // Observe this daemon and reuse the tested Stop retries, without a second desktop bridge.
  // No native snapshot/ack is owned here; the main window remains the native Stop owner.
  useEffect(() => {
    if (!client) return;
    const detach = runtime.attach(client, null);
    const off = onDaemonEvent((event) => runtime.onDaemonEvent(event));
    const timer = window.setInterval(() => runtime.tick(), 1000);
    return () => { window.clearInterval(timer); off(); detach(); };
  }, [client, onDaemonEvent, runtime]);
  useEffect(() => runtime.eventsConnected(eventsConnected), [eventsConnected, runtime]);
  const chat = useMemo(() => chats.find((c) => c.id === chatId) ?? null, [chats, chatId]);
  const bot = useMemo(
    () => (chat?.bot_id ? bots.find((b) => b.id === chat.bot_id) ?? null : null),
    [bots, chat]
  );
  const view = bot ? botView(bot) : null;
  const title = chat
    ? chatTitleLabel(chat.title, t("botPanel.untitled"))
    : t("popout.chatFallbackTitle");

  const handleNativeStop = useCallback(async () => {
    const snap = await stopBotRun();
    onControl(snap);
  }, [onControl]);

  return (
    <div className={styles.shell} data-custom-chrome={isTauriRuntime() && !settings.windowFrame ? "true" : undefined} data-testid="popout-chat-window">
      <PopoutHeader
        title={title}
        subtitle={view?.name ?? null}
        avatar={view}
        pinned={pinned}
        customChrome={isTauriRuntime() && !settings.windowFrame}
        onTogglePin={onTogglePin}
        onFocusMain={onFocusMain}
      />
      <div className={styles.body}>
        {phase === "error" ? (
          <div className={styles.statusState} role="alert">
            <h2 className={styles.statusTitle}>{t("main.daemonOfflineTitle")}</h2>
            {error ? <p>{humanLine(error)}</p> : null}
          </div>
        ) : phase === "connecting" && chats.length === 0 ? (
          <div className={styles.statusState} role="status">
            <LoaderCircle className={styles.spin} aria-hidden="true" />
            <p>{t("popout.connecting")}</p>
          </div>
        ) : (
          <ChatView
            key={chatId}
            chatId={chatId}
            control={control}
            onControl={onControl}
            showAgentRoute={settings.showAgentRoute}
            sendWith={settings.sendWith}
            messageTimes={settings.messageTimes}
            onNativeStop={handleNativeStop}
            hideHeader
          />
        )}
      </div>
    </div>
  );
}

function LivePopoutBody({
  source,
  botId,
  targetId,
  settings,
  pinned,
  onTogglePin,
  onFocusMain,
  onClose,
  onControl
}: {
  source: PopoutLiveSource;
  botId?: string;
  targetId?: string;
  settings: AppSettings;
  pinned: boolean;
  onTogglePin: () => void;
  onFocusMain: () => void;
  onClose: () => void;
  onControl: (next: ControlSnapshot) => void;
}) {
  const { t } = useTranslation();
  const { bots, phase, client, error } = useDaemon();
  const toast = useToast();
  const homeLive = useLiveView();
  const localLive = useLocalLiveView();
  const live = source === "local" ? localLive : homeLive;

  const selectTarget = live.selectTarget;
  useEffect(() => {
    if (targetId) selectTarget(targetId);
  }, [targetId, selectTarget]);

  const bot = useMemo(
    () => (botId ? bots.find((b) => b.id === botId) ?? null : null),
    [bots, botId]
  );
  const view = bot ? botView(bot) : null;

  const handleStop = useCallback(async () => {
    try {
      const snap = await stopBotRun();
      onControl(snap);
      if (client) await stopLiveRuns(client);
    } catch (err) {
      toast.show(humanLine(err), "danger");
    }
  }, [client, onControl, toast]);

  return (
    <div className={styles.shell} data-custom-chrome={isTauriRuntime() && !settings.windowFrame ? "true" : undefined} data-testid="popout-live-window">
      <PopoutHeader
        title={t("liveView.title")}
        subtitle={view?.name ?? (source === "local" ? t("account.localComputer") : null)}
        avatar={view}
        pinned={pinned}
        customChrome={isTauriRuntime() && !settings.windowFrame}
        onTogglePin={onTogglePin}
        onFocusMain={onFocusMain}
      />
      <div className={styles.liveStage}>
        {phase === "error" ? (
          <div className={styles.statusState} role="alert">
            <h2 className={styles.statusTitle}>{t("main.daemonOfflineTitle")}</h2>
            {error ? <p>{humanLine(error)}</p> : null}
          </div>
        ) : phase === "connecting" ? (
          <div className={styles.statusState} role="status">
            <LoaderCircle className={styles.spin} aria-hidden="true" />
            <p>{t("popout.connecting")}</p>
          </div>
        ) : (
          <LiveViewer
            source={source}
            botId={botId ?? null}
            embedded
            onClose={onClose}
            onStop={handleStop}
          />
        )}
      </div>
    </div>
  );
}

/** Root component rendered inside `chat-*` and `live-*` pop-out windows. */
function PopoutContent({
  route: initialRoute
}: {
  route: Extract<ResolvedWindowRoute, { kind: "chat" | "live" }>;
}) {
  const [route, setRoute] = useState(initialRoute);
  useEffect(() => {
    if (route.kind !== "live") return;
    return onLiveTargetUpdate((payload) => setRoute((prev) => prev.kind === "live" ? {
      ...prev,
      botId: payload.botId ?? undefined,
      targetId: payload.targetId ?? undefined,
      computerId: payload.computerId ?? undefined
    } : prev));
  }, [route.kind]);
  const shell = usePopoutShellState(route.pinned);
  const computerId = route.kind === "live" && route.source === "local"
    ? LOCAL_COMPUTER
    : route.computerId;

  return (
    <PopoutDaemonScope computerId={computerId}>
      <EngineConsentProvider>
        <LiveViewProvider
          key={computerId}
          scope={computerId === LOCAL_COMPUTER ? "local" : "home"}
          initialOpen={route.kind === "live"}
          focusBotId={route.kind === "live" ? route.botId : null}
          initialTargetId={route.kind === "live" ? route.targetId : null}
        >
          {route.kind === "chat" ? (
            <ChatPopoutBody
              chatId={route.chatId}
              settings={shell.settings}
              pinned={shell.pinned}
              onTogglePin={() => void shell.togglePinned()}
              onFocusMain={shell.focusMain}
              control={shell.control}
              onControl={shell.setControl}
            />
          ) : (
            <LivePopoutBody
              source={route.source}
              botId={route.botId}
              targetId={route.targetId}
              settings={shell.settings}
              pinned={shell.pinned}
              onTogglePin={() => void shell.togglePinned()}
              onFocusMain={shell.focusMain}
              onClose={shell.closeWindow}
              onControl={shell.setControl}
            />
          )}
        </LiveViewProvider>
      </EngineConsentProvider>
    </PopoutDaemonScope>
  );
}

/** A pop-out has its own providers and daemon stream; drafts and queued messages stay in it. */
export function PopoutWindow(props: { route: Extract<ResolvedWindowRoute, { kind: "chat" | "live" }> }) {
  return <ToastProvider><PopoutContent {...props} /></ToastProvider>;
}
