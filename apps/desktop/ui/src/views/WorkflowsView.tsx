import { ArrowDown, ArrowUp, Copy, Download, Play, Plus, Trash2, Upload } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Card } from "ui";
import type { WorkflowDefinition, WorkflowRunRecord } from "../api/types";
import { ConfirmDialog } from "../components/DialogParts";
import formStyles from "../components/Form.module.css";
import { useToast } from "../components/Toast";
import { botView } from "../lib/botProfile";
import { humanLine } from "../lib/humanError";
import {
  CHECK_KINDS,
  type CheckKind,
  checkOf,
  copyName,
  type EditRow,
  editSteps,
  importProblem,
  moveRow,
  removeRow,
  rowsOf,
  stepParams,
  stepView,
  type WaitDraft,
  waitProblem
} from "../lib/workflowEdit";
import { useDaemon } from "../state/DaemonContext";
import styles from "./WorkflowsView.module.css";

type Param = WorkflowDefinition["params"][number];
const NEW_WAIT: WaitDraft = { mode: "text", value: "", timeoutSecs: 15 };

function StepLine({ step, index }: { step: unknown; index: number }) {
  const { t } = useTranslation();
  const v = stepView(step);
  const used = stepParams(step);
  return (
    <div className={styles.stepText}>
      <strong>{t(`workflows.kind.${v.kind}`, { defaultValue: v.kind })}{v.action ? ` · ${v.action}` : ""}</strong>
      {v.target ? <span className={styles.target}>{v.target}</span> : null}
      {v.secretName ? <span className={styles.secret}>{t("workflows.secretTyped", { name: v.secretName })}</span>
        : v.value !== null ? <span className={styles.value}>{t("workflows.typed", { value: v.value })}</span> : null}
      {used.length ? <span className={styles.chips}>{used.map((n) => <code key={n}>{`{${n}}`}</code>)}</span> : null}
      <span className={styles.visuallyHidden}>{t("workflows.stepNumber", { n: index + 1 })}</span>
    </div>
  );
}

function History({ rows }: { rows: WorkflowRunRecord[] | null }) {
  const { t, i18n } = useTranslation();
  if (rows === null) return <p role="status">{t("workflows.loading")}</p>;
  if (rows.length === 0) return <p className={formStyles.hint}>{t("workflows.historyEmpty")}</p>;
  return (
    <ul className={styles.history}>
      {rows.map((r) => (
        <li key={r.run_id}>
          <time dateTime={new Date(r.started_at_ms).toISOString()}>{new Date(r.started_at_ms).toLocaleString(i18n.language, { dateStyle: "short", timeStyle: "short" })}</time>
          <ol>
            {r.steps.map((s) => (
              <li key={s.step_index} data-status={s.status}>
                {t("workflows.historyStep", { n: s.step_index + 1 })}: {t(`workflows.status.${s.status}`)}
                {s.reason ? <span className={styles.reason}> — {s.reason}</span> : null}
              </li>
            ))}
          </ol>
        </li>
      ))}
    </ul>
  );
}

export function WorkflowsView({ initialBotId }: { initialBotId?: string }) {
  const { t } = useTranslation();
  const { client, bots, chats } = useDaemon();
  const toast = useToast();
  const [botId, setBotId] = useState(initialBotId ?? bots[0]?.id ?? "");
  const [list, setList] = useState<WorkflowDefinition[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [history, setHistory] = useState<WorkflowRunRecord[] | null>(null);
  // editor draft
  const [rows, setRows] = useState<EditRow[]>([]);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [params, setParams] = useState<Param[]>([]);
  const [check, setCheck] = useState<{ kind: CheckKind; text: string } | null>(null);
  const [wait, setWait] = useState<WaitDraft>(NEW_WAIT);
  const [runValues, setRunValues] = useState<Record<string, string>>({});
  const [confirmDelete, setConfirmDelete] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  const current = useMemo(() => list?.find((w) => w.name === selected) ?? null, [list, selected]);
  const dirty = useMemo(() => {
    if (!current) return false;
    return JSON.stringify([rows, name, description, params, check]) !==
      JSON.stringify([rowsOf(current.steps.length), current.name, current.description, current.params, checkOf(current.success_check)]);
  }, [current, rows, name, description, params, check]);

  const load = useCallback(async () => {
    if (!client || !botId) return;
    try {
      setList(await client.listWorkflows(botId));
      setError(null);
    } catch (e) { setError(humanLine(e)); }
  }, [client, botId]);
  useEffect(() => { setList(null); setSelected(null); void load(); }, [load]);

  useEffect(() => {
    if (!current) return;
    setRows(rowsOf(current.steps.length));
    setName(current.name);
    setDescription(current.description);
    setParams(current.params);
    setCheck(checkOf(current.success_check));
    setWait(NEW_WAIT);
    setRunValues({});
    setHistory(null);
    client?.workflowHistory(current.name, botId).then(setHistory).catch(() => setHistory([]));
    // Reload the draft only when another workflow, or a new saved revision, is shown.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [current?.name, current?.revision]);

  const act = async (work: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try { await work(); } catch (e) { setError(humanLine(e)); } finally { setBusy(false); }
  };

  const save = () => current && client && act(async () => {
    if (rows.length === 0) throw new Error(t("workflows.needOneStep"));
    const saved = await client.editWorkflow(current.name, {
      bot_id: botId, revision: current.revision, name, description,
      params: params.map((p) => ({ name: p.name, default: p.default ?? null, description: p.description })),
      steps: editSteps(rows),
      success_check: check && check.text.trim() ? { kind: check.kind, text: check.text.trim() } : null
    });
    await load();
    setSelected(saved.name);
    toast.show(t("workflows.saved"), "success");
  });

  const run = () => current && client && act(async () => {
    const entered = Object.fromEntries(Object.entries(runValues).filter(([, v]) => v !== ""));
    const chat = chats.find((c) => c.bot_id === botId) ?? await client.createChat(botId, current.name);
    await client.runWorkflow(current.name, { chat_id: chat.id, params: entered });
    toast.show(t("workflows.started"), "success");
    setTimeout(() => client.workflowHistory(current.name, botId).then(setHistory).catch(() => undefined), 3000);
  });

  const duplicate = () => current && client && act(async () => {
    const copy = await client.duplicateWorkflow(current.name, {
      bot_id: botId, new_name: copyName(current.name, (list ?? []).map((w) => w.name))
    });
    await load();
    setSelected(copy.name);
  });

  const exportFile = () => current && client && act(async () => {
    const file = await client.exportWorkflow(current.name, botId);
    const url = URL.createObjectURL(new Blob([file.content], { type: "application/json" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = file.file_name;
    a.click();
    URL.revokeObjectURL(url);
  });

  const importFile = (file: File) => client && act(async () => {
    const text = await file.text();
    const problem = importProblem(text);
    if (problem) throw new Error(t(`workflows.import.${problem}`));
    const imported = await client.importWorkflow({ bot_id: botId, content: text, name: null });
    await load();
    setSelected(imported.name);
    toast.show(t("workflows.importedUntrusted"), "success");
  });

  const remove = () => current && client && act(async () => {
    await client.deleteWorkflow(current.name, botId);
    setConfirmDelete(false);
    setSelected(null);
    await load();
  });

  const waitError = waitProblem(wait);
  const addWait = () => {
    if (waitError) return;
    setRows((r) => [...r, { wait }]);
    setWait(NEW_WAIT);
  };
  const untrusted = (current?.source as { untrusted_content?: boolean } | null | undefined)?.untrusted_content === true;

  return (
    <section className={styles.pane} aria-labelledby="workflows-title">
      <div className={styles.container}>
        <header className={styles.header}>
          <div>
            <h1 id="workflows-title">{t("workflows.title")}</h1>
            <p>{t("workflows.intro")}</p>
          </div>
          <div className={styles.row}>
            <label>{t("workflows.bot")}
              <select className={formStyles.select} value={botId} onChange={(e) => setBotId(e.target.value)}>
                {bots.map((b) => <option key={b.id} value={b.id}>{botView(b).name}</option>)}
              </select>
            </label>
            <input ref={fileInput} type="file" accept=".json,application/json" hidden data-testid="workflow-import-input"
              onChange={(e) => { const f = e.target.files?.[0]; e.target.value = ""; if (f) void importFile(f); }} />
            <Button size="sm" disabled={!client || busy} onClick={() => fileInput.current?.click()}><Upload aria-hidden="true" />{t("workflows.import.button")}</Button>
          </div>
        </header>
        {error ? <p className={formStyles.error} role="alert">{error}</p> : null}
        <div className={styles.columns}>
          <nav aria-label={t("workflows.listLabel")} className={styles.list}>
            {list === null ? <p role="status">{t("workflows.loading")}</p>
              : list.length === 0 ? <Card padding="md"><h2>{t("workflows.emptyTitle")}</h2><p>{t("workflows.emptyHint")}</p></Card>
              : <ul>{list.map((w) => (
                <li key={w.name}>
                  <button type="button" className={styles.item} aria-current={w.name === selected ? "true" : undefined} onClick={() => setSelected(w.name)}>
                    <strong>{w.name}</strong>
                    <span>{t("workflows.stepCount", { count: w.steps.length })}</span>
                  </button>
                </li>))}</ul>}
          </nav>
          {current ? (
            <Card padding="md" className={styles.detail}>
              {untrusted ? <p role="note" className={styles.warn}>{t("workflows.untrusted")}</p> : null}
              <div className={styles.grid}>
                <label className={formStyles.field}><span className={formStyles.label}>{t("workflows.name")}</span>
                  <input className={formStyles.input} value={name} maxLength={64} pattern="[a-z0-9_-]+" onChange={(e) => setName(e.target.value)} /></label>
                <label className={formStyles.field}><span className={formStyles.label}>{t("workflows.description")}</span>
                  <textarea className={formStyles.textarea} rows={2} value={description} onChange={(e) => setDescription(e.target.value)} /></label>
              </div>
              <h2>{t("workflows.steps")}</h2>
              <ol className={styles.timeline}>
                {rows.map((row, i) => (
                  <li key={"keep" in row ? `k${row.keep}` : `w${i}`}>
                    {"keep" in row ? <StepLine step={current.steps[row.keep]} index={i} />
                      : <StepLine step={{ kind: "wait_for", ...(row.wait.mode === "window" ? { window_title_pattern: row.wait.value } : row.wait.mode === "text" ? { text: row.wait.value } : {}) }} index={i} />}
                    <span className={styles.row}>
                      <Button size="sm" variant="ghost" disabled={i === 0 || busy} aria-label={t("workflows.moveUp", { n: i + 1 })} title={t("workflows.moveUp", { n: i + 1 })} onClick={() => setRows(moveRow(rows, i, -1))}><ArrowUp aria-hidden="true" /></Button>
                      <Button size="sm" variant="ghost" disabled={i === rows.length - 1 || busy} aria-label={t("workflows.moveDown", { n: i + 1 })} title={t("workflows.moveDown", { n: i + 1 })} onClick={() => setRows(moveRow(rows, i, 1))}><ArrowDown aria-hidden="true" /></Button>
                      <Button size="sm" variant="ghost" disabled={busy} aria-label={t("workflows.removeStep", { n: i + 1 })} title={t("workflows.removeStep", { n: i + 1 })} onClick={() => setRows(removeRow(rows, i))}><Trash2 aria-hidden="true" /></Button>
                    </span>
                  </li>
                ))}
              </ol>
              <fieldset className={styles.palette}>
                <legend>{t("workflows.addWait")}</legend>
                <label>{t("workflows.waitFor")}
                  <select className={formStyles.select} value={wait.mode} onChange={(e) => setWait({ ...wait, mode: e.target.value as WaitDraft["mode"] })}>
                    {(["text", "window", "idle"] as const).map((m) => <option key={m} value={m}>{t(`workflows.waitMode.${m}`)}</option>)}
                  </select></label>
                {wait.mode !== "idle" ? <label>{t("workflows.waitValue")}
                  <input className={formStyles.input} value={wait.value} onChange={(e) => setWait({ ...wait, value: e.target.value })} /></label> : null}
                <label>{t("workflows.waitSeconds")}
                  <input className={formStyles.input} type="number" min={1} max={600} value={wait.timeoutSecs} onChange={(e) => setWait({ ...wait, timeoutSecs: Number(e.target.value) })} /></label>
                <Button size="sm" disabled={waitError !== null || busy} onClick={addWait}><Plus aria-hidden="true" />{t("workflows.addWaitButton")}</Button>
              </fieldset>
              <fieldset className={styles.palette}>
                <legend>{t("workflows.finalCheck")}</legend>
                <p className={formStyles.hint}>{t("workflows.finalCheckHint")}</p>
                <label>{t("workflows.checkKind")}
                  <select className={formStyles.select} value={check?.kind ?? ""} onChange={(e) => setCheck(e.target.value ? { kind: e.target.value as CheckKind, text: check?.text ?? "" } : null)}>
                    <option value="">{t("workflows.checkNone")}</option>
                    {CHECK_KINDS.map((k) => <option key={k} value={k}>{t(`workflows.checkKinds.${k}`)}</option>)}
                  </select></label>
                {check ? <label>{t("workflows.waitValue")}
                  <input className={formStyles.input} value={check.text} onChange={(e) => setCheck({ ...check, text: e.target.value })} /></label> : null}
              </fieldset>
              {params.length > 0 ? <>
                <h2>{t("workflows.parameters")}</h2>
                <ul className={styles.params}>
                  {params.map((p, i) => (
                    <li key={p.name}>
                      <code>{`{${p.name}}`}</code>
                      <label>{t("workflows.paramDescription")}
                        <input className={formStyles.input} value={p.description} onChange={(e) => setParams(params.map((x, j) => j === i ? { ...x, description: e.target.value } : x))} /></label>
                      {p.type === "secret" || current.params[i].default == null
                        ? <span className={formStyles.hint}>{t("workflows.askEveryTime")}</span>
                        : <label>{t("workflows.defaultValue")}
                          <input className={formStyles.input} value={p.default ?? ""} onChange={(e) => setParams(params.map((x, j) => j === i ? { ...x, default: e.target.value } : x))} /></label>}
                    </li>))}
                </ul></> : null}
              <div className={styles.row}>
                <Button variant="primary" disabled={!dirty || busy || !name.trim()} onClick={() => void save()}>{t("workflows.save")}</Button>
                <Button disabled={busy} onClick={() => void duplicate()}><Copy aria-hidden="true" />{t("workflows.duplicate")}</Button>
                <Button disabled={busy} onClick={() => void exportFile()}><Download aria-hidden="true" />{t("workflows.export")}</Button>
                <Button disabled={busy} onClick={() => setConfirmDelete(true)}><Trash2 aria-hidden="true" />{t("workflows.delete")}</Button>
              </div>
              <h2>{t("workflows.runTitle")}</h2>
              <p className={formStyles.hint}>{dirty ? t("workflows.saveFirst") : t("workflows.runHint")}</p>
              <div className={styles.grid}>
                {current.params.map((p) => (
                  <label key={p.name} className={formStyles.field}>
                    <span className={formStyles.label}>{p.name}{p.description ? ` — ${p.description}` : ""}</span>
                    <input className={formStyles.input} type={p.type === "secret" ? "password" : "text"} autoComplete="off"
                      placeholder={p.default ?? t("workflows.required")} value={runValues[p.name] ?? ""}
                      onChange={(e) => setRunValues({ ...runValues, [p.name]: e.target.value })} />
                  </label>))}
              </div>
              <Button variant="primary" disabled={dirty || busy || !client} onClick={() => void run()}><Play aria-hidden="true" />{t("workflows.run")}</Button>
              <h2>{t("workflows.history")}</h2>
              <p className={formStyles.hint}>{t("workflows.historyNote")}</p>
              <History rows={history} />
            </Card>
          ) : list && list.length > 0 ? <Card padding="md"><p>{t("workflows.pick")}</p></Card> : null}
        </div>
      </div>
      <ConfirmDialog open={confirmDelete} title={t("workflows.deleteTitle", { name: current?.name ?? "" })} body={t("workflows.deleteBody")}
        confirmLabel={t("workflows.delete")} cancelLabel={t("common.cancel")} tone="danger"
        onCancel={() => setConfirmDelete(false)} onConfirm={() => void remove()} />
    </section>
  );
}
