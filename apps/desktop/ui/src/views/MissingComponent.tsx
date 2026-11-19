import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import type { Component, ComponentId } from "../api/types";
import { openExternal } from "../lib/externalLink";
import { runtimeFamily } from "../lib/mcpError";
import { useDaemon } from "../state/DaemonContext";
import own from "./AppsGrid.module.css";

const POLL_MS = 1500;

/** The installable component a failed launcher belongs to, or null (pipx, docker ...). */
export function componentFor(command: string | null | undefined): ComponentId | null {
  const family = runtimeFamily(command);
  return family === "uv" ? "uv" : family === "node" ? "node" : null;
}

/**
 * "This app needs uv": one button installs it after a confirm that shows the
 * exact command and its source, then the app starts again by itself.  Without
 * a package manager the official page is offered instead.
 */
export function MissingComponent({
  id,
  onReady
}: {
  id: ComponentId;
  /** The component is installed: start the app again. */
  onReady: () => void;
}) {
  const { t } = useTranslation();
  const { client } = useDaemon();
  const [info, setInfo] = useState<Component | null>(null);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const name = t(`apps.component.name.${id}`);

  useEffect(() => {
    let live = true;
    void client
      ?.component(id)
      .then((c) => {
        if (live) setInfo(c);
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [client, id]);

  const state = info?.state;
  useEffect(() => {
    if (!client || state !== "installing") {
      return undefined;
    }
    const timer = setInterval(() => {
      void client
        .component(id)
        .then((c) => {
          setInfo(c);
          if (c.state === "done" && c.installed) {
            onReady();
          }
        })
        .catch(() => undefined);
    }, POLL_MS);
    return () => clearInterval(timer);
  }, [client, id, state, onReady]);

  const install = () => {
    if (!client) return;
    setBusy(true);
    setAsking(false);
    void client
      .installComponent(id)
      .then(setInfo)
      .catch(() => setInfo((c) => (c ? { ...c, state: "failed", error: "unavailable" } : c)))
      .finally(() => setBusy(false));
  };

  return (
    <div className={own.need} data-testid={`component-${id}`}>
      <p>{t(`apps.component.needs.${id}`)}</p>
      {info?.installed ? (
        <Button size="sm" variant="primary" onClick={onReady}>
          {t("apps.reconnect")}
        </Button>
      ) : state === "installing" ? (
        <p role="status">{t("apps.component.installing", { name })}</p>
      ) : info && !info.can_install ? (
        <>
          <p>{t("apps.component.manual")}</p>
          <Button size="sm" variant="secondary" onClick={() => void openExternal(info.manual_command)}>
            {t("apps.component.openPage")}
          </Button>
        </>
      ) : asking && info ? (
        <>
          <p>{t("apps.component.confirm", { name })}</p>
          <pre className={own.cmd} data-testid={`component-command-${id}`}>
            {info.command}
          </pre>
          <p>{t("apps.component.source", { source: info.source })}</p>
          <div className={own.row}>
            <Button size="sm" variant="primary" disabled={busy} data-testid={`component-confirm-${id}`} onClick={install}>
              {t("apps.component.run")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setAsking(false)}>
              {t("common.cancel")}
            </Button>
          </div>
        </>
      ) : (
        <>
          {state === "failed" ? <p role="alert">{t(`apps.component.failed.${info?.error ?? "exit"}`)}</p> : null}
          <Button size="sm" variant="primary" disabled={!info || busy} data-testid={`component-install-${id}`} onClick={() => setAsking(true)}>
            {t("apps.component.install", { name })}
          </Button>
        </>
      )}
    </div>
  );
}
