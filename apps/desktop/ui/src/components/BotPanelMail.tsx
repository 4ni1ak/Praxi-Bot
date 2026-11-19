import { AtSign, Mail, PlugZap, Plus, SlidersHorizontal } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { EmailAccount, EmailTestReport } from "../api/types";
import { accountsForBot, MAIL_CHANGED_EVENT, mailToolsOn, mailToolsPatch } from "../lib/botMail";
import { humanLine } from "../lib/humanError";
import { useDaemon } from "../state/DaemonContext";
import { TestResult } from "../views/settings/EmailCard";
import { Switch } from "./SettingsUi";
import { useToast } from "./Toast";
import styles from "./BotPanelMail.module.css";

function MailboxRow({ account, botName }: { account: EmailAccount; botName: string }) {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const { client } = useDaemon();
  const [report, setReport] = useState<EmailTestReport | null>(null);
  const [checkedAt, setCheckedAt] = useState<Date | null>(null);
  const [checking, setChecking] = useState(false);
  const agentmail = account.provider === "agentmail";

  const check = async () => {
    if (!client) return;
    setChecking(true);
    try {
      setReport(await client.testEmailAccount(account.name));
      setCheckedAt(new Date());
    } catch (e) {
      toast.show(humanLine(e), "danger");
    } finally {
      setChecking(false);
    }
  };

  return (
    <li className={styles.box} data-testid={`bot-mail-${account.name}`}>
      <div className={styles.address}>
        {agentmail ? <AtSign aria-hidden="true" /> : <Mail aria-hidden="true" />}
        <span className={styles.addressText} title={account.address}>
          {account.address}
        </span>
      </div>
      <div className={styles.tags}>
        {agentmail ? <span className={styles.tag}>{t("botPanel.mail.agentmail")}</span> : null}
        <span className={styles.tag}>
          {account.bot ? t("botPanel.mail.own", { name: botName }) : t("botPanel.mail.shared")}
        </span>
      </div>
      {report ? <TestResult report={report} /> : null}
      <div className={styles.row}>
        <button
          type="button"
          className={styles.textButton}
          disabled={checking}
          data-testid={`bot-mail-check-${account.name}`}
          onClick={() => void check()}
        >
          <PlugZap aria-hidden="true" />
          {checking ? t("botPanel.mail.checking") : t("botPanel.mail.check")}
        </button>
        {checkedAt ? (
          <span className={styles.muted}>
            {t("botPanel.mail.checked", {
              time: checkedAt.toLocaleTimeString(i18n.language, { hour: "2-digit", minute: "2-digit" })
            })}
          </span>
        ) : null}
      </div>
    </li>
  );
}

/**
 * Mail in the right panel: the mailboxes this bot may use, whether it is offered the mail tools,
 * and a way to check the connection.  Adding or removing a mailbox happens in Settings, Mail.
 */
export function BotPanelMail({
  botId,
  botName,
  onOpenMail
}: {
  botId: string;
  botName: string;
  onOpenMail: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const { client, bots, refreshBots } = useDaemon();
  const bot = bots.find((b) => b.id === botId);
  const [accounts, setAccounts] = useState<EmailAccount[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    if (!client) return;
    try {
      setAccounts(await client.listEmailAccounts());
      setError(null);
    } catch (e) {
      setError(humanLine(e));
    }
  }, [client]);

  useEffect(() => {
    setAccounts(null);
    void load();
    window.addEventListener(MAIL_CHANGED_EVENT, load);
    return () => window.removeEventListener(MAIL_CHANGED_EVENT, load);
  }, [load, botId]);

  const mine = accounts ? accountsForBot(accounts, botId) : [];
  const toolsOn = bot ? mailToolsOn(bot.config) : false;

  const setTools = async (on: boolean) => {
    if (!client || !bot) return;
    const patch = mailToolsPatch(bot, on);
    if (patch.ops.length === 0) return;
    setSaving(true);
    try {
      await client.patchBot(bot.id, patch);
      await refreshBots();
    } catch (e) {
      toast.show(t("botPanel.mail.toolsError", { message: humanLine(e) }), "danger");
    } finally {
      setSaving(false);
    }
  };

  return (
    <section aria-labelledby="bot-panel-mail" data-testid="bot-panel-mail">
      <div className={styles.head}>
        <h3 className={styles.label} id="bot-panel-mail">
          {t("botPanel.mail.heading")}
        </h3>
        <button
          type="button"
          className={styles.tool}
          title={t("botPanel.mail.manage")}
          aria-label={t("botPanel.mail.manage")}
          data-testid="bot-panel-mail-manage"
          onClick={onOpenMail}
        >
          <SlidersHorizontal aria-hidden="true" />
        </button>
      </div>
      {error ? (
        <p className={styles.error} role="alert">
          {t("botPanel.mail.loadError", { message: error })}
        </p>
      ) : accounts === null ? (
        <p className={styles.muted}>{t("botPanel.mail.loading")}</p>
      ) : mine.length === 0 ? (
        <div className={styles.empty}>
          <p className={styles.muted}>{t("botPanel.mail.empty", { name: botName })}</p>
          <button type="button" className={styles.textButton} data-testid="bot-panel-mail-connect" onClick={onOpenMail}>
            <Plus aria-hidden="true" />
            {t("botPanel.mail.connect")}
          </button>
        </div>
      ) : (
        <>
          <ul className={styles.list}>
            {mine.map((a) => (
              <MailboxRow key={a.name} account={a} botName={botName} />
            ))}
          </ul>
          <div className={styles.row}>
            <span className={styles.switchLabel}>{t("botPanel.mail.toolsLabel", { name: botName })}</span>
            <Switch
              checked={toolsOn}
              disabled={saving || !bot}
              label={t("botPanel.mail.toolsLabel", { name: botName })}
              onChange={(on) => void setTools(on)}
            />
          </div>
          <p className={styles.muted} data-testid="bot-mail-tools-hint">
            {toolsOn ? t("botPanel.mail.toolsOnHint") : t("botPanel.mail.toolsOffHint", { name: botName })}
          </p>
        </>
      )}
    </section>
  );
}
