import { useTranslation } from "react-i18next";
import { BotAvatar, type BotShapeId } from "ui";
import { CharacterColorRow, CharacterShapeGrid } from "./CharacterPicker";
import styles from "./BotIdentityPicker.module.css";

/**
 * Who the bot is: hero preview, name, one of 16 shapes and a colour. The one component behind
 * both the New-bot modal and the first-run flow, so the two never drift apart.
 */
export function BotIdentityPicker({
  previewId,
  name,
  shape,
  color,
  avatar,
  onName,
  onShape,
  onColor
}: {
  previewId: string;
  name: string;
  shape: BotShapeId;
  color: string;
  avatar?: string | null;
  onName: (name: string) => void;
  onShape: (shape: BotShapeId) => void;
  onColor: (color: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className={styles.root}>
      <div className={styles.preview}>
        <BotAvatar
          bot={{
            id: previewId,
            name: name.trim() || t("newBot.title"),
            shape,
            color,
            avatar: avatar ?? undefined
          }}
          size="hero"
          live
          decorative
        />
      </div>
      <div className={styles.nameWrap}>
        <input
          type="text"
          className={styles.nameInput}
          data-autofocus
          data-testid="new-bot-name"
          maxLength={24}
          value={name}
          placeholder={t("newBot.namePlaceholder")}
          aria-label={t("botForm.name")}
          onChange={(e) => onName(e.target.value)}
        />
      </div>
      <CharacterShapeGrid value={shape} color={color} onChange={onShape} />
      <CharacterColorRow value={color} onChange={onColor} />
    </div>
  );
}
