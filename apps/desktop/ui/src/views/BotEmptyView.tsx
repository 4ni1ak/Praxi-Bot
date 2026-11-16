import { Plus } from "lucide-react";
import { useTranslation } from "react-i18next";
import { BotAvatar, type BotIdentity, Button } from "ui";
import { FIRST_PROMPT_KEYS } from "../lib/firstRun";
import styles from "./BotEmptyView.module.css";

/**
 * What a bot without any chat shows in the center (owner feedback 244, point 4): the bot and
 * one action.  Everything else about the bot lives in the right panel.
 */
export function BotEmptyView({
  bot,
  busy,
  error,
  onNewChat,
  onSuggest
}: {
  bot: BotIdentity | undefined;
  busy: boolean;
  error: string | null;
  onNewChat: () => void;
  /** Starts a chat with this prompt already sent; omitted where suggestions make no sense. */
  onSuggest?: (prompt: string) => void;
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
        {onSuggest ? (
          <section className={styles.suggest} aria-label={t("botEmpty.suggestionsTitle")}>
            <h2 className={styles.suggestTitle}>{t("botEmpty.suggestionsTitle")}</h2>
            <div className={styles.suggestList}>
              {FIRST_PROMPT_KEYS.map((key) => (
                <button
                  key={key}
                  type="button"
                  className={styles.suggestChip}
                  disabled={busy}
                  data-testid={`first-prompt-${key}`}
                  onClick={() => onSuggest(t(`firstRun.prompts.${key}`))}
                >
                  {t(`firstRun.prompts.${key}`)}
                </button>
              ))}
            </div>
          </section>
        ) : null}
        {error ? (
          <p className={styles.error} role="alert">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  );
}
