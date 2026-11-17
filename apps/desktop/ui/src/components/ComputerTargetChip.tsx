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

/** The key of the paired server a bot uses as ITS computer (`runtime.computer = "server:<key>"`), if any. */
export function serverComputerOf(bot: Pick<Bot, "config">): string | null {
  const runtime = (bot.config as Json | null)?.runtime as Json | undefined;
  const value = runtime?.computer;
  return typeof value === "string" && value.startsWith("server:") ? value.slice("server:".length) : null;
}

/**
 * "Its computer: this computer / <server>" for a bot that lives on THIS computer: the bot (its model
 * providers, keys and subscription CLIs) stays here and keeps working without signing in again, and
 * its browser tools act on the chosen paired server. This computer must be on for that to work.
 */
function BotServerChip({ bot }: { bot: Bot }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { client, hostList, refreshBots } = useDaemon();
  const [busy, setBusy] = useState(false);
  const servers = hostList?.hosts ?? [];
  if (!client || servers.length === 0) {
    return null;
  }
  const current = serverComputerOf(bot);
  const ids = [null, ...servers.map((h) => h.id)];
  const next = ids[(ids.indexOf(current) + 1) % ids.length] ?? null;
  const shown = current ? (servers.find((h) => h.id === current)?.name ?? t("home.server.unknown")) : t("home.server.thisComputer");
  const toggle = async () => {
    setBusy(true);
    try {
      await writeBotProfile(client, bot, (existing) => {
        const config = { ...((existing.config as Json | null) ?? {}) };
        config.runtime = { ...((config.runtime as Json | undefined) ?? {}), computer: next ? `server:${next}` : "auto" };
        return { name: existing.name, config } as never;
      });
      await refreshBots();
    } catch (e) {
      toast.show(botWriteMessage(e, t), "danger");
    } finally {
      setBusy(false);
    }
  };
  return (
    <button
      type="button"
      className={styles.chip}
      disabled={busy}
      title={t("home.server.title")}
      data-testid="bot-server-chip"
      data-server={current ?? ""}
      onClick={() => void toggle()}
    >
      <Monitor aria-hidden="true" />
      <span className={styles.label}>{t("home.server.label")}</span>
      <b>{shown}</b>
    </button>
  );
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
  if (home.kind !== "host" || workLocalNow) {
    // The bot lives here: its computer is this one, or a paired server used as a tool target.
    return <BotServerChip bot={bot} />;
  }
  if (!client) {
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
