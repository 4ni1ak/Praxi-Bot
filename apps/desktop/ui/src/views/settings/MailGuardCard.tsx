import { Download, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Card } from "ui";
import type { EmailGuardSettings, EmailOptOut, EmailSendLogEntry } from "../../api/types";
import formStyles from "../../components/Form.module.css";
import { useToast } from "../../components/Toast";
import { type HumanError, humanError, humanLine } from "../../lib/humanError";
import { formatDateTime } from "../../lib/messageTime";
import { saveTextFile } from "../../lib/runReport";
import { useDaemon } from "../../state/DaemonContext";

const LOG_ROWS = 100;

/** "Safe sending": the limits, the opt-out list and the log of every mail the bots sent. */
export function MailGuardCard() {
  const { t, i18n } = useTranslation();
  const toast = useToast();
  const { client } = useDaemon();
  const [settings, setSettings] = useState<EmailGuardSettings | null>(null);
  const [optOuts, setOptOuts] = useState<EmailOptOut[]>([]);
  const [log, setLog] = useState<EmailSendLogEntry[]>([]);
  const [entry, setEntry] = useState("");
  const [domains, setDomains] = useState("");
  const [busy, setBusy] = useState(false);

  // The three parts load on their own: a part that cannot be read (an older host, a refused list)
  // leaves a small note where it would be, it never blocks the settings or the other parts.
  const [problems, setProblems] = useState<Record<"settings" | "optOuts" | "log", HumanError | null>>({
    settings: null,
    optOuts: null,
    log: null
  });

  const load = useCallback(async () => {
    if (!client) return;
    const [s, o, l] = await Promise.allSettled([
      client.getMailGuardSettings(),
      client.listMailOptOuts(),
      client.listMailSendLog(LOG_ROWS)
    ]);
    if (s.status === "fulfilled") {
      setSettings(s.value);
      setDomains((s.value.verification_link_domains ?? []).join("\n"));
    }
    if (o.status === "fulfilled") setOptOuts(o.value);
    if (l.status === "fulfilled") setLog(l.value);
    setProblems({
      settings: s.status === "rejected" ? humanError(s.reason) : null,
      optOuts: o.status === "rejected" ? humanError(o.reason) : null,
      log: l.status === "rejected" ? humanError(l.reason) : null
    });
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const act = async (work: () => Promise<void>) => {
    setBusy(true);
    try {
      await work();
    } catch (e) {
      toast.show(humanLine(e), "danger");
    } finally {
      setBusy(false);
    }
  };

  const number = (v: string) => Math.max(0, Math.min(100000, Number.parseInt(v, 10) || 0));

  const note = (problem: HumanError | null, testId: string) =>
    problem ? (
      <p className={formStyles.hint} role="status" data-testid={testId}>
        {problem.title}
        {problem.hint ? ` ${problem.hint}` : ""}{" "}
        <Button size="sm" variant="ghost" onClick={() => void load()}>{t("settingsModal.mailGuard.retry")}</Button>
      </p>
    ) : null;

  if (!settings) {
    return (
      <Card>
        {problems.settings ? note(problems.settings, "mail-guard-load-error") : t("settingsModal.mailGuard.loading")}
      </Card>
    );
  }
  return (
    <Card>
      <div data-testid="mail-guard">
        <p className={formStyles.hint}>{t("settingsModal.mailGuard.intro")}</p>

        <div className={formStyles.row}>
          <div className={formStyles.field}>
            <label className={formStyles.label} htmlFor="mg-daily">{t("settingsModal.mailGuard.daily")}</label>
            <input id="mg-daily" className={formStyles.input} type="number" min={0}
              value={settings.daily_limit}
              onChange={(e) => setSettings({ ...settings, daily_limit: number(e.target.value) })} />
          </div>
          <div className={formStyles.field}>
            <label className={formStyles.label} htmlFor="mg-days">{t("settingsModal.mailGuard.days")}</label>
            <input id="mg-days" className={formStyles.input} type="number" min={0}
              value={settings.same_recipient_days}
              onChange={(e) => setSettings({ ...settings, same_recipient_days: number(e.target.value) })} />
          </div>
        </div>
        <p className={formStyles.hint}>{t("settingsModal.mailGuard.softHint")}</p>

        <label className={formStyles.label}>
          <input type="checkbox" checked={settings.opt_out_line}
            onChange={(e) => setSettings({ ...settings, opt_out_line: e.target.checked })} />{" "}
          {t("settingsModal.mailGuard.optOutLine")}
        </label>
        <div className={formStyles.field}>
          <label className={formStyles.label} htmlFor="mg-footer">{t("settingsModal.mailGuard.footer")}</label>
          <textarea id="mg-footer" className={formStyles.textarea} rows={3} maxLength={1000}
            placeholder={t("settingsModal.mailGuard.footerPlaceholder")}
            value={settings.footer}
            onChange={(e) => setSettings({ ...settings, footer: e.target.value })} />
          <p className={formStyles.hint}>{t("settingsModal.mailGuard.footerHint")}</p>
        </div>
        <div className={formStyles.field}>
          <label className={formStyles.label} htmlFor="mg-verify">{t("settingsModal.mailGuard.verifyDomains")}</label>
          <textarea id="mg-verify" className={formStyles.textarea} rows={3} maxLength={4000}
            data-testid="mail-guard-verify-domains"
            placeholder={t("settingsModal.mailGuard.verifyDomainsPlaceholder")}
            value={domains}
            onChange={(e) => setDomains(e.target.value)} />
          <p className={formStyles.hint}>{t("settingsModal.mailGuard.verifyDomainsHint")}</p>
        </div>
        <Button size="sm" disabled={busy} data-testid="mail-guard-save"
          onClick={() => void act(async () => {
            const saved = await client!.putMailGuardSettings({
              ...settings,
              verification_link_domains: domains.split(/[\s,;]+/).filter(Boolean)
            });
            setSettings(saved);
            setDomains((saved.verification_link_domains ?? []).join("\n"));
            toast.show(t("settingsModal.mailGuard.saved"), "success");
          })}>
          {t("settingsModal.mailGuard.save")}
        </Button>

        <h3>{t("settingsModal.mailGuard.optOutTitle")}</h3>
        <p className={formStyles.hint}>{t("settingsModal.mailGuard.optOutHint")}</p>
        <div className={formStyles.row}>
          <input className={formStyles.input} aria-label={t("settingsModal.mailGuard.optOutAdd")}
            placeholder={t("settingsModal.mailGuard.optOutPlaceholder")}
            value={entry} onChange={(e) => setEntry(e.target.value)} data-testid="mail-guard-optout-input" />
          <Button size="sm" variant="secondary" disabled={busy || entry.trim() === ""}
            data-testid="mail-guard-optout-add"
            onClick={() => void act(async () => {
              await client!.addMailOptOut(entry.trim());
              setEntry("");
              setOptOuts(await client!.listMailOptOuts());
            })}>
            {t("settingsModal.mailGuard.optOutAdd")}
          </Button>
        </div>
        {note(problems.optOuts, "mail-guard-optouts-error")}
        {problems.optOuts ? null : optOuts.length === 0 ? <p className={formStyles.hint}>{t("settingsModal.mailGuard.optOutEmpty")}</p> : (
          <ul data-testid="mail-guard-optouts">
            {optOuts.map((o) => (
              <li key={o.value}>
                {o.value}{" "}
                <span className={formStyles.hint}>
                  {t(o.kind === "domain" ? "settingsModal.mailGuard.kindDomain" : "settingsModal.mailGuard.kindAddress")}
                </span>{" "}
                <Button size="sm" variant="ghost" disabled={busy}
                  aria-label={t("settingsModal.mailGuard.optOutRemove", { value: o.value })}
                  title={t("settingsModal.mailGuard.optOutRemove", { value: o.value })}
                  onClick={() => void act(async () => {
                    await client!.removeMailOptOut(o.value);
                    setOptOuts(await client!.listMailOptOuts());
                  })}>
                  <Trash2 size={14} aria-hidden />
                </Button>
              </li>
            ))}
          </ul>
        )}

        <h3>{t("settingsModal.mailGuard.logTitle")}</h3>
        <p className={formStyles.hint}>{t("settingsModal.mailGuard.logHint")}</p>
        <Button size="sm" variant="secondary" disabled={busy} data-testid="mail-guard-export"
          onClick={() => void act(async () => {
            saveTextFile("send-log.csv", "text/csv", await client!.exportMailSendLog());
          })}>
          <Download size={14} aria-hidden /> {t("settingsModal.mailGuard.export")}
        </Button>
        {note(problems.log, "mail-guard-log-error")}
        {problems.log ? null : log.length === 0 ? <p className={formStyles.hint}>{t("settingsModal.mailGuard.logEmpty")}</p> : (
          <table data-testid="mail-guard-log">
            <thead>
              <tr>
                <th>{t("settingsModal.mailGuard.colTime")}</th>
                <th>{t("settingsModal.mailGuard.colTo")}</th>
                <th>{t("settingsModal.mailGuard.colSubject")}</th>
                <th>{t("settingsModal.mailGuard.colBot")}</th>
                <th>{t("settingsModal.mailGuard.colNote")}</th>
              </tr>
            </thead>
            <tbody>
              {log.map((e, i) => (
                <tr key={`${e.at}-${e.message_id}-${i}`}>
                  <td>{formatDateTime(e.at * 1000, i18n.language)}</td>
                  <td>{e.recipients.join(", ")}</td>
                  <td>{e.subject}</td>
                  <td>{e.bot ?? ""}</td>
                  <td>{e.send_anyway.length > 0 ? t("settingsModal.mailGuard.sentAnyway") : ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </Card>
  );
}
