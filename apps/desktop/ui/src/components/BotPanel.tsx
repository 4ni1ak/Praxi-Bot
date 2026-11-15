import {
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
import { useTranslation } from "react-i18next";
import { BotAvatar, type BotAvatarState, type BotIdentity } from "ui";
import { type PermissionTone, permissionTone } from "../lib/botEditor";
import { chatActivity, chatDateLabel } from "../lib/botLanding";
import type { PresetChoice } from "../lib/botProfile";
import { modelLine } from "../lib/modelName";
import type { BotSkillsState } from "../state/useBotSkills";
import { BotComputerBlock } from "./BotComputerBlock";
import { StopStatusNotice } from "./StopStatusNotice";
import styles from "./BotPanel.module.css";
import { BotPanelSkills } from "./BotPanelSkills";
import { BrandMark } from "./BrandMark";
import { QuotaBlock } from "./QuotaRings";

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
  state,
  chats,
  activeChatId,
  needs,
  runs,
  skills,
  newChatBusy,
  onClose,
  onOpenChat,
  onNewChat,
  onOpenProfile,
  onOpenSettings,
  onManageSkills,
  onAddSkill
}: {
  bot: BotIdentity;
  /** `provider/model` the bot runs on. */
  model?: string;
  preset: PresetChoice;
  state?: BotAvatarState;
  chats: readonly BotPanelChat[];
  activeChatId?: string;
  needs: ReadonlyArray<{ chatId: string }>;
  runs: ReadonlyArray<{ chat_id?: string | null; state: string }>;
  skills: BotSkillsState;
  newChatBusy: boolean;
  onClose: () => void;
  onOpenChat: (chatId: string) => void;
  onNewChat: () => void;
  onOpenProfile: () => void;
  onOpenSettings: () => void;
  onManageSkills: () => void;
  onAddSkill: () => void;
}) {
  const { t, i18n } = useTranslation();
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
        <BotAvatar bot={bot} size="hero" state={state ?? "idle"} decorative />
        <h2 className={styles.name}>{bot.name}</h2>
        <div className={styles.meta}>
          <ModelLineText model={model} />
          <PermissionChip preset={preset} />
        </div>
      </header>

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
        <div className={styles.actionRow}>
          <button
            type="button"
            className={styles.pill}
            data-testid="bot-panel-edit"
            onClick={onOpenSettings}
          >
            <PenLine aria-hidden="true" />
            {t("botPanel.edit")}
          </button>
          <button
            type="button"
            className={styles.pill}
            data-testid="bot-panel-profile"
            onClick={onOpenProfile}
          >
            <UserRound aria-hidden="true" />
            {t("botPanel.profile")}
          </button>
        </div>
      </div>

      <BotPanelSkills skills={skills} onManage={onManageSkills} onAdd={onAddSkill} />

      <section className={styles.section} aria-labelledby="bot-panel-chats">
        <h3 className={styles.label} id="bot-panel-chats">
          {t("botPanel.chats")}
        </h3>
        {chats.length === 0 ? (
          <p className={styles.empty}>{t("botPanel.noChats")}</p>
        ) : (
          <ul className={styles.list}>
            {chats.map((c) => {
              const activity = chatActivity(c.id, needs, runs);
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
                      <span className={styles.chatTitle}>{c.title || t("botPanel.untitled")}</span>
                      <span className={styles.chatDate} data-testid="bot-panel-chat-date">
                        {chatDateLabel(c.createdAtMs, i18n.language)}
                      </span>
                    </span>
                    {activity ? (
                      <span className={styles.chatState} data-state={activity}>
                        {t(`bots.chatState.${activity}`)}
                      </span>
                    ) : null}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </section>
      <StopStatusNotice variant="panel" />
      <BotComputerBlock />
      <QuotaBlock />
    </div>
  );
}
