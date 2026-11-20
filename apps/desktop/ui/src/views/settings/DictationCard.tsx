import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import type { VoiceConfig } from "../../api/types";
import formStyles from "../../components/Form.module.css";
import { SettingRow } from "../../components/SettingsUi";
import { useToast } from "../../components/Toast";
import { humanLine } from "../../lib/humanError";
import { useDaemon } from "../../state/DaemonContext";

/** Where dictation audio is transcribed (an OpenAI-compatible service); the key is write-only. */
export function DictationCard() {
  const { t } = useTranslation();
  const toast = useToast();
  const { client } = useDaemon();
  const [config, setConfig] = useState<VoiceConfig | null>(null);
  const [url, setUrl] = useState("");
  const [model, setModel] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);

  const apply = useCallback((c: VoiceConfig) => {
    setConfig(c);
    setUrl(c.stt_base_url);
    setModel(c.stt_model);
  }, []);
  useEffect(() => {
    client?.getVoice().then(apply).catch(() => undefined);
  }, [client, apply]);

  const save = async (input: { stt_key?: string; clear_stt_key?: boolean } = {}) => {
    if (!client) {
      return;
    }
    setBusy(true);
    try {
      apply(await client.updateVoice({ stt_base_url: url, stt_model: model, ...input }));
      setKey("");
      toast.show(t("voice.stt.saved"), "success");
    } catch (e) {
      toast.show(humanLine(e), "danger");
    } finally {
      setBusy(false);
    }
  };

  if (!config) {
    return null;
  }
  return (
    <SettingRow stacked title={t("voice.stt.title")} description={`${t("voice.stt.description")} ${t("voice.stt.privacy")}`}>
      <input
        className={formStyles.input}
        aria-label={t("voice.stt.url")}
        value={url}
        placeholder="https://api.openai.com/v1"
        onChange={(e) => setUrl(e.target.value)}
      />
      <input
        className={formStyles.input}
        aria-label={t("voice.stt.model")}
        value={model}
        placeholder="whisper-1"
        onChange={(e) => setModel(e.target.value)}
      />
      <input
        className={formStyles.input}
        type="password"
        autoComplete="off"
        aria-label={t("voice.stt.key")}
        value={key}
        placeholder={config.stt_key_set ? t("voice.stt.keyStored") : t("voice.stt.key")}
        onChange={(e) => setKey(e.target.value)}
      />
      <Button size="sm" disabled={busy} onClick={() => void save(key ? { stt_key: key } : {})}>
        {t("voice.stt.save")}
      </Button>
      {config.stt_key_set ? (
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void save({ clear_stt_key: true })}>
          {t("voice.stt.clearKey")}
        </Button>
      ) : null}
    </SettingRow>
  );
}
