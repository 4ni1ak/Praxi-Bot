import { Check, ChevronDown, Search, Server, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { BotAvatar, Button } from "ui";
import { BrandIcon } from "../components/BrandIcon";
import { brandIcon } from "../lib/brandIcons";
import { botView } from "../lib/botProfile";
import type { McpAuthState, McpCatalogEntry, McpServer } from "../api/types";
import formStyles from "../components/Form.module.css";
import { BotAccessPicker } from "../components/BotAccessPicker";
import { Modal } from "../components/Modal";
import { Segmented } from "../components/SettingsUi";
import { useToast } from "../components/Toast";
import { openExternal } from "../lib/externalLink";
import { mcpErrorHintKey } from "../lib/mcpError";
import {
  ALL_CATEGORIES,
  type BotScope,
  botAccessLabels,
  botScope,
  botSelection,
  saveServerScope,
  connectCatalogEntry,
  customServerConfig,
  filterCatalog,
  filterServers,
  isConnected,
  MissingFolderError,
  McpBrowserOpenError,
  MissingSecretError,
  signInMcpServer,
  statusTone,
  withFolder
} from "../lib/mcpApps";
import { useDaemon } from "../state/DaemonContext";
import { useMcp } from "../state/useMcp";
import styles from "./AppsModal.module.css";

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Most bot faces shown next to an app; the rest collapse into "+n". */
const MAX_FACES = 4;

/**
 * A connected app: bare logo, name, the bots that use it, state.  "Manage"
 * (on hover / focus, always while open or broken) opens bot access,
 * reconnect and disconnect below the row.  An HTTP server that waits for
 * OAuth offers "Sign in" (opens the browser); a signed-in one "Sign out".
 */

/** The failure of one server: what to do first, then the full message and what the server printed. */
function McpErrorBlock({
  server,
  busy,
  onChooseFolder
}: {
  server: McpServer;
  busy: boolean;
  onChooseFolder: (folder: string) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [folder, setFolder] = useState("");
  const hintKey = mcpErrorHintKey(server.error_kind, server.command);
  const stderr = server.stderr_tail ?? [];
  const id = `mcp-error-${server.name}`;
  return (
    <div className={styles.errorBlock} data-testid={`mcp-error-${server.name}`}>
      {hintKey ? <p className={styles.errorHint}>{t(hintKey)}</p> : null}
      {server.error_kind === "needs_folder" ? (
        <form
          className={styles.folderForm}
          onSubmit={(e) => {
            e.preventDefault();
            if (folder.trim()) {
              onChooseFolder(folder.trim());
            }
          }}
        >
          <label className={formStyles.label} htmlFor={`mcp-folder-${server.name}`}>
            {t("apps.folder.label")}
          </label>
          <input
            id={`mcp-folder-${server.name}`}
            className={formStyles.input}
            value={folder}
            placeholder={t("apps.folder.placeholder")}
            spellCheck={false}
            data-testid={`mcp-folder-input-${server.name}`}
            onChange={(e) => setFolder(e.target.value)}
          />
          <Button type="submit" size="sm" variant="primary" disabled={busy || !folder.trim()}>
            {t("apps.folder.save")}
          </Button>
        </form>
      ) : null}
      <p className={styles.errorText}>{server.error}</p>
      <button
        type="button"
        className={styles.errorToggle}
        aria-expanded={open}
        aria-controls={id}
        data-testid={`mcp-error-toggle-${server.name}`}
        onClick={() => setOpen((o) => !o)}
      >
        {open ? t("apps.error.hideDetails") : t("apps.error.showDetails")}
      </button>
      {open ? (
        <div className={styles.errorDetails} id={id}>
          <span className={styles.errorLabel}>{t("apps.error.message")}</span>
          <pre className={styles.errorPre}>{server.error}</pre>
          <span className={styles.errorLabel}>{t("apps.error.stderr")}</span>
          <pre className={styles.errorPre}>
            {stderr.length > 0 ? stderr.join("\n") : t("apps.error.noStderr")}
          </pre>
        </div>
      ) : null}
    </div>
  );
}

function ConnectedCell({
  server,
  description,
  busy,
  signingIn,
  onReconnect,
  onSignIn,
  onCancelSignIn,
  onPasteRedirect,
  onSignOut,
  onDisconnect,
  onSaveBots,
  onChooseFolder
}: {
  server: McpServer;
  description?: string;
  busy: boolean;
  onChooseFolder: (folder: string) => void;
  signingIn: boolean;
  onReconnect: () => void;
  onSignIn: () => void;
  onCancelSignIn: () => void;
  onPasteRedirect: (url: string) => void;
  onSignOut: () => void;
  onDisconnect: () => void;
  onSaveBots: (from: BotScope, to: BotScope) => void;
}) {
  const { t } = useTranslation();
  const { bots, client } = useDaemon();
  const [confirm, setConfirm] = useState(false);
  const [open, setOpen] = useState(false);
  const [auth, setAuth] = useState<McpAuthState | null>(null);
  const [pasted, setPasted] = useState("");
  const isHttp = server.transport === "http";
  const needsSignIn = server.status === "auth_required";
  // Ask the daemon (never the server) whether a sign-in is stored, so
  // header-authenticated servers do not get a meaningless "Sign out".
  useEffect(() => {
    if (!open || !isHttp || !client) {
      return;
    }
    let live = true;
    client
      .mcpAuthStatus(server.name)
      .then((state) => {
        if (live) setAuth(state);
      })
      .catch(() => {
        if (live) setAuth(null);
      });
    return () => {
      live = false;
    };
  }, [open, isHttp, client, server.name, server.status, signingIn]);
  const [scope, setScope] = useState<BotScope>(() => botScope(server));
  // The scope the editor opened with: the saved diff is against this, not
  // against whatever the live server says at click time.
  const [baseline, setBaseline] = useState<BotScope>(() => botScope(server));
  const access = botAccessLabels(server, bots, t("apps.access.deletedBot"));
  const tone = statusTone(server.status);
  const needsReconnect =
    needsSignIn ||
    server.status === "error" ||
    server.status === "disconnected" ||
    server.status === "quarantined";
  const users = bots.filter((b) => access.all || server.bots?.includes(b.id));
  const usedBy = access.all
    ? t("apps.botsAll")
    : access.names.length === 0
      ? t("apps.botsNone")
      : t("apps.botsSome", { names: access.names.join(", ") });
  const known = brandIcon(server.name);
  const panelId = `mcp-manage-${server.name}`;
  return (
    <li
      className={`${styles.cell} ${open ? styles.cellOpen : ""}`}
      data-testid={`mcp-server-${server.name}`}
    >
      <div className={styles.cellRow}>
        <BrandIcon name={server.name} bare size={26} />
        <div className={styles.cellText}>
          <b className={styles.cellName}>
            <span className={styles.cellTitle}>{known.known ? known.name : server.name}</span>
            <span className={styles.faces} title={usedBy}>
              {users.slice(0, MAX_FACES).map((b) => (
                <BotAvatar key={b.id} bot={botView(b)} size="xs" decorative />
              ))}
              {users.length > MAX_FACES ? (
                <span className={styles.more}>+{users.length - MAX_FACES}</span>
              ) : null}
            </span>
          </b>
          <small className={styles.cellSub}>
            {description ?? t("apps.toolCount", { count: server.tool_count })}
            {" · "}
            {usedBy}
          </small>
        </div>
        <span className={`${styles.state} ${styles[`tone_${tone}`]}`}>
          {tone === "ok" ? <Check aria-hidden="true" /> : <span className={styles.dot} aria-hidden="true" />}
          {t(`apps.status.${server.status}`)}
        </span>
        <button
          type="button"
          className={`${styles.manage} ${needsReconnect ? styles.manageShown : ""}`}
          aria-expanded={open}
          aria-controls={panelId}
          data-testid={`mcp-manage-${server.name}`}
          onClick={() => {
            if (!open) {
              setBaseline(botScope(server));
              setScope(botSelection(botScope(server), bots));
            }
            setConfirm(false);
            setOpen((o) => !o);
          }}
        >
          {t("apps.manage")}
          <ChevronDown aria-hidden="true" />
        </button>
      </div>
      {server.error ? (
        <McpErrorBlock server={server} busy={busy} onChooseFolder={onChooseFolder} />
      ) : null}
      {open ? (
        <div className={styles.panel} id={panelId}>
          <span className={styles.panelLabel}>{t("apps.access.label")}</span>
          <BotAccessPicker scope={scope} bots={bots} onChange={setScope} />
          <div className={styles.actions}>
            <Button
              size="sm"
              variant="primary"
              disabled={busy}
              data-testid={`mcp-access-save-${server.name}`}
              onClick={() => {
                setOpen(false);
                onSaveBots(baseline, scope);
              }}
            >
              {t("apps.access.save")}
            </Button>
            {needsSignIn && !signingIn ? (
              <Button
                size="sm"
                variant="secondary"
                disabled={busy}
                data-testid={`mcp-sign-in-${server.name}`}
                onClick={onSignIn}
              >
                {t("apps.auth.signIn")}
              </Button>
            ) : null}
            {signingIn ? (
              <>
                <span className={styles.panelLabel} role="status">
                  {t("apps.auth.waiting")}
                </span>
                <Button size="sm" variant="ghost" onClick={onCancelSignIn}>
                  {t("common.cancel")}
                </Button>
              </>
            ) : null}
          </div>
          {signingIn ? (
            <form
              className={styles.actions}
              onSubmit={(e) => {
                e.preventDefault();
                if (pasted.trim()) {
                  onPasteRedirect(pasted.trim());
                  setPasted("");
                }
              }}
            >
              <input
                className={formStyles.input}
                value={pasted}
                onChange={(e) => setPasted(e.target.value)}
                placeholder="http://127.0.0.1:…/callback?…"
                aria-label={t("apps.auth.pasteLabel")}
                data-testid={`mcp-paste-${server.name}`}
              />
              <Button size="sm" variant="secondary" type="submit" disabled={!pasted.trim()}>
                {t("apps.auth.pasteSubmit")}
              </Button>
            </form>
          ) : null}
          {signingIn ? <p className={formStyles.hint}>{t("apps.auth.pasteHint")}</p> : null}
          <div className={styles.actions}>
            {needsReconnect && !needsSignIn ? (
              <Button size="sm" variant="secondary" disabled={busy} onClick={onReconnect}>
                {t("apps.reconnect")}
              </Button>
            ) : null}
            {isHttp && auth === "signed_in" && !signingIn ? (
              <Button
                size="sm"
                variant="ghost"
                disabled={busy}
                data-testid={`mcp-sign-out-${server.name}`}
                onClick={onSignOut}
              >
                {t("apps.auth.signOut")}
              </Button>
            ) : null}
            <span className={styles.grow} />
            {confirm ? (
              <>
                <Button
                  size="sm"
                  variant="danger"
                  disabled={busy}
                  onClick={() => {
                    setConfirm(false);
                    setOpen(false);
                    onDisconnect();
                  }}
                >
                  {t("apps.disconnectConfirm")}
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setConfirm(false)}>
                  {t("common.cancel")}
                </Button>
              </>
            ) : (
              <Button size="sm" variant="ghost" disabled={busy} onClick={() => setConfirm(true)}>
                {t("apps.disconnect")}
              </Button>
            )}
          </div>
        </div>
      ) : null}
    </li>
  );
}

/** A catalog app: one click connects; apps that need a key ask for it right under the row. */
function DiscoverCell({
  entry,
  onConnect
}: {
  entry: McpCatalogEntry;
  onConnect: (values: Record<string, string>, folder?: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const secrets = entry.install.required_secrets;
  const wantsFolder = Boolean(entry.install.folder);
  const [folder, setFolder] = useState(entry.install.folder?.suggestion ?? "");
  const [asking, setAsking] = useState(false);
  const [values, setValues] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  const submit = async (v: Record<string, string>) => {
    setBusy(true);
    try {
      await onConnect(v, wantsFolder ? folder : undefined);
      setAsking(false);
      setValues({});
    } finally {
      setBusy(false);
    }
  };

  const complete =
    secrets.every((s) => (values[s.name] ?? "").trim() !== "") &&
    (!wantsFolder || folder.trim() !== "");

  return (
    <li
      className={`${styles.cell} ${asking ? styles.cellOpen : ""}`}
      data-testid={`mcp-catalog-${entry.id}`}
    >
      <div className={styles.cellRow}>
        <BrandIcon name={entry.name} bare size={26} />
        <div className={styles.cellText}>
          <b className={styles.cellName}>
            <span className={styles.cellTitle}>{entry.name}</span>
          </b>
          <small className={styles.cellSub} title={entry.description}>{entry.description}</small>
        </div>
        <button
          type="button"
          className={styles.connect}
          disabled={busy || asking}
          aria-label={t("apps.connectApp", { name: entry.name })}
          onClick={() => {
            if (secrets.length === 0 && !wantsFolder) {
              void submit({});
            } else {
              setAsking(true);
            }
          }}
        >
          {t("apps.connect")}
        </button>
      </div>
      {asking ? (
        <form
          className={styles.panel}
          onSubmit={(e) => {
            e.preventDefault();
            if (complete) {
              void submit(values);
            }
          }}
        >
          {wantsFolder ? (
            <div className={formStyles.field}>
              <label className={formStyles.label} htmlFor={`${entry.id}-folder`}>
                {t("apps.folder.label")}
              </label>
              <input
                id={`${entry.id}-folder`}
                className={formStyles.input}
                autoFocus
                spellCheck={false}
                value={folder}
                placeholder={t("apps.folder.placeholder")}
                data-testid={`mcp-catalog-folder-${entry.id}`}
                onChange={(e) => setFolder(e.target.value)}
              />
              <p className={formStyles.hint}>{t("apps.folder.hint")}</p>
            </div>
          ) : null}
          {secrets.map((s, i) => (
            <div key={s.name} className={formStyles.field}>
              <label className={formStyles.label} htmlFor={`${entry.id}-${s.name}`}>
                {s.label}
              </label>
              <input
                id={`${entry.id}-${s.name}`}
                className={formStyles.input}
                type="password"
                autoComplete="off"
                autoFocus={i === 0}
                value={values[s.name] ?? ""}
                onChange={(e) =>
                  setValues((v) => ({ ...v, [s.name]: e.target.value }))
                }
              />
              {s.help_url ? (
                <button
                  type="button"
                  className={styles.link}
                  onClick={() => void openExternal(s.help_url ?? "")}
                >
                  {t("apps.getKey")}
                </button>
              ) : null}
            </div>
          ))}
          {secrets.length > 0 ? (
            <p className={formStyles.hint}>{t("apps.secretHint")}</p>
          ) : null}
          <div className={styles.actions}>
            <Button type="submit" size="sm" variant="primary" disabled={!complete || busy}>
              {t("apps.connectNow")}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setAsking(false);
                setValues({});
              }}
            >
              {t("common.cancel")}
            </Button>
          </div>
        </form>
      ) : null}
    </li>
  );
}

function CustomServerForm({
  existing,
  onSubmit,
  onCancel
}: {
  existing: string[];
  onSubmit: (cfg: ReturnType<typeof customServerConfig>) => Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = useState({
    name: "",
    transport: "stdio" as "stdio" | "http",
    command: "",
    url: "",
    env: ""
  });
  const [error, setError] = useState<"name" | "command" | "url" | null>(null);
  const [busy, setBusy] = useState(false);
  const { bots } = useDaemon();
  const [scope, setScope] = useState<BotScope>({ mode: "all" });
  const set = (patch: Partial<typeof form>) => setForm((f) => ({ ...f, ...patch }));

  return (
    <form
      className={styles.custom}
      onSubmit={(e) => {
        e.preventDefault();
        const result = customServerConfig(form, existing, botSelection(scope, bots));
        setError(result.ok ? null : result.error);
        if (result.ok) {
          setBusy(true);
          void onSubmit(result).finally(() => setBusy(false));
        }
      }}
    >
      <p className={formStyles.hint}>{t("apps.custom.intro")}</p>
      <div className={formStyles.field}>
        <label className={formStyles.label} htmlFor="mcp-name">
          {t("apps.custom.name")}
        </label>
        <input
          id="mcp-name"
          className={formStyles.input}
          value={form.name}
          placeholder="my-server"
          onChange={(e) => set({ name: e.target.value })}
        />
        {error === "name" ? (
          <p className={formStyles.error}>{t("apps.custom.errName")}</p>
        ) : null}
      </div>
      <Segmented
        label={t("apps.custom.transport")}
        value={form.transport}
        onChange={(transport) => set({ transport })}
        options={[
          { value: "stdio", label: t("apps.custom.stdio") },
          { value: "http", label: t("apps.custom.http") }
        ]}
      />
      {form.transport === "stdio" ? (
        <>
          <div className={formStyles.field}>
            <label className={formStyles.label} htmlFor="mcp-cmd">
              {t("apps.custom.command")}
            </label>
            <input
              id="mcp-cmd"
              className={formStyles.input}
              value={form.command}
              placeholder="npx -y @scope/server"
              onChange={(e) => set({ command: e.target.value })}
            />
            {error === "command" ? (
              <p className={formStyles.error}>{t("apps.custom.errCommand")}</p>
            ) : null}
          </div>
          <div className={formStyles.field}>
            <label className={formStyles.label} htmlFor="mcp-env">
              {t("apps.custom.env")}
            </label>
            <textarea
              id="mcp-env"
              className={formStyles.textarea}
              value={form.env}
              placeholder="KEY=value"
              onChange={(e) => set({ env: e.target.value })}
            />
            <p className={formStyles.hint}>{t("apps.custom.envHint")}</p>
          </div>
        </>
      ) : (
        <div className={formStyles.field}>
          <label className={formStyles.label} htmlFor="mcp-url">
            {t("apps.custom.url")}
          </label>
          <input
            id="mcp-url"
            className={formStyles.input}
            value={form.url}
            placeholder="https://"
            onChange={(e) => set({ url: e.target.value })}
          />
          {error === "url" ? (
            <p className={formStyles.error}>{t("apps.custom.errUrl")}</p>
          ) : null}
        </div>
      )}
      <div className={formStyles.field}>
        <span className={formStyles.label}>{t("apps.access.label")}</span>
        <BotAccessPicker scope={scope} bots={bots} onChange={setScope} />
      </div>
      <div className={styles.formActions}>
        <Button type="submit" variant="primary" size="sm" disabled={busy}>
          {t("apps.custom.add")}
        </Button>
        <Button variant="ghost" size="sm" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
      </div>
    </form>
  );
}

/**
 * "Uygulamalar": search, Connected and Discover.  Apps connect in one click;
 * "Your own MCP server" adds a custom stdio / http server.
 */
export function AppsModal({
  open,
  onClose
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const { client } = useDaemon();
  const { servers, catalog, loading, error, reload } = useMcp(open);
  const [query, setQuery] = useState("");
  const [advanced, setAdvanced] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  // Pending sign-ins by server; the flag stops the poll and cancels the flow.
  const [signing, setSigning] = useState<Record<string, { aborted: boolean }>>({});

  const signIn = async (name: string) => {
    if (!client || signing[name]) {
      return;
    }
    const signal = { aborted: false };
    setSigning((m) => ({ ...m, [name]: signal }));
    try {
      const state = await signInMcpServer(client, name, openExternal, { signal });
      if (state === "signed_in") {
        toast.show(t("apps.auth.signedInToast", { name }), "success");
      } else if (!signal.aborted) {
        toast.show(t("apps.auth.failedToast", { name }), "danger");
      }
    } catch (e) {
      toast.show(
        e instanceof McpBrowserOpenError ? t("apps.auth.openFailed", { name }) : errorText(e),
        "danger"
      );
    } finally {
      setSigning((m) => {
        const next = { ...m };
        delete next[name];
        return next;
      });
      await reload();
    }
  };

  useEffect(() => {
    if (open) {
      setQuery("");
      setAdvanced(false);
    }
  }, [open]);

  const shownServers = useMemo(() => filterServers(servers, query), [servers, query]);
  const shownCatalog = useMemo(
    () =>
      filterCatalog(catalog, query, ALL_CATEGORIES).filter(
        (entry) => !isConnected(entry, servers)
      ),
    [catalog, query, servers]
  );
  const describe = (name: string) => catalog.find((c) => c.id === name)?.description;

  const run = async (key: string, fn: () => Promise<void>) => {
    setBusy(key);
    try {
      await fn();
    } catch (e) {
      toast.show(errorText(e), "danger");
    } finally {
      setBusy(null);
    }
  };

  const connect = async (
    entry: McpCatalogEntry,
    values: Record<string, string>,
    folder?: string
  ) => {
    if (!client) {
      return;
    }
    try {
      await connectCatalogEntry(client, entry, values, folder);
      toast.show(t("apps.connectedToast", { name: entry.name }), "success");
      await reload();
    } catch (e) {
      toast.show(
        e instanceof MissingSecretError
          ? t("apps.missingSecret")
          : e instanceof MissingFolderError
            ? t("apps.folder.missing")
            : errorText(e),
        "danger"
      );
      throw e;
    }
  };

  return (
    <Modal open={open} onClose={onClose} label={t("apps.title")} size="apps">
      <div className={styles.root}>
        <header className={styles.top}>
          <div className={styles.find}>
            <Search aria-hidden="true" />
            <input
              type="search"
              data-autofocus
              value={query}
              placeholder={t("apps.search")}
              aria-label={t("apps.search")}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          <button
            type="button"
            className={`${styles.pill} ${advanced ? styles.pillOn : ""}`}
            aria-pressed={advanced}
            data-testid="apps-advanced"
            onClick={() => setAdvanced((a) => !a)}
          >
            <Server aria-hidden="true" />
            {t("apps.mcpServers")}
          </button>
          <button
            type="button"
            className={styles.close}
            title={t("common.close")}
            aria-label={t("common.close")}
            onClick={onClose}
          >
            <X />
          </button>
        </header>

        <div className={styles.body}>
          {error ? (
            <p className={formStyles.error} role="alert">
              {error}
            </p>
          ) : null}

          {advanced ? (
            <CustomServerForm
              existing={servers.map((s) => s.name)}
              onCancel={() => setAdvanced(false)}
              onSubmit={async (result) => {
                if (!client || !result.ok) {
                  return;
                }
                await run("custom", async () => {
                  await client.createMcpServer(result.config);
                  toast.show(
                    t("apps.connectedToast", { name: result.config.name }),
                    "success"
                  );
                  await reload();
                  setAdvanced(false);
                });
              }}
            />
          ) : (
            <>
              {shownServers.length > 0 ? (
                <section className={styles.section} aria-labelledby="apps-connected-h">
                  <h3 className={styles.sectionLabel} id="apps-connected-h">{t("apps.connectedSection")}</h3>
                  <ul className={styles.grid}>
                    {shownServers.map((s) => (
                      <ConnectedCell
                        key={s.name}
                        server={s}
                        description={describe(s.name)}
                        busy={busy === s.name}
                        onChooseFolder={(folder) =>
                          void run(s.name, async () => {
                            await client?.updateMcpServer(s.name, {
                              args: withFolder(s.args ?? [], folder)
                            });
                            await reload();
                          })
                        }
                        signingIn={Boolean(signing[s.name])}
                        onSignIn={() => void signIn(s.name)}
                        onCancelSignIn={() => {
                          const signal = signing[s.name];
                          if (signal) {
                            signal.aborted = true;
                          }
                        }}
                        onPasteRedirect={(url) =>
                          void run(s.name, async () => {
                            // The poll in signIn() sees the finished flow.
                            await client?.completeMcpAuth(s.name, url);
                          })
                        }
                        onSignOut={() =>
                          void run(s.name, async () => {
                            await client?.logoutMcpAuth(s.name);
                            toast.show(t("apps.auth.signedOutToast", { name: s.name }), "info");
                            await reload();
                          })
                        }
                        onReconnect={() =>
                          void run(s.name, async () => {
                            await client?.reconnectMcpServer(s.name);
                            await reload();
                          })
                        }
                        onSaveBots={(from, to) =>
                          void run(s.name, async () => {
                            if (!client) {
                              return;
                            }
                            const { applied } = await saveServerScope(client, s.name, from, to);
                            if (applied) {
                              toast.show(t("apps.access.savedToast", { name: s.name }), "success");
                            } else {
                              // The server is not in the state the user chose
                              // (changed elsewhere meanwhile): say so, show the truth.
                              toast.show(t("apps.access.notApplied", { name: s.name }), "danger");
                            }
                            await reload();
                          })
                        }
                        onDisconnect={() =>
                          void run(s.name, async () => {
                            await client?.deleteMcpServer(s.name);
                            toast.show(t("apps.disconnectedToast", { name: s.name }), "info");
                            await reload();
                          })
                        }
                      />
                    ))}
                  </ul>
                </section>
              ) : null}

              <section className={styles.section} aria-labelledby="apps-discover-h">
                <h3 className={styles.sectionLabel} id="apps-discover-h">{t("apps.catalogSection")}</h3>
                {shownCatalog.length === 0 ? (
                  <p className={styles.empty}><Search aria-hidden="true" />
                    {loading ? t("apps.loading") : t("apps.noResults")}
                  </p>
                ) : (
                  <ul className={styles.grid}>
                    {shownCatalog.map((entry) => (
                      <DiscoverCell
                        key={entry.id}
                        entry={entry}
                        onConnect={(values, folder) => connect(entry, values, folder)}
                      />
                    ))}
                  </ul>
                )}
              </section>
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}
