import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import formStyles from "../../components/Form.module.css";
import { SettingRow } from "../../components/SettingsUi";
import { listVoices, speak, speechSupported } from "../../lib/speaker";
import { VOICE_RATE_MAX, VOICE_RATE_MIN } from "../../lib/voicePrefs";
import type { AppSettings } from "../../store/settingsStore";
import styles from "../SettingsModal.module.css";

/** Voice and speed for the read-aloud button (OS speech synthesis; online voices are marked). */
export function VoiceSettings({
  settings,
  update
}: {
  settings: AppSettings;
  update: (patch: Partial<AppSettings>) => void;
}) {
  const { t, i18n } = useTranslation();
  const [voices, setVoices] = useState(listVoices);
  useEffect(() => {
    if (!speechSupported()) {
      return undefined;
    }
    const synth = window.speechSynthesis;
    const refresh = () => setVoices(listVoices());
    synth.addEventListener("voiceschanged", refresh);
    return () => synth.removeEventListener("voiceschanged", refresh);
  }, []);
  const title = t("voice.title");
  if (!speechSupported() || voices.length === 0) {
    return (
      <SettingRow title={title} description={t("voice.unsupported")}>
        <span />
      </SettingRow>
    );
  }
  return (
    <SettingRow title={title} description={t("voice.description")}>
      <select
        className={`${formStyles.select} ${styles.shortInput}`}
        aria-label={title}
        value={settings.voiceName}
        onChange={(e) => update({ voiceName: e.target.value })}
      >
        <option value="">{t("voice.auto")}</option>
        {voices.map((v) => (
          <option key={`${v.name}|${v.lang}`} value={v.name}>
            {`${v.name} (${v.lang})${v.localService ? "" : ` · ${t("voice.online")}`}`}
          </option>
        ))}
      </select>
      <input
        type="range"
        aria-label={t("voice.speed")}
        min={VOICE_RATE_MIN}
        max={VOICE_RATE_MAX}
        step={0.05}
        value={settings.voiceRate}
        onChange={(e) => update({ voiceRate: Number(e.target.value) })}
        onPointerUp={() =>
          speak("settings-preview", t("voice.title"), {
            voiceName: settings.voiceName,
            rate: settings.voiceRate,
            lang: i18n.language
          })
        }
      />
    </SettingRow>
  );
}
