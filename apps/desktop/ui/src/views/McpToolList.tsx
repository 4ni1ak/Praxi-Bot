import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { McpServer, McpTool } from "../api/types.ts";
import type { ApiClient } from "../api/client.ts";
import { nextExclude, toolRows } from "../lib/mcpTools.ts";
import styles from "./AppsModal.module.css";

/** Per-tool on/off switches for a connected server (writes `tool_exclude`). */
export function McpToolList({
  server,
  client,
  busy,
  onChange
}: {
  server: McpServer;
  client: ApiClient | null;
  busy: boolean;
  onChange: (exclude: string[]) => void;
}) {
  const { t } = useTranslation();
  const [tools, setTools] = useState<McpTool[] | null>(null);
  const exclude = server.tool_exclude ?? [];
  const allow = server.tool_allow ?? [];
  const excludeKey = exclude.join("\n");
  useEffect(() => {
    if (!client || server.status !== "connected") {
      setTools([]);
      return;
    }
    let live = true;
    client
      .listMcpTools(server.name)
      .then((list) => {
        if (live) setTools(list);
      })
      .catch(() => {
        if (live) setTools([]);
      });
    return () => {
      live = false;
    };
  }, [client, server.name, server.status, excludeKey]);
  const rows = tools ? toolRows(tools, exclude, allow) : [];
  if (rows === null) {
    return <p className={styles.panelLabel}>{t("apps.tools.restricted")}</p>;
  }
  if (rows.length === 0) {
    return null;
  }
  return (
    <div className={styles.toolList} data-testid={`mcp-tools-${server.name}`}>
      <span className={styles.panelLabel}>{t("apps.tools.label")}</span>
      {rows.map((r) => (
        <label key={r.tool} className={styles.toolRow} title={r.description}>
          <input
            type="checkbox"
            checked={r.enabled}
            disabled={busy}
            data-testid={`mcp-tool-${server.name}-${r.tool}`}
            onChange={(e) => onChange(nextExclude(exclude, r.tool, e.target.checked))}
          />
          <span className={styles.toolName}>{r.tool}</span>
          {r.effect && r.effect !== "read" ? (
            <small className={styles.panelLabel}>{t(`apps.tools.effect.${r.effect}`, r.effect)}</small>
          ) : null}
        </label>
      ))}
    </div>
  );
}
