import { Command, Paperclip, Phone, Plus } from "lucide-react";
import { useCallback, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { Popover } from "./Popover";
import styles from "./ComposerPlusMenu.module.css";

export interface ComposerPlusMenuProps {
  /** Start a "/" command or "@" bot mention in the message field. */
  onCommands: () => void;
  onAttach: () => void;
  onVoice: () => void;
  attachDisabled: boolean;
  /** Text of the tooltip / accessible description of the voice row (why it cannot start, or what it does). */
  voiceTitle: string;
  voiceBlocked: boolean;
  voiceNotice?: string;
  voiceExpanded: boolean;
  voiceControls?: string;
  /** Where the attach limits are said (the old paperclip tooltip). */
  attachTitle: string;
  className?: string;
}

/**
 * The composer's one "+" button. The common things stay visible next to it (the model, the computer,
 * Send); the rarely used ones (a "/" command or a bot, attaching files, a voice call) live in this small
 * menu, so the message box shows three controls instead of five. Dragging files onto the box still works.
 */
export function ComposerPlusMenu({
  onCommands,
  onAttach,
  onVoice,
  attachDisabled,
  voiceTitle,
  voiceBlocked,
  voiceNotice,
  voiceExpanded,
  voiceControls,
  attachTitle,
  className
}: ComposerPlusMenuProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const close = useCallback(() => setOpen(false), []);
  const run = (action: () => void) => {
    setOpen(false);
    action();
  };
  return (
    <>
      <Button
        ref={trigger}
        variant="ghost"
        size="sm"
        className={className}
        title={t("chat.composerMenu.label")}
        aria-label={t("chat.composerMenu.label")}
        aria-haspopup="menu"
        aria-expanded={open}
        data-testid="composer-plus"
        onClick={() => setOpen((o) => !o)}
      >
        <Plus />
      </Button>
      <Popover
        open={open}
        anchor={trigger}
        onClose={close}
        placement="top-start"
        label={t("chat.composerMenu.label")}
        className={styles.menu}
      >
        <button type="button" role="menuitem" className={styles.item} data-testid="composer-plus-commands" onClick={() => run(onCommands)}>
          <Command aria-hidden="true" />
          <span>{t("chat.addCommand")}</span>
        </button>
        <button
          type="button"
          role="menuitem"
          className={styles.item}
          disabled={attachDisabled}
          title={attachTitle}
          data-testid="composer-plus-attach"
          onClick={() => run(onAttach)}
        >
          <Paperclip aria-hidden="true" />
          <span>{t("chat.attach.add")}</span>
        </button>
        <button
          type="button"
          role="menuitem"
          className={styles.item}
          title={voiceTitle}
          aria-disabled={voiceBlocked ? true : undefined}
          aria-description={voiceNotice}
          aria-expanded={voiceExpanded}
          aria-controls={voiceControls}
          data-testid="voice-call-start"
          onClick={() => run(onVoice)}
        >
          <Phone aria-hidden="true" />
          <span>{t("voice.call.start")}</span>
        </button>
      </Popover>
    </>
  );
}
