import { Square, Volume2 } from "lucide-react";
import { useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { markdownToSpeech } from "../lib/speechText";
import { currentSpeakingId, speak, speechSupported, stopSpeaking, subscribeSpeaking } from "../lib/speaker";
import { DEFAULT_SETTINGS, loadSettingsSync } from "../store/settingsStore";
import styles from "./MessageInfo.module.css";

/** "Read aloud" beside a bot reply (ADR 0021): OS speech synthesis, replies are never sent anywhere. */
export function SpeakButton({ id, text }: { id: string; text: string }) {
  const { t, i18n } = useTranslation();
  const speakingId = useSyncExternalStore(subscribeSpeaking, currentSpeakingId, currentSpeakingId);
  if (!speechSupported() || !text.trim()) {
    return null;
  }
  const active = speakingId === id;
  const label = t(active ? "voice.stop" : "voice.speak");
  return (
    <button
      type="button"
      className={styles.info}
      aria-pressed={active}
      aria-label={label}
      title={label}
      data-testid="speak-reply"
      onClick={() => {
        if (active) {
          stopSpeaking();
          return;
        }
        // Read at click time from the settings mirror, so a changed voice applies at once.
        const prefs = loadSettingsSync() ?? DEFAULT_SETTINGS;
        speak(id, markdownToSpeech(text, { code: t("voice.codeOmitted"), table: t("voice.tableOmitted") }), {
          voiceName: prefs.voiceName,
          rate: prefs.voiceRate,
          lang: i18n.language
        });
      }}
    >
      {active ? <Square aria-hidden="true" /> : <Volume2 aria-hidden="true" />}
    </button>
  );
}
