import { Monitor, Server } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { Bot } from "../api/types";
import { botWriteMessage, writeBotProfile } from "../lib/botWrite";
import { useDaemon } from "../state/DaemonContext";
import { useToast } from "./Toast";
import styles from "./ComputerTargetChip.module.css";

type Json = Record<string, unknown>;

/** The bot's `runtime.computer` as the chip shows it: its own computer (`auto`, `host`) or mine (`pc`). */
export function computerTargetOf(bot: Pick<Bot, "config">): "bot" | "mine" {
  const runtime = (bot.config as Json | null)?.runtime as Json | undefined;
  return runtime?.computer === "pc" ? "mine" : "bot";
}

/**
 * "Runs on: bot's computer / my computer" for the open chat's bot. It is the bot's own
 * `runtime.computer` setting, saved through the versioned bot write; shown only while the bots'
 * home is a host (without one there is only this computer).
 */
export function ComputerTargetChip({ bot }: { bot: Bot }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { client, home, workLocalNow, refreshBots } = useDaemon();
  const [busy, setBusy] = useState(false);
  if (home.kind !== "host" || workLocalNow || !client) {
    return null;
  }
  const value = computerTargetOf(bot);
  const next = value === "bot" ? "pc" : "host";
  const toggle = async () => {
    setBusy(true);
    try {
      await writeBotProfile(client, bot, (current) => {
        const config = { ...((current.config as Json | null) ?? {}) };
        config.runtime = { ...((config.runtime as Json | undefined) ?? {}), computer: next };
        return { name: current.name, config } as never;
      });
      await refreshBots();
    } catch (e) {
      toast.show(botWriteMessage(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  const Icon = value === "bot" ? Server : Monitor;
  return (
    <button
      type="button"
      className={styles.chip}
      disabled={busy}
      title={t("home.target.title")}
      data-testid="computer-target-chip"
      data-target={value}
      onClick={() => void toggle()}
    >
      <Icon aria-hidden="true" />
      <span className={styles.label}>{t("home.target.label")}</span>
      <b>{value === "bot" ? t("home.target.bot") : t("home.target.mine")}</b>
    </button>
  );
}
