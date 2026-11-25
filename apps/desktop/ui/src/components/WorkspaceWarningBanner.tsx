import { AlertTriangle, FolderOpen, Settings2, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { isTauriRuntime, openWorkspaceFolder } from "../api/connection";
import type { CleanupTarget, WorkspaceWarningDto } from "../api/types";
import { humanLine } from "../lib/humanError";
import { formatBytes } from "../lib/workspaceFormat";
import { useDaemon } from "../state/DaemonContext";
import { useToast } from "./Toast";
import styles from "./WorkspaceWarningBanner.module.css";

export function WorkspaceWarningBanner({
  onOpenStorage
}: {
  onOpenStorage: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const { client, activeComputerId, onDaemonEvent } = useDaemon();
  const [warning, setWarning] = useState<WorkspaceWarningDto | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [cleaning, setCleaning] = useState(false);

  const refresh = useCallback(async () => {
    if (!client) return;
    try {
      const overview = await client.listWorkspaces();
      setWarning(overview.active_warning);
    } catch {
      // Owner-only or unconfigured hosts stay silent here.
    }
  }, [client]);

  useEffect(() => {
    setDismissed(false);
    void refresh();
  }, [refresh]);

  useEffect(() => {
    return onDaemonEvent((env) => {
      if (env.event.type === "workspace_warning") {
        setWarning(env.event.warning);
        setDismissed(false);
      }
    });
  }, [onDaemonEvent]);

  if (!warning || dismissed) {
    return null;
  }

  const big = warning.largest_items[0];
  const suggestion = warning.suggestions[0];

  const handleCleanup = async (target: CleanupTarget) => {
    if (!client) return;
    setCleaning(true);
    try {
      const res = await client.runWorkspaceCleanup({ target });
      toast.show(
        t("workspace.cleanedSummary", { size: formatBytes(res.freed_bytes) }),
        "success"
      );
      await refresh();
    } catch (err) {
      toast.show(humanLine(err), "danger");
    } finally {
      setCleaning(false);
    }
  };

  const handleOpenFolder = async () => {
    if (isTauriRuntime() && activeComputerId === "local") {
      try {
        await openWorkspaceFolder(big?.folder_name);
        return;
      } catch (err) {
        toast.show(humanLine(err), "danger");
        return;
      }
    }
    onOpenStorage();
  };

  return (
    <div
      className={styles.banner}
      role="status"
      aria-live="polite"
      data-severity={warning.severity}
      data-testid="workspace-warning-banner"
    >
      <AlertTriangle className={styles.icon} aria-hidden="true" />
      <div className={styles.text}>
        <strong className={styles.title}>
          {warning.below_min_free && warning.available_bytes !== null
            ? t("workspace.warningLowFree", {
                free: formatBytes(warning.available_bytes)
              })
            : warning.threshold_percent !== null
              ? t("workspace.warningPercent", {
                  percent: warning.disk_used_percent ?? warning.threshold_percent
                })
              : t("workspace.warningBotLimit")}
        </strong>
        <span className={styles.hint}>
          {big
            ? t("workspace.warningBigItem", {
                path: `${big.folder_name}/${big.relative_path}`,
                size: formatBytes(big.size_bytes)
              })
            : t("workspace.warningCheckStorage")}
        </span>
      </div>
      <div className={styles.actions}>
        {suggestion ? (
          <Button
            size="sm"
            variant="primary"
            disabled={cleaning}
            onClick={() => void handleCleanup(suggestion.target)}
            data-testid="workspace-warning-cleanup"
          >
            <Trash2 aria-hidden="true" />
            {t(`workspace.cleanupTarget.${suggestion.target}`, {
              size: formatBytes(suggestion.reclaimable_bytes)
            })}
          </Button>
        ) : null}
        <Button size="sm" variant="ghost" onClick={() => void handleOpenFolder()}>
          <FolderOpen aria-hidden="true" />
          {t("workspace.openFolder")}
        </Button>
        <Button size="sm" variant="ghost" onClick={onOpenStorage}>
          <Settings2 aria-hidden="true" />
          {t("workspace.manageStorage")}
        </Button>
        <Button
          size="sm"
          variant="ghost"
          title={t("common.close")}
          aria-label={t("common.close")}
          onClick={() => setDismissed(true)}
        >
          <X aria-hidden="true" />
        </Button>
      </div>
    </div>
  );
}
