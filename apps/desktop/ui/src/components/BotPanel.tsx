import {
  ChevronRight,
  Lock,
  MessageSquare,
  PenLine,
  Plus,
  ShieldCheck,
  SlidersHorizontal,
  UserRound,
  X,
  Zap
} from "lucide-react";
import { type CSSProperties, Fragment, useState } from "react";
import { useTranslation } from "react-i18next";
import { BotAvatar, type BotAvatarState, type BotIdentity } from "ui";
import { activitySections, librarySections, subagentRows } from "../lib/activityPanel";
import { type PermissionTone, permissionTone } from "../lib/botEditor";
import { chatActivity, chatDateLabel } from "../lib/botLanding";
import { CHAT_SEARCH_MIN_CHATS, filterChats } from "../lib/chatSearch";
import { chatTitleLabel } from "../lib/chatTitle";
import type { PresetChoice } from "../lib/botProfile";
import { modelLine } from "../lib/modelName";
import { runNow } from "../lib/runNow";
import { toolSentence } from "../lib/runSentence";
import { cancelRun } from "../state/appStop";
import { useDaemon } from "../state/DaemonContext";
import { useQuotas } from "../state/useQuotas";
import type { BotSkillsState } from "../state/useBotSkills";
import { BotComputerBlock } from "./BotComputerBlock";
import { StopStatusNotice } from "./StopStatusNotice";
import styles from "./BotPanel.module.css";
import { BotPanelMail } from "./BotPanelMail";
import { BotPanelSkills } from "./BotPanelSkills";
import { BrandMark } from "./BrandMark";
import { QuotaBlock } from "./QuotaRings";
import { SubagentDetail } from "./SubagentDetail";
import { SubagentList, useNow } from "./SubagentList";
import { formatElapsed } from "../lib/activityPanel";
import { UpcomingBlock, useUpcoming } from "./UpcomingBlock";

export interface BotPanelChat {
  id: string;
  title: string;
  createdAtMs: number;
}

const TONE_ICONS: Record<PermissionTone, typeof Lock> = {
  locked: Lock,
  guarded: ShieldCheck,
  open: Zap,
  custom: SlidersHorizontal
};

/** The permission mode as a small tinted chip (shared with the profile dialog). */
export function PermissionChip({ preset }: { preset: PresetChoice }) {
  const { t } = useTranslation();
  const tone = permissionTone(preset);
  const Icon = TONE_ICONS[tone];
  return (
    <span
      className={styles.chip}
      data-tone={tone}
      title={t(`botForm.preset.${preset}.hint`)}
      data-testid="bot-permission-chip"
    >
      <Icon aria-hidden="true" />
      <span>{t(`botForm.preset.${preset}.name`)}</span>
    </span>
  );
}

/** The model a bot runs on, by its human name (never the raw id). */
export function ModelLineText({ model }: { model: string | undefined }) {
  const { t } = useTranslation();
  const line = modelLine(model);
  if (!line.name) {
    return <span className={styles.modelMuted}>{t("bots.noModel")}</span>;
  }
  return (
    <span className={styles.modelLine} title={model} data-testid="bot-model-line">
      {line.brand ? <BrandMark name={line.brand} size={14} /> : null}
      <span className={styles.modelName}>{line.name}</span>
    </span>
  );
}

/**
 * Right panel of the app shell: everything about the selected bot in one place (owner feedback
 * 244, point 4) - who it is, the quick actions, its model and permission mode, its skills, its
 * chats and the computer it works on.  The center only ever shows a chat.
 */
export function BotPanel({
  bot,
  model,
  preset,
  engineCli,
  state,
  chats,
  activeChatId,
  needs,
  runs,
  skills,
  newChatBusy,
  visible = true,
  onClose,
  onOpenChat,
  onNewChat,
  onOpenProfile,
  onOpenSettings,
  onManageSkills,
  onAddSkill,
  onOpenMail
}: {
  bot: BotIdentity;
  /** `provider/model` the bot runs on. */
  model?: string;
  preset: PresetChoice;
  /** Name of the CLI that runs this bot's files and shell (native-tools engine), if any. */
  engineCli?: string | null;
  state?: BotAvatarState;
  chats: readonly BotPanelChat[];
  activeChatId?: string;
  needs: ReadonlyArray<{ chatId: string }>;
  runs: ReadonlyArray<{ chat_id?: string | null; state: string }>;
  skills: BotSkillsState;
  newChatBusy: boolean;
  /** The panel is on screen (closing it ends the computer view). */
  visible?: boolean;
  onClose: () => void;
  onOpenChat: (chatId: string) => void;
  onNewChat: () => void;
  onOpenProfile: () => void;
  onOpenSettings: () => void;
  onManageSkills: () => void;
  onAddSkill: () => void;
  /** Open Settings, Mail (connect, edit or remove a mailbox). */
  onOpenMail: () => void;
}) {
  const { t, i18n } = useTranslation();
  const { events, refreshRuns } = useDaemon();
  const [tab, setTab] = useState<"activity" | "library">("activity");
  const [openSub, setOpenSub] = useState<string | null>(null);
  const [chatQuery, setChatQuery] = useState("");
  const visibleChats = filterChats(chats, chatQuery, t("botPanel.untitled"));
  const subs = subagentRows(events, activeChatId);
  const upcoming = useUpcoming(bot.id);
  const quotas = useQuotas();
  const activity = activitySections({ subagents: subs.length, upcoming: upcoming.rows.length, quota: quotas.length });
  const now = runNow(events, activeChatId);
  const nowTick = useNow(now !== null);
  const library = librarySections({ chats: chats.length });
  const stopSub = (id: string) => {
    cancelRun(id);
    void refreshRuns();
  };
  return (
    <div className={styles.root} id="bot-panel" data-testid="bot-panel">
      <button
        type="button"
        className={styles.close}
        title={t("botPanel.close")}
        aria-label={t("botPanel.close")}
        onClick={onClose}
      >
        <X aria-hidden="true" />
      </button>

      <header className={styles.hero}>
        <div className={styles.ava} style={{ "--bot": bot.color } as CSSProperties}>
          <BotAvatar bot={bot} size="panel" state={state ?? "idle"} decorative />
        </div>
        <h2 className={styles.name}>{bot.name}</h2>
        <div className={styles.meta}>
          <ModelLineText model={model} />
          <PermissionChip preset={preset} />
          {engineCli ? <span data-testid="bot-engine-label">{t("botEngine.label", { cli: engineCli })}</span> : null}
        </div>
        <div className={styles.actions}>
          <button
            type="button"
            className={styles.primary}
            disabled={newChatBusy}
            data-testid="bot-panel-new-chat"
            onClick={onNewChat}
          >
            <Plus aria-hidden="true" />
            {t("bots.newChat")}
          </button>
          <button
            type="button"
            className={styles.iconBtn}
            data-testid="bot-panel-edit"
            title={t("botPanel.edit")}
            aria-label={t("botPanel.edit")}
            onClick={onOpenSettings}
          >
            <PenLine aria-hidden="true" />
          </button>
          <button
            type="button"
            className={styles.iconBtn}
            data-testid="bot-panel-profile"
            title={t("botPanel.profile")}
            aria-label={t("botPanel.profile")}
            onClick={onOpenProfile}
          >
            <UserRound aria-hidden="true" />
          </button>
        </div>
      </header>

      <div className={styles.tabs} role="tablist" aria-label={t("botPanel.tabs.label")}>
        {(["activity", "library"] as const).map((id) => (
          <button
            key={id}
            type="button"
            role="tab"
            id={`bot-panel-tab-${id}`}
            aria-selected={tab === id}
            aria-controls="bot-panel-tabpanel"
            tabIndex={tab === id ? 0 : -1}
            className={styles.tab}
            data-testid={`bot-panel-tab-${id}`}
            onClick={() => setTab(id)}
            onKeyDown={(e) => {
              if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
                e.preventDefault();
                const next = id === "activity" ? "library" : "activity";
                setTab(next);
                document.getElementById(`bot-panel-tab-${next}`)?.focus();
              }
            }}
          >
            {t(`botPanel.tabs.${id}`)}
          </button>
        ))}
      </div>

      <div className={styles.tabPanel} role="tabpanel" id="bot-panel-tabpanel" aria-labelledby={`bot-panel-tab-${tab}`}>
        {tab === "activity" ? (
          <>
            <StopStatusNotice variant="panel" />
            {now ? (
              <div className={styles.now} data-kind={now.approval ? "approval" : "run"} data-testid="bot-panel-now">
                <span className={styles.pulse} aria-hidden="true" />
                <div className={styles.nowText}>
                  <b>
                    {now.approval
                      ? t("botPanel.now.waiting")
                      : now.step
                        ? toolSentence(t, now.step)
                        : t("botPanel.now.thinking")}
                  </b>
                  <small>
                    {[
                      now.subagents > 0 ? t("botPanel.now.subagents", { count: now.subagents }) : null,
                      formatElapsed(nowTick - now.startedMs)
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </small>
                </div>
                <button
                  type="button"
                  className={styles.nowOpen}
                  title={t("botPanel.now.open")}
                  aria-label={t("botPanel.now.open")}
                  data-testid="bot-panel-now-open"
                  onClick={() => setOpenSub(now.run.id)}
                >
                  <ChevronRight aria-hidden="true" />
                </button>
              </div>
            ) : null}
            {activity.map((id) => {
              switch (id) {
                case "subagents":
                  return <SubagentList key={id} rows={subs} onOpen={setOpenSub} onStop={stopSub} />;
                case "upcoming":
                  return <UpcomingBlock key={id} rows={upcoming.rows} onToggle={upcoming.toggle} />;
                case "computer":
                  return <BotComputerBlock key={id} botName={bot.name} botId={bot.id} botColor={bot.color} visible={visible} />;
                default:
                  return <QuotaBlock key={id} />;
              }
            })}
          </>
        ) : (
          library.map((id) =>
            id === "skills" ? (
              <Fragment key={id}>
                <BotPanelSkills skills={skills} onManage={onManageSkills} onAdd={onAddSkill} />
                <BotPanelMail botId={bot.id} botName={bot.name} onOpenMail={onOpenMail} />
              </Fragment>
            ) : (
              <div key={id}>
        <section className={styles.section} aria-labelledby="bot-panel-chats">
        <h3 className={styles.label} id="bot-panel-chats">
          {t("botPanel.chats")}
        </h3>
        {chats.length >= CHAT_SEARCH_MIN_CHATS ? (
          <input
            type="search"
            className={styles.chatSearch}
            value={chatQuery}
            placeholder={t("botPanel.searchChats")}
            aria-label={t("botPanel.searchChats")}
            onChange={(e) => setChatQuery(e.target.value)}
          />
        ) : null}
        {visibleChats.length === 0 && chatQuery.trim() ? (
          <p className={styles.chatSearchEmpty} role="status">{t("botPanel.noChatMatches")}</p>
        ) : null}
        <ul className={styles.list}>
            {visibleChats.map((c) => {
              const chatState = chatActivity(c.id, needs, runs);
              return (
                <li key={c.id}>
                  <button
                    type="button"
                    className={styles.chat}
                    aria-current={c.id === activeChatId ? "true" : undefined}
                    onClick={() => onOpenChat(c.id)}
                  >
                    <MessageSquare aria-hidden="true" />
                    <span className={styles.chatText}>
                      <span className={styles.chatTitle}>{chatTitleLabel(c.title, t("botPanel.untitled"))}</span>
                      <span className={styles.chatDate} data-testid="bot-panel-chat-date">
                        {chatDateLabel(c.createdAtMs, i18n.language)}
                      </span>
                    </span>
                    {chatState ? (
                      <span className={styles.chatState} data-state={chatState}>
                        {t(`bots.chatState.${chatState}`)}
                      </span>
                    ) : null}
                  </button>
                </li>
              );
            })}
        </ul>
      </section>
              </div>
            )
          )
        )}
      </div>
      <SubagentDetail
        run={openSub ? events.runs[openSub] : undefined}
        fallbackTitle={t("botPanel.now.detailTitle")}
        onClose={() => setOpenSub(null)}
        onStop={stopSub}
      />
    </div>
  );
}
