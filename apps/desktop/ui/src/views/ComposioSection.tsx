import { Check, ExternalLink, KeyRound, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import type { ComposioApp, ComposioToolkit } from "../api/types";
import { BrandIcon } from "../components/BrandIcon";
import formStyles from "../components/Form.module.css";
import { useToast } from "../components/Toast";
import { upsertApp, withoutConnected } from "../lib/composioApps";
import { openComposioLink } from "../lib/externalLink";
import { useDaemon } from "../state/DaemonContext";
import { useComposio } from "../state/useComposio";
import styles from "./AppsModal.module.css";
import own from "./ComposioSection.module.css";

/** Data notice shown at connect; mail, documents and chat are named explicitly. */
const SENSITIVE = new Set(["gmail", "googledrive", "outlook", "googledocs", "googlesheets", "slack", "whatsapp", "telegram", "discord"]);

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function tone(state: ComposioApp["state"]): "ok" | "busy" | "bad" | "muted" {
  return state === "active" ? "ok" : state === "pending" ? "busy" : state === "expired" ? "bad" : "muted";
}

/** One catalog tile: Connect opens a short notice first, then the consent page. */
function ToolkitTile({
  toolkit,
  busy,
  onConnect
}: {
  toolkit: ComposioToolkit;
  busy: boolean;
  onConnect: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const [asking, setAsking] = useState(false);
  const sensitive = SENSITIVE.has(toolkit.slug);
  return (
    <li
      className={`${styles.cell} ${asking ? styles.cellOpen : ""}`}
      data-testid={`composio-tile-${toolkit.slug}`}
    >
      <div className={styles.cellRow}>
        <BrandIcon name={toolkit.name} bare size={26} />
        <div className={styles.cellText}>
          <b className={styles.cellName}>
            <span className={styles.cellTitle}>{toolkit.name}</span>
          </b>
          <small className={styles.cellSub} title={toolkit.description}>
            {toolkit.native_first
              ? t("apps.composio.nativeNote")
              : toolkit.description || toolkit.categories.join(", ")}
          </small>
        </div>
        <button
          type="button"
          className={styles.connect}
          disabled={busy || asking}
          aria-label={t("apps.connectApp", { name: toolkit.name })}
          onClick={() => setAsking(true)}
        >
          {t("apps.connect")}
        </button>
      </div>
      {asking ? (
        <div className={styles.panel}>
          <p className={own.notice}>
            <ShieldCheck aria-hidden="true" />
            {sensitive ? t("apps.composio.dataNoticeSensitive") : t("apps.composio.dataNotice")}
          </p>
          <div className={styles.actions}>
            <Button
              size="sm"
              variant="primary"
              disabled={busy}
              data-testid={`composio-continue-${toolkit.slug}`}
              onClick={() => {
                setAsking(false);
                void onConnect();
              }}
            >
              {t("apps.composio.continue")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setAsking(false)}>
              {t("common.cancel")}
            </Button>
          </div>
        </div>
      ) : null}
    </li>
  );
}

/** A connected (or connecting) Composio app. */
function AppCell({
  app,
  busy,
  onEnabled,
  onReconnect,
  onDisconnect
}: {
  app: ComposioApp;
  busy: boolean;
  onEnabled: (enabled: boolean) => void;
  onReconnect: () => void;
  onDisconnect: () => void;
}) {
  const { t } = useTranslation();
  const [confirm, setConfirm] = useState(false);
  const k = tone(app.state);
  return (
    <li className={styles.cell} data-testid={`composio-app-${app.slug}`}>
      <div className={styles.cellRow}>
        <BrandIcon name={app.name} bare size={26} />
        <div className={styles.cellText}>
          <b className={styles.cellName}>
            <span className={styles.cellTitle}>{app.name}</span>
            <span className={own.via}>{t("apps.composio.via")}</span>
          </b>
          <small className={styles.cellSub}>
            {app.state === "pending"
              ? t("apps.composio.waiting")
              : t("apps.toolCount", { count: app.tools })}
          </small>
        </div>
        <span className={`${styles.state} ${styles[`tone_${k}`]}`}>
          {k === "ok" ? <Check aria-hidden="true" /> : <span className={styles.dot} aria-hidden="true" />}
          {t(`apps.composio.state.${app.state}`)}
        </span>
      </div>
      <div className={own.cellActions}>
        {app.state === "active" ? (
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            aria-pressed={app.enabled}
            data-testid={`composio-enabled-${app.slug}`}
            onClick={() => onEnabled(!app.enabled)}
          >
            {app.enabled ? t("apps.composio.disable") : t("apps.composio.enable")}
          </Button>
        ) : null}
        {app.state !== "active" ? (
          <Button size="sm" variant="secondary" disabled={busy} onClick={onReconnect}>
            {t("apps.reconnect")}
          </Button>
        ) : null}
        <span className={styles.grow} />
        {confirm ? (
          <>
            <Button
              size="sm"
              variant="danger"
              disabled={busy}
              data-testid={`composio-disconnect-confirm-${app.slug}`}
              onClick={() => {
                setConfirm(false);
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
          <Button
            size="sm"
            variant="ghost"
            disabled={busy}
            data-testid={`composio-disconnect-${app.slug}`}
            onClick={() => setConfirm(true)}
          >
            {t("apps.disconnect")}
          </Button>
        )}
      </div>
    </li>
  );
}

/** One-time key setup: a deep link to the key page, one paste field, validated at once. */
function KeySetup({ keyUrl, onSaved }: { keyUrl: string; onSaved: () => Promise<void> }) {
  const { t } = useTranslation();
  const { client } = useDaemon();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return (
    <form
      className={own.setup}
      data-testid="composio-setup"
      onSubmit={(e) => {
        e.preventDefault();
        if (!client || value.trim() === "") {
          return;
        }
        setBusy(true);
        setError(null);
        void client
          .setComposioKey(value.trim())
          .then(async () => {
            // The key leaves this component's state at once; it is never shown again.
            setValue("");
            await onSaved();
          })
          .catch((err: unknown) => setError(errorText(err)))
          .finally(() => setBusy(false));
      }}
    >
      <div className={own.setupHead}>
        <KeyRound aria-hidden="true" />
        <div>
          <b>{t("apps.composio.setupTitle")}</b>
          <p className={formStyles.hint}>{t("apps.composio.setupBody")}</p>
        </div>
      </div>
      <button
        type="button"
        className={styles.link}
        data-testid="composio-get-key"
        onClick={() => { void openComposioLink(keyUrl, true).then((ok) => { if (!ok) setError(t("apps.composio.openFailed")); }); }}
      >
        <ExternalLink aria-hidden="true" /> {t("apps.composio.getKey")}
      </button>
      <div className={formStyles.field}>
        <label className={formStyles.label} htmlFor="composio-key">
          {t("apps.composio.keyLabel")}
        </label>
        <input
          id="composio-key"
          className={formStyles.input}
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={value}
          onChange={(e) => setValue(e.target.value)}
        />
        <p className={formStyles.hint}>{t("apps.composio.keyHint")}</p>
        {error ? (
          <p className={formStyles.error} role="alert">
            {error}
          </p>
        ) : null}
      </div>
      <p className={own.notice}>
        <ShieldCheck aria-hidden="true" />
        {t("apps.composio.dataNotice")}
      </p>
      <div className={styles.actions}>
        <Button type="submit" size="sm" variant="primary" disabled={busy || value.trim() === ""}>
          {t("apps.composio.keySave")}
        </Button>
      </div>
    </form>
  );
}

/**
 * "via Composio": the connected apps and Composio's whole catalog (searchable,
 * paged), behind a one-time key setup.  Nothing here knows a per-service token.
 */
export function ComposioSection({ active, query }: { active: boolean; query: string }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { client } = useDaemon();
  const c = useComposio(active, query);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmKey, setConfirmKey] = useState(false);
  const [pendingLink, setPendingLink] = useState<string | null>(null);

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

  const connect = (slug: string, name: string) =>
    run(slug, async () => {
      if (!client) {
        return;
      }
      const link = await client.composioConnect(slug);
      setPendingLink(link.redirect_url);
      if (!(await openComposioLink(link.redirect_url))) {
        toast.show(t("apps.composio.openFailed"), "danger");
        return;
      }
      setPendingLink(null);
      c.setApps((prev) =>
        upsertApp(prev, {
          slug,
          name,
          state: "pending",
          enabled: true,
          native_first: false,
          bots: null,
          tools: 0
        })
      );
    });

  if (!c.status) {
    return null;
  }
  if (!c.status.configured) {
    return (
      <section className={styles.section} aria-labelledby="composio-h">
        <h3 className={styles.sectionLabel} id="composio-h">
          {t("apps.composio.title")}
        </h3>
        <KeySetup keyUrl={c.status.key_setup_url} onSaved={async () => {
            await c.reloadStatus();
            await c.reloadApps();
          }} />
      </section>
    );
  }

  const q = query.trim();
  const popular = q === "" ? withoutConnected(c.popular, c.apps) : [];
  const popularSlugs = new Set(popular.map((p) => p.slug));
  const rest = withoutConnected(c.items, c.apps).filter((i) => !popularSlugs.has(i.slug));

  return (
    <>
      {pendingLink ? <Button size="sm" variant="secondary" onClick={() => {
          void openComposioLink(pendingLink).then((ok) => {
            if (ok) { setPendingLink(null); void c.reloadApps(); }
            else toast.show(t("apps.composio.openFailed"), "danger");
          });
        }}>{t("apps.composio.retryOpen")}</Button> : null}
      {c.apps.length > 0 ? (
        <section className={styles.section} aria-labelledby="composio-connected-h">
          <h3 className={styles.sectionLabel} id="composio-connected-h">
            {t("apps.composio.connectedSection")}
          </h3>
          <ul className={styles.grid}>
            {c.apps.map((a) => (
              <AppCell
                key={a.slug}
                app={a}
                busy={busy === a.slug}
                onEnabled={(enabled) =>
                  void run(a.slug, async () => {
                    const next = await client?.composioSetEnabled(a.slug, enabled);
                    if (next) {
                      c.setApps((prev) => upsertApp(prev, next));
                    }
                  })
                }
                onReconnect={() => void connect(a.slug, a.name)}
                onDisconnect={() =>
                  void run(a.slug, async () => {
                    const r = await client?.composioDisconnect(a.slug);
                    toast.show(
                      r && !r.remote_removed
                        ? t("apps.composio.disconnectedLocal", { name: a.name })
                        : t("apps.disconnectedToast", { name: a.name }),
                      "info"
                    );
                    await c.reloadApps();
                  })
                }
              />
            ))}
          </ul>
        </section>
      ) : null}

      {popular.length > 0 ? (
        <section className={styles.section} aria-labelledby="composio-popular-h">
          <h3 className={styles.sectionLabel} id="composio-popular-h">
            {t("apps.composio.popular")} · {t("apps.composio.via")}
          </h3>
          <ul className={styles.grid}>
            {popular.map((tk) => (
              <ToolkitTile
                key={tk.slug}
                toolkit={tk}
                busy={busy === tk.slug}
                onConnect={() => connect(tk.slug, tk.name)}
              />
            ))}
          </ul>
        </section>
      ) : null}

      <section className={styles.section} aria-labelledby="composio-catalog-h">
        <h3 className={styles.sectionLabel} id="composio-catalog-h">
          {t("apps.composio.catalog")} · {t("apps.composio.via")}
        </h3>
        {c.error ? (
          <p className={formStyles.error} role="alert">
            {c.error}
          </p>
        ) : null}
        {rest.length === 0 ? (
          <p className={styles.empty}>{c.loading ? t("apps.composio.loading") : t("apps.composio.noResults")}</p>
        ) : (
          <ul className={styles.grid}>
            {rest.map((tk) => (
              <ToolkitTile
                key={tk.slug}
                toolkit={tk}
                busy={busy === tk.slug}
                onConnect={() => connect(tk.slug, tk.name)}
              />
            ))}
          </ul>
        )}
        <div className={styles.actions}>
          {c.hasMore ? (
            <Button size="sm" variant="secondary" disabled={c.loading} onClick={() => void c.loadMore()}>
              {t("apps.composio.loadMore")}
            </Button>
          ) : null}
          <span className={styles.grow} />
          {confirmKey ? (
            <>
              <Button
                size="sm"
                variant="danger"
                data-testid="composio-remove-key-confirm"
                onClick={() =>
                  void run("key", async () => {
                    await client?.removeComposioKey();
                    setConfirmKey(false);
                    await c.reloadStatus();
                  })
                }
              >
                {t("apps.composio.removeKeyConfirm")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmKey(false)}>
                {t("common.cancel")}
              </Button>
            </>
          ) : (
            <Button size="sm" variant="ghost" onClick={() => setConfirmKey(true)}>
              {t("apps.composio.removeKey")}
            </Button>
          )}
        </div>
      </section>
    </>
  );
}
