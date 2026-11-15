import { Plus } from "lucide-react";
import { useTranslation } from "react-i18next";
import { BotAvatar, type BotIdentity, Button } from "ui";
import styles from "./BotEmptyView.module.css";

/**
 * What a bot without any chat shows in the center (owner feedback 244, point 4): the bot and
 * one action.  Everything else about the bot lives in the right panel.
 */
export function BotEmptyView({
  bot,
  busy,
  error,
  onNewChat
}: {
  bot: BotIdentity | undefined;
  busy: boolean;
  error: string | null;
  onNewChat: () => void;
}) {
  const { t } = useTranslation();
  if (!bot) {
    return (
      <div className={styles.pane}>
        <p className={styles.text}>{t("bots.notFound")}</p>
      </div>
    );
  }
  return (
    <div className={styles.pane} data-testid="bot-empty-state">
      <div className={styles.stage}>
        <BotAvatar bot={bot} size="hero" decorative />
        <h1 className={styles.title}>{t("botEmpty.title", { name: bot.name })}</h1>
        <p className={styles.text}>{t("botEmpty.body")}</p>
        <Button
          variant="primary"
          size="lg"
          disabled={busy}
          data-testid="bot-empty-new-chat"
          onClick={onNewChat}
        >
          <Plus aria-hidden="true" />
          {t("bots.newChat")}
        </Button>
        {error ? (
          <p className={styles.error} role="alert">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  );
}
