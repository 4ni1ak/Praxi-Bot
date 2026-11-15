import { Info, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { CliEngineDto, CliInstallApproval, DetectedProviderDto } from "../../api/types";
import { ApprovalCard, useCountdown } from "../../components/ChatCards";
import { BrandIcon } from "../../components/BrandIcon";
import { Switch } from "../../components/SettingsUi";
import { useToast } from "../../components/Toast";
import {
  configuredCliIds,
  detectedRowAction,
  signInCommand
} from "../../lib/detectedAccounts";
import { openExternal } from "../../lib/externalLink";
import { useDaemon } from "../../state/DaemonContext";
import { notifyProvidersChanged, useProviders } from "../../state/useProviders";
import styles from "./AiPage.module.css";

function brandOf(row: DetectedProviderDto): string {
  return row.id === "chatgpt-plan" ? "codex" : row.id;
}

/**
 * "Bu bilgisayarda bulunanlar": coding CLIs the daemon found
 * (`GET /v1/providers/cli`); runnable ones are turned on with one switch
 * (`POST /v1/providers/cli/enable`).  The daemon has no "disable" route, so a
 * switch that is on stays on here.
 */
export function DetectedAccountsBlock() {
  const { t } = useTranslation();
  const toast = useToast();
  const { client } = useDaemon();
  const { dto, reload: reloadProviders } = useProviders();
  const [rows, setRows] = useState<DetectedProviderDto[]>([]);
  const [engines, setEngines] = useState<CliEngineDto[]>([]);
  const [approval, setApproval] = useState<CliInstallApproval | null>(null);
  const approvalRemaining = useCountdown(approval?.expires_at_ms);
  const [installing, setInstalling] = useState(false);
  const [installOutput, setInstallOutput] = useState("");
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [enabling, setEnabling] = useState<string | null>(null);
  const [enableError, setEnableError] = useState<string | null>(null);
  const [showSignIn, setShowSignIn] = useState<string | null>(null);

  const enabledCli = useMemo(() => configuredCliIds(dto), [dto]);

  const loadDetected = useCallback(
    async (refresh: boolean) => {
      if (!client) {
        setRows([]);
        setLoadError(null);
        return;
      }
      setLoading(true);
      setLoadError(null);
      try {
        const found = await client.cliEngines(refresh);
        setEngines(found);
        setRows(found.map((engine) => engine.detected));
      } catch (e) {
        setLoadError(
          e instanceof Error && e.message
            ? e.message
            : t("settingsModal.ai.detected.detectError")
        );
      } finally {
        setLoading(false);
      }
    },
    [client, t]
  );

  useEffect(() => {
    void loadDetected(false);
  }, [loadDetected]);

  const previewInstall = async (id: string) => {
    if (!client) { return; }
    setEnableError(null);
    setInstallOutput("");
    try { setApproval(await client.previewCliInstall(id)); }
    catch (error) { setEnableError(String(error)); }
  };

  const decideInstall = async (decision: "approve" | "deny") => {
    if (!client || !approval) { return; }
    setInstalling(true);
    try {
      await client.decideCliInstall(approval.approval_id, decision, (text) => setInstallOutput((previous) => (previous + text).slice(-65536)));
      setApproval(null);
      if (decision === "approve") { toast.show(t("settingsModal.ai.detected.installDone"), "success"); }
    } catch (error) {
      setApproval(null);
      const code = error instanceof Error ? error.message : "install_failed";
      setEnableError(t(`settingsModal.ai.detected.installErrors.${code}`, {
        defaultValue: t("settingsModal.ai.detected.installErrors.install_failed")
      }));
    } finally { setInstalling(false); await loadDetected(true); }
  };

  const copyCommand = async (command: string) => {
    try { await navigator.clipboard.writeText(command); toast.show(t("settingsModal.ai.detected.copied"), "success"); }
    catch (error) { setEnableError(String(error)); }
  };

  const onEnable = async (row: DetectedProviderDto) => {
    if (!client) {
      return;
    }
    setEnabling(row.id);
    setEnableError(null);
    try {
      await client.enableCliProvider(row.id);
      await reloadProviders();
      notifyProvidersChanged();
      toast.show(
        t("settingsModal.ai.detected.enabledToast", { name: row.display_name }),
        "success"
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setEnableError(msg);
      toast.show(msg, "danger");
    } finally {
      setEnabling(null);
    }
  };

  const subtitle = (row: DetectedProviderDto, kind: string) => {
    const meta = [row.account_label, row.installed ? row.version : null]
      .filter(Boolean)
      .join(" · ");
    switch (kind) {
      case "enabled":
        return [meta, t("settingsModal.ai.detected.ready")].filter(Boolean).join(" · ");
      case "enable":
        // Never claim "signed in" when the daemon could not tell.
        return [
          meta,
          row.auth === "signed_in"
            ? t("settingsModal.ai.detected.authSignedIn")
            : t("settingsModal.ai.detected.authUnknown")
        ]
          .filter(Boolean)
          .join(" · ");
      case "sign_in_hint":
        return [meta, t("settingsModal.ai.detected.foundSignInNeeded")].filter(Boolean).join(" · ");
      case "not_installed":
        return t("settingsModal.ai.detected.notInstalled");
      case "codex_unconfinable":
        return [meta, t("settingsModal.ai.detected.codexUnconfinable")].filter(Boolean).join(" · ");
      default:
        return [meta, t("settingsModal.ai.detected.notRunnable")].filter(Boolean).join(" · ");
    }
  };

  return (
    <section className={styles.sec} data-testid="detected-accounts-block" aria-labelledby="ai-detected-h">
      <div className={styles.head}>
        <h3 id="ai-detected-h">{t("settingsModal.ai.detected.title")}</h3>
        <span className={styles.grow} />
        <button
          type="button"
          className={styles.textBtn}
          disabled={!client || loading}
          onClick={() => void loadDetected(true)}
        >
          <RefreshCw size={14} aria-hidden="true" className={loading ? styles.spin : undefined} />
          {t("settingsModal.ai.detected.refresh")}
        </button>
      </div>

      <div className={styles.card}>
        {!client ? (
          <p className={styles.errorRow}>{t("settingsModal.ai.detected.offline")}</p>
        ) : loadError ? (
          <div className={styles.errorRow} role="alert">
            <span>
              {t("settingsModal.ai.detected.detectError")} ({loadError})
            </span>
            <button type="button" className={styles.pill} disabled={loading} onClick={() => void loadDetected(true)}>
              {t("settingsModal.ai.detected.retry")}
            </button>
          </div>
        ) : (
          <ul className={styles.list}>
            {rows.map((row) => {
              const engine = engines.find((entry) => entry.detected.id === row.id);
              // A CLI the confined engine cannot run can still be enabled for bots that
              // use its own tools (chosen per bot in the Engine settings).
              const nativeOnly = Boolean(engine?.native) && row.installed && !row.runnable;
              const action = nativeOnly
                ? ({ kind: enabledCli.has(`cli:${row.id}`) ? "enabled" : "enable" } as const)
                : detectedRowAction(row, enabledCli);
              const install = engine?.install;
              return (
                <li key={row.id} className={styles.item} data-testid={`detected-${row.id}`}>
                  <BrandIcon name={brandOf(row)} bare size={22} />
                  <div className={styles.text}>
                    <span>{row.display_name}</span>
                    <span className={styles.statusChip}>
                      {t(engine?.outdated ? "settingsModal.ai.detected.outdated" : !row.installed ? "settingsModal.ai.detected.notInstalled" : row.auth === "signed_out" ? "settingsModal.ai.detected.foundSignInNeeded" : "settingsModal.ai.detected.installedVersion", { version: row.version ?? "?" })}
                    </span>
                    {engine?.path ? <span className={styles.cliPath}>{engine.path}</span> : null}
                    {engine?.limitation === "native_tools_not_confined" ? <span className={styles.subWarn}>{t("settingsModal.ai.detected.nativeToolsOpen")}</span> : null}
                    {engine?.native?.support === "experimental" ? <span className={styles.subWarn}>{t("settingsModal.ai.detected.experimental")}</span> : null}
                    {engine?.limitation === "native_only" ? <span className={styles.sub}>{t("settingsModal.ai.detected.nativeOnly")}</span> : null}
                    {engine?.limitation === "detection_only" ? <span className={styles.subWarn}>{t("settingsModal.ai.detected.detectionOnly")}</span> : null}
                    {engine?.limitation === "codex_unconfinable" ? <span className={styles.subWarn}>{t("settingsModal.ai.detected.codexBlocked")}</span> : null}
                    {row.runnable ? <span className={styles.sub}>{t("settingsModal.ai.detected.runnableEngine")}</span> : null}
                    {install ? <>
                      <code className={styles.cliCommand}>{install.command}</code>
                      <span className={styles.sub}>{t(install.verified ? "settingsModal.ai.detected.commandVerified" : "settingsModal.ai.detected.commandUnverified")}</span>
                    </> : null}
                    <span className={`${styles.sub} ${action.kind === "codex_unconfinable" ? styles.subWarn : ""}`}>
                      {subtitle(row, action.kind)}
                    </span>
                    {showSignIn === row.id ? (
                      <span className={styles.sub}>
                        {t("settingsModal.ai.detected.signInIn")}{" "}
                        <code className={styles.code}>{engine?.login ?? signInCommand(row.id)}</code>
                      </span>
                    ) : null}
                  </div>
                  <div className={styles.ctl}>
                    {action.kind === "enable" || action.kind === "enabled" ? (
                      <Switch
                        checked={action.kind === "enabled"}
                        disabled={action.kind === "enabled" || enabling === row.id}
                        label={t("settingsModal.ai.detected.toggle", { name: row.display_name })}
                        onChange={() => void onEnable(row)}
                      />
                    ) : null}
                    {install?.executable ? (
                      <button type="button" className={styles.pill} disabled={installing || approval !== null} onClick={() => void previewInstall(row.id)}>
                        {t("settingsModal.ai.detected.install")}
                      </button>
                    ) : null}
                    {install ? <button type="button" className={styles.textBtn} onClick={() => void copyCommand(install.command)}>{t("settingsModal.ai.detected.copyCommand")}</button> : null}
                    <button type="button" className={styles.textBtn} aria-expanded={showSignIn === row.id} onClick={() => setShowSignIn((current) => current === row.id ? null : row.id)}>{t("settingsModal.ai.detected.howSignIn")}</button>
                    {engine?.docs_url ? <button type="button" className={styles.textBtn} onClick={() => void openExternal(engine.docs_url)}>{t("settingsModal.ai.detected.docs")}</button> : null}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {approval ? <div className={styles.cliApproval}>
        <pre className={styles.cliCommand}>{approval.command}</pre>
        <p className={styles.hint}>{t("settingsModal.ai.detected.installApproval")}</p>
        <ApprovalCard key={approval.approval_id} expiresAtMs={approval.expires_at_ms} tool={{
          kind: "tool", key: approval.approval_id, callId: approval.approval_id,
          name: "cli_install", input: { command: approval.command },
          status: "awaiting_approval", requestedMs: Date.now(), gated: true,
          summary: t("settingsModal.ai.detected.installSummary", { engine: approval.engine_id }),
          details: { command: approval.command }
        }} onDecide={decideInstall} />
        {approvalRemaining === 0 && !installing ? <button type="button" className={styles.textBtn} onClick={() => setApproval(null)}>{t("settingsModal.ai.detected.dismissExpired")}</button> : null}
      </div> : null}
      {installing ? <p role="status" className={styles.hint}>{t("settingsModal.ai.detected.installRunning")}</p> : null}
      {installOutput ? <pre className={styles.cliOutput} role="log" aria-label={t("settingsModal.ai.detected.installOutput")}>{installOutput}</pre> : null}
      <p className={styles.hint}>
        <Info size={12} aria-hidden="true" /> {t("settingsModal.ai.detected.description")}{" "}
        {t("settingsModal.ai.detected.termsNotice")}
      </p>
      {enableError ? (
        <p className={styles.error} role="alert">
          {enableError}
        </p>
      ) : null}
    </section>
  );
}
