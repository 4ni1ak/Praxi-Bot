import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { getBotVoice, setBotVoice } from "../lib/botVoice";
import { listVoices, speak, speechSupported } from "../lib/speaker";
import { DEFAULT_VOICE_RATE, VOICE_RATE_MAX, VOICE_RATE_MIN } from "../lib/voicePrefs";
import formStyles from "./Form.module.css";

/** Voice and speed this bot reads its replies with; empty = the global Settings choice. */
export function BotVoiceField({ botId }: { botId: string }) {
  const { t, i18n } = useTranslation();
  const [voices, setVoices] = useState(listVoices);
  const [own, setOwn] = useState(() => getBotVoice(botId));
  useEffect(() => {
    if (!speechSupported()) {
      return undefined;
    }
    const synth = window.speechSynthesis;
    const refresh = () => setVoices(listVoices());
    synth.addEventListener("voiceschanged", refresh);
    return () => synth.removeEventListener("voiceschanged", refresh);
  }, []);
  if (!speechSupported() || voices.length === 0) {
    return null;
  }
  const save = (next: { voiceName: string; rate: number } | null) => {
    setBotVoice(botId, next);
    setOwn(next);
  };
  const current = own ?? { voiceName: "", rate: DEFAULT_VOICE_RATE };
  return (
    <div data-testid="bot-voice">
      <label className={formStyles.label}>{t("voice.bot.title")}</label>
      <select
        className={formStyles.select}
        aria-label={t("voice.bot.title")}
        value={own ? current.voiceName : "\u0000global"}
        onChange={(e) =>
          e.target.value === "\u0000global" ? save(null) : save({ ...current, voiceName: e.target.value })
        }
      >
        <option value={"\u0000global"}>{t("voice.bot.useGlobal")}</option>
        <option value="">{t("voice.auto")}</option>
        {voices.map((v) => (
          <option key={`${v.name}|${v.lang}`} value={v.name}>
            {`${v.name} (${v.lang})`}
          </option>
        ))}
      </select>
      {own ? (
        <input
          type="range"
          aria-label={t("voice.speed")}
          min={VOICE_RATE_MIN}
          max={VOICE_RATE_MAX}
          step={0.05}
          value={own.rate}
          onChange={(e) => save({ ...own, rate: Number(e.target.value) })}
          onPointerUp={() => speak("bot-voice-preview", t("voice.bot.preview"), { voiceName: own.voiceName, rate: own.rate, lang: i18n.language })}
        />
      ) : null}
      <p className={formStyles.hint}>{t("voice.bot.hint")}</p>
    </div>
  );
}
