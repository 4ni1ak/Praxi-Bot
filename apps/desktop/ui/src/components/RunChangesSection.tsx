import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  ChevronDown,
  ChevronRight,
  FileDiff,
  Info,
  Undo2
} from "lucide-react";
import { Button } from "ui";
import { ApiError, type ApiClient } from "../api/client";
import type { FileChange, RestorePreview, RunChanges } from "../api/types";
import { highlight, type Token } from "../lib/highlight";
import {
  displayPath,
  evaluateRestoreGate,
  languageForPath,
  parseUnifiedDiff,
  verificationLabel,
  restoreConsentKeys,
  type DiffRow
} from "../lib/fileChanges";
import { Modal } from "./Modal";
import { useToast } from "./Toast";
import styles from "./RunChangesSection.module.css";

interface PendingRestore {
  checkpointId: string;
  targetPath: string | null;
}

function renderTokens(tokens: Token[]) {
  return tokens.map((tok, idx) =>
    tok.type === "plain" ? (
      tok.text
    ) : (
      <span key={idx} className={styles[`tok_${tok.type}`]}>
        {tok.text}
      </span>
    )
  );
}

function formatByteLabel(
  value: number | null | undefined,
  t: (k: string, opts?: Record<string, unknown>) => string
): string {
  if (value === null || value === undefined) {
    return t("chat.changes.bytes", { count: 0 });
  }
  return t("chat.changes.bytes", { count: value });
}

function InlineDiffView({ change }: { change: FileChange }) {
  const { t } = useTranslation();
  const rows: DiffRow[] = useMemo(
    () => (change.text ? parseUnifiedDiff(change.text.unified) : []),
    [change.text]
  );
  const lang = useMemo(() => languageForPath(change.path), [change.path]);

  if (!change.text || rows.length === 0) {
    if (change.diff_omitted_reason) {
      return (
        <div className={styles.omittedNote} data-testid="diff-omitted-note">
          {t(`chat.changes.omitted.${change.diff_omitted_reason}`, {
            before: formatByteLabel(change.before_size, t),
            after: formatByteLabel(change.after_size, t)
          })}
        </div>
      );
    }
    if (!change.current_matches_after) {
      return (
        <div className={styles.omittedNote}>
          {t(verificationLabel(change) ?? "chat.changes.omitted.modified_since")}
        </div>
      );
    }
    return null;
  }

  return (
    <div className={styles.diffWrap} data-testid="inline-diff">
      <table className={styles.diffTable}>
        <tbody>
          {rows.map((row, index) => {
            if (row.kind === "hunk") {
              return (
                <tr key={index} className={styles.diffRow_hunk}>
                  <td className={styles.lineNum} />
                  <td className={styles.lineNum} />
                  <td className={styles.lineMark} />
                  <td className={styles.hunkCell}>{row.text}</td>
                </tr>
              );
            }
            const mark =
              row.kind === "add" ? "+" : row.kind === "del" ? "-" : " ";
            const tokens = highlight(row.text, lang);
            return (
              <tr key={index} className={styles[`diffRow_${row.kind}`]}>
                <td className={styles.lineNum}>{row.oldLine ?? ""}</td>
                <td className={styles.lineNum}>{row.newLine ?? ""}</td>
                <td className={styles.lineMark}>{mark}</td>
                <td className={styles.lineCode}>{renderTokens(tokens)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function RunChangesSection({
  runId,
  client,
  restoreSignal,
  safetyCheckpointIds = [],
  defaultExpanded = false
}: {
  runId: string;
  client: ApiClient | null;
  /** Increments whenever a `checkpoint_restored` event arrives for this run. */
  restoreSignal: number;
  safetyCheckpointIds?: string[];
  defaultExpanded?: boolean;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [data, setData] = useState<RunChanges | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(defaultExpanded);
  const [expandedFiles, setExpandedFiles] = useState<Record<string, boolean>>(
    {}
  );

  const [pending, setPending] = useState<PendingRestore | null>(null);
  const [preview, setPreview] = useState<RestorePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [overwriteChecked, setOverwriteChecked] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [conflictRequiresConfirm, setConflictRequiresConfirm] = useState(false);
  const [safetyIds, setSafetyIds] = useState<string[]>([]);
  const [restoreWarning, setRestoreWarning] = useState<string | null>(null);

  const loadChanges = useCallback(async (offset = 0) => {
    if (!client) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const next = await client.getRunChanges(runId, offset);
      setData((previous) => offset > 0 && previous ? { ...next, changes: [...previous.changes, ...next.changes] } : next);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [client, runId]);

  useEffect(() => {
    if (open) void loadChanges();
  }, [loadChanges, restoreSignal, open]);

  const toggleFile = (path: string) => {
    setExpandedFiles((prev) => ({
      ...prev,
      [path]: !(prev[path] ?? true)
    }));
  };

  const openRestoreModal = async (
    checkpointId: string,
    targetPath: string | null
  ) => {
    if (!client) {
      return;
    }
    setConflictRequiresConfirm(false);
    setPending({ checkpointId, targetPath });
    setPreview(null);
    setPreviewError(null);
    setOverwriteChecked(false);
    try {
      const res = await client.previewRestore(runId, checkpointId);
      setPreview(res);
    } catch (e) {
      setPreviewError(e instanceof Error ? e.message : String(e));
    }
  };

  const closeRestoreModal = () => {
    if (restoring) {
      return;
    }
    setPending(null);
    setPreview(null);
    setPreviewError(null);
    setOverwriteChecked(false);
    setConflictRequiresConfirm(false);
  };

  const gate = evaluateRestoreGate(preview, overwriteChecked, restoring, conflictRequiresConfirm);

  const confirmRestore = async () => {
    if (!client || !pending || !gate.canConfirm) {
      return;
    }
    setRestoring(true);
    setPreviewError(null);
    try {
      const res = await client.restoreRunChanges(
        runId,
        pending.checkpointId,
        gate.requiresForce
      );
      setSafetyIds(res.safety_checkpoint_ids);
      setRestoreWarning([res.error, ...res.warnings].filter(Boolean).join("; ") || null);
      setPending(null);
      setPreview(null);
      setOverwriteChecked(false);
      toast.show(
        t(res.partial ? "chat.changes.restorePartial" : "chat.changes.restoreDone", { count: res.restored.length }),
        res.partial ? "danger" : "success"
      );
      await loadChanges();
    } catch (e) {
      setPreviewError(
        t("chat.changes.restoreFailed", {
          message: e instanceof Error ? e.message : String(e)
        })
      );
      // Discard the old clean preview immediately, and reset explicit consent.
      if (e instanceof ApiError && e.status === 409 && (e.code === "modified_since" || e.code === "could_not_verify")) {
        setPreview(null);
        setConflictRequiresConfirm(true);
        setOverwriteChecked(false);
        try {
          setPreview(await client.previewRestore(runId, pending.checkpointId));
        } catch (previewFailure) {
          setPreviewError(previewFailure instanceof Error ? previewFailure.message : String(previewFailure));
        }
      }
    } finally {
      setRestoring(false);
    }
  };

  const changes = data?.changes ?? [];

  return (
    <div className={styles.container} data-testid="run-changes-section">
      <div className={styles.bar}>
        <button
          type="button"
          className={[styles.tabButton, open ? styles.tabButtonOpen : ""].join(
            " "
          )}
          aria-expanded={open}
          onClick={() => setOpen((prev) => !prev)}
          data-testid="run-changes-toggle"
        >
          <FileDiff className={styles.tabIcon} aria-hidden="true" />
          <span>
            {changes.length > 0
              ? t("chat.changes.title", { count: changes.length })
              : t("chat.changes.tab")}
          </span>
          {open ? (
            <ChevronDown className={styles.tabIcon} aria-hidden="true" />
          ) : (
            <ChevronRight className={styles.tabIcon} aria-hidden="true" />
          )}
        </button>
      </div>

      {open ? (
        <div className={styles.panel}>
          <div className={styles.panelHeader}>
            <span className={styles.panelTitle}>
              {t("chat.changes.title", { count: changes.length })}
            </span>
            {changes.length > 0 ? (
              <Button
                size="sm"
                variant="secondary"
                onClick={() => void openRestoreModal("all", null)}
                data-testid="undo-all-button"
              >
                <Undo2 />
                {t("chat.changes.undoAll")}
              </Button>
            ) : null}
          </div>

          {[...new Set([...safetyCheckpointIds, ...safetyIds])].map((id) => (
            <p key={id} className={styles.metaText} data-testid="safety-checkpoint">
              {t("chat.changes.safetyCheckpoint", { id })}
              <Button size="sm" variant="secondary" onClick={() => void openRestoreModal(id, null)} data-testid="undo-undo-button">
                {t("chat.changes.undoUndo")}
              </Button>
            </p>
          ))}
          {restoreWarning ? <p role="alert">{restoreWarning}</p> : null}
          <div className={styles.disclaimer} role="note">
            <Info className={styles.disclaimerIcon} aria-hidden="true" />
            <span>{t("chat.changes.scopeDisclaimer")}</span>
          </div>

          {loading && !data ? (
            <p className={styles.statusMessage}>{t("chat.changes.loading")}</p>
          ) : null}
          {error ? (
            <p className={styles.errorMessage}>
              {t("chat.changes.loadError", { message: error })}
            </p>
          ) : null}

          {!loading && !error && changes.length === 0 ? (
            <p className={styles.statusMessage}>{t("chat.changes.empty")}</p>
          ) : null}

          <div className={styles.list}>
            {changes.map((change) => {
              const fileExpanded = expandedFiles[change.path] ?? true;
              const shownPath = displayPath(change.path);
              return (
                <div
                  key={change.path}
                  className={styles.fileRow}
                  data-testid="changed-file-row"
                >
                  <div className={styles.fileHead}>
                    <div className={styles.fileMain}>
                      <button
                        type="button"
                        className={styles.fileToggle}
                        aria-expanded={fileExpanded}
                        aria-label={
                          fileExpanded
                            ? t("chat.changes.hideDiff")
                            : t("chat.changes.showDiff")
                        }
                        onClick={() => toggleFile(change.path)}
                      >
                        {fileExpanded ? (
                          <ChevronDown
                            className={styles.tabIcon}
                            aria-hidden="true"
                          />
                        ) : (
                          <ChevronRight
                            className={styles.tabIcon}
                            aria-hidden="true"
                          />
                        )}
                        <span>{shownPath}</span>
                      </button>

                      <span
                        className={[
                          styles.badge,
                          styles[`badge_${change.kind}`]
                        ].join(" ")}
                      >
                        {t(`chat.changes.kind.${change.kind}`)}
                      </span>

                      {!change.accessible ? (
                        <span
                          className={[styles.badge, styles.badgeWarning].join(
                            " "
                          )}
                        >
                          {t("chat.changes.omitted.not_accessible")}
                        </span>
                      ) : !change.current_matches_after ? (
                        <span
                          className={[styles.badge, styles.badgeWarning].join(
                            " "
                          )}
                        >
                          {t(verificationLabel(change) ?? "chat.changes.changedOutsideRun", { run: change.changed_by_run })}
                        </span>
                      ) : null}

                      {change.moved_from ? (
                        <span className={styles.metaText}>
                          {t("chat.changes.movedFrom", {
                            path: displayPath(change.moved_from)
                          })}
                        </span>
                      ) : null}

                      {change.via_link_of ? (
                        <span className={styles.metaText}>
                          {t("chat.changes.viaLink", {
                            path: displayPath(change.via_link_of)
                          })}
                        </span>
                      ) : null}

                      {change.text ? (
                        <span className={styles.stats}>
                          {t("chat.changes.diffStats", {
                            added: change.text.added_lines,
                            removed: change.text.removed_lines
                          })}
                        </span>
                      ) : null}
                    </div>

                    <div className={styles.fileActions}>
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={!change.accessible}
                        onClick={() =>
                          void openRestoreModal(change.checkpoint_id, shownPath)
                        }
                        data-testid="undo-file-button"
                      >
                        <Undo2 />
                        {t("chat.changes.undoFile")}
                      </Button>
                    </div>
                  </div>

                  {fileExpanded ? <InlineDiffView change={change} /> : null}
                </div>
              );
            })}
          </div>
          {data?.next_offset !== null && data?.next_offset !== undefined ? (
            <Button size="sm" disabled={loading} onClick={() => void loadChanges(data.next_offset ?? 0)} data-testid="changes-load-more">
              {t("chat.changes.loadMore")}
            </Button>
          ) : null}
        </div>
      ) : null}

      <Modal
        open={pending !== null}
        onClose={closeRestoreModal}
        size="md"
        label={
          pending?.targetPath
            ? t("chat.changes.modal.titleSingle", { path: pending.targetPath })
            : t("chat.changes.modal.titleAll")
        }
      >
        <div className={styles.modalBody}>
          <p className={styles.metaText}>{t("chat.changes.modal.safetyNote")}</p>

          {!preview && !previewError ? (
            <p className={styles.statusMessage}>
              {t("chat.changes.modal.previewLoading")}
            </p>
          ) : null}

          {preview ? (
            <>
              <div className={styles.previewList}>
                {preview.entries.map((entry) => (
                  <div key={entry.path} className={styles.previewEntry}>
                    <span className={styles.previewPath}>
                      {displayPath(entry.path)}
                    </span>
                    <span className={styles.previewAction}>
                      {!entry.accessible ? t("chat.changes.omitted.not_accessible") : t(`chat.changes.modal.action.${entry.action}`)}
                      {!entry.current_matches_after && entry.accessible ? t(verificationLabel(entry) ?? "chat.changes.changedOutsideRun", { run: entry.changed_by_run }) : null}
                    </span>
                  </div>
                ))}
              </div>

              {gate.needsOverwriteConfirm ? (
                <>
                  <div className={styles.warningBox} role="alert">
                    <AlertTriangle
                      className={styles.warningIcon}
                      aria-hidden="true"
                    />
                    <span>{t(restoreConsentKeys(preview).warning)}</span>
                  </div>

                  <label className={styles.checkboxRow}>
                    <input
                      type="checkbox"
                      checked={overwriteChecked}
                      onChange={(e) => setOverwriteChecked(e.target.checked)}
                      data-testid="overwrite-newer-checkbox"
                    />
                    <span>{t(restoreConsentKeys(preview).label)}</span>
                  </label>
                </>
              ) : null}
            </>
          ) : null}

          {previewError ? (
            <p className={styles.errorMessage}>{previewError}</p>
          ) : null}

          <div className={styles.modalActions}>
            <Button
              size="sm"
              variant="ghost"
              disabled={restoring}
              onClick={closeRestoreModal}
            >
              {t("chat.changes.modal.cancel")}
            </Button>
            <Button
              size="sm"
              variant={gate.needsOverwriteConfirm ? "danger" : "primary"}
              disabled={!gate.canConfirm}
              onClick={() => void confirmRestore()}
              data-testid="confirm-restore-button"
            >
              {restoring
                ? t("chat.changes.modal.restoring")
                : t("chat.changes.modal.confirm")}
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
