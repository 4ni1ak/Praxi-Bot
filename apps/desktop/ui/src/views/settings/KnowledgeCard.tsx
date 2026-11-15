import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button, Card } from "ui";
import type { Chat, KnowledgeEmailCapture, KnowledgeEmbeddingProgress, KnowledgeLocalArtifacts, KnowledgeSource, KnowledgeSourceKind, KnowledgeSettings, KnowledgeSettingsInput } from "../../api/types";
import { useDaemon } from "../../state/DaemonContext";
import { useProviders } from "../../state/useProviders";
import { advanceKnowledgeEmbeddings } from "../../lib/knowledgeEmbedding";
import form from "../../components/Form.module.css";
import styles from "./KnowledgeCard.module.css";

const KINDS: KnowledgeSourceKind[] = ["file", "folder", "saved_web", "conversation", "email", "note", "shared_brain"];
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

/** Bot-scoped source selection and separate document/query egress consent. */
export function KnowledgeCard() {
  const { t } = useTranslation();
  const { client, bots } = useDaemon();
  const { dto: providers } = useProviders();
  const [bot, setBot] = useState("");
  const activeBot = bot || bots[0]?.id || "";
  const [sources, setSources] = useState<KnowledgeSource[]>([]);
  const [settings, setSettings] = useState<KnowledgeSettings | null>(null);
  const [chats, setChats] = useState<Chat[]>([]);
  const [kind, setKind] = useState<KnowledgeSourceKind>("file");
  const [label, setLabel] = useState("");
  const [path, setPath] = useState("");
  const [content, setContent] = useState("");
  const [url, setUrl] = useState("");
  const [html, setHtml] = useState(false);
  const [chatId, setChatId] = useState("");
  const [captures, setCaptures] = useState<KnowledgeEmailCapture[]>([]);
  const [emailError, setEmailError] = useState<string | null>(null);
  const [captureId, setCaptureId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const [cursors, setCursors] = useState<Record<string, string>>({});
  const [grant, setGrant] = useState<KnowledgeSource | null>(null);
  const [removing, setRemoving] = useState<KnowledgeSource | null>(null);
  const [renaming, setRenaming] = useState<KnowledgeSource | null>(null);
  const [renameLabel, setRenameLabel] = useState("");
  const [rebinding, setRebinding] = useState<KnowledgeSource | null>(null);
  const [bindingPath, setBindingPath] = useState("");
  const [indexFailures, setIndexFailures] = useState<Record<string, { locator: string; code: string }[]>>({});
  const [embeddingProgress, setEmbeddingProgress] = useState<Record<string, KnowledgeEmbeddingProgress>>({});
  const epoch = useRef(0);

  const reload = useCallback(async () => {
    if (!client || !activeBot) return;
    const current = ++epoch.current;
    const [next, config, allChats, mail] = await Promise.all([
      client.listKnowledgeSources(activeBot), client.getKnowledgeSettings(activeBot), client.listChats(),
      client.listKnowledgeEmailCaptures(activeBot).then((captures) => ({ captures, error: null }), (error: unknown) => ({ captures: [], error: message(error) }))
    ]);
    if (current !== epoch.current) return;
    setSources(next); setSettings(config); setChats(allChats.filter((chat) => chat.bot_id === activeBot)); setCaptures(mail.captures); setEmailError(mail.error);
  }, [client, activeBot]);

  useEffect(() => {
    setSources([]); setSettings(null); setGrant(null); setRemoving(null); setRenaming(null); setRebinding(null); setCursors({}); setError(null); setEmailError(null); setEmbeddingProgress({}); setIndexFailures({});
    void reload().catch((e: unknown) => setError(message(e)));
    return () => { epoch.current += 1; };
  }, [reload]);

  const act = async (operation: () => Promise<unknown>) => {
    setBusy(true); setError(null); setNotice("");
    try { await operation(); await reload(); }
    catch (e) { setError(message(e)); await reload().catch(() => undefined); }
    finally { setBusy(false); }
  };

  const add = () => act(async () => {
    if (!client) return;
    const capture = captures.find((item) => item.tool_call_id === captureId);
    const result = await client.createKnowledgeSource(activeBot, {
      kind, label: kind === "shared_brain" ? label.trim() || t("knowledge.defaultBrainName") : label, path: kind === "file" || kind === "folder" ? path : null,
      content: kind === "note" || kind === "saved_web" ? content : null,
      url: kind === "saved_web" ? url : null, html,
      chat_id: kind === "conversation" ? chatId : null,
      account: kind === "email" ? capture?.account ?? null : null, run_id: kind === "email" ? capture?.run_id ?? null : null,
      tool_call_id: kind === "email" ? capture?.tool_call_id ?? null : null
    });
    if (result.indexing.next_cursor) setCursors((previous) => ({ ...previous, [result.source.id]: result.indexing.next_cursor! }));
    setIndexFailures((previous) => ({ ...previous, [result.source.id]: result.indexing.failures ?? [] }));
    setNotice(t("knowledge.indexed", { count: result.indexing.processed }));
    setLabel(""); setContent("");
  });

  const change = (source: KnowledgeSource, action: "enable" | "disable" | "grant_remote" | "revoke_remote") => act(async () => {
    await client?.updateKnowledgeSource(activeBot, source.id, {
      action, expected_revision: source.revision, label: null,
      profile_fingerprint: action === "grant_remote" ? settings?.embedding?.profile_fingerprint ?? null : null
    });
    setGrant(null);
  });

  return <Card padding="md" className={styles.root}>
    <h3 className={styles.heading}>{t("knowledge.title")}</h3>
    <p className={form.hint}>{t("knowledge.description")}</p>
    <p className={form.hint}>{t("knowledge.importHint")}</p>
    <label className={form.field}>{t("knowledge.bot")}
      <select className={form.select} value={activeBot} disabled={busy} onChange={(e) => setBot(e.target.value)}>
        {bots.map((item) => <option key={item.id} value={item.id}>{item.name}</option>)}
      </select>
    </label>
    {!activeBot ? <p>{t("knowledge.noBot")}</p> : <>
      {error ? <p role="alert" className={form.error}>{error}</p> : null}
      {notice ? <p role="status" className={form.success}>{notice}</p> : null}
      <ul className={styles.list}>
        {sources.map((source) => <li key={source.id} className={styles.source}>
          <strong>{source.label}</strong>
          <span className={form.hint}>{t(`knowledge.kinds.${source.kind}`)} · {t("knowledge.counts", { documents: source.documents, chunks: source.chunks })}</span>
          {["file", "folder", "saved_web", "shared_brain"].includes(source.kind) && !source.requires_path_selection ? <code className={styles.locator}>{source.locator}</code> : null}
          <span className={form.hint}>{source.enabled ? t("knowledge.active") : t("knowledge.disabled")}</span>
          {source.requires_path_selection ? <p className={form.hint}>{t("knowledge.reselectHint")}</p> : null}
          {indexFailures[source.id]?.length ? <details><summary>{t("knowledge.skippedFiles", { count: indexFailures[source.id].length })}</summary><ul>{indexFailures[source.id].map((failure) => <li key={failure.locator}><code className={styles.locator}>{failure.locator}</code> · {t("knowledge.fileSkipped")}</li>)}</ul></details> : null}
          {embeddingProgress[source.id] ? <p role="status" className={form.hint}>
            {t(`knowledge.embeddingStatus.${embeddingProgress[source.id].status}`)} · {t("knowledge.embeddingRemaining", { count: embeddingProgress[source.id].remaining_chunks })}
            {embeddingProgress[source.id].next_attempt_ms ? <> · {t("knowledge.embeddingRetry", { time: new Date(embeddingProgress[source.id].next_attempt_ms!).toLocaleString() })}</> : null}
          </p> : null}
          <div className={styles.actions}>
            {source.requires_path_selection ? <Button size="sm" variant="secondary" disabled={busy} onClick={() => { setRebinding(source); setBindingPath(""); }}>{t("knowledge.reselect")}</Button> : <Button size="sm" variant="secondary" disabled={busy} onClick={() => void change(source, source.enabled ? "disable" : "enable")}>{t(source.enabled ? "knowledge.disable" : "knowledge.reauthorize")}</Button>}
            <Button size="sm" variant="secondary" disabled={busy || !source.enabled} onClick={() => void act(async () => {
              const result = await client?.reindexKnowledgeSource(activeBot, source.id, source.revision, cursors[source.id]);
              if (!result) return;
              setCursors((previous) => { const next = { ...previous }; if (result.indexing.next_cursor) next[source.id] = result.indexing.next_cursor; else delete next[source.id]; return next; });
              setIndexFailures((previous) => ({ ...previous, [source.id]: result.indexing.failures ?? [] }));
              setNotice(t("knowledge.indexed", { count: result.indexing.processed }));
            })}>{t(cursors[source.id] ? "knowledge.continue" : "knowledge.reindex")}</Button>
            {settings?.embedding?.provider_id !== "local-onnx" ? <Button size="sm" variant="secondary" disabled={busy || !source.enabled || !settings?.embedding} onClick={() => source.remote_profile_fingerprint ? void change(source, "revoke_remote") : setGrant(source)}>{t(source.remote_profile_fingerprint ? "knowledge.revokeRemote" : "knowledge.grantRemote")}</Button> : null}
            <Button size="sm" variant="secondary" disabled={busy || !source.enabled || !settings?.embedding || (settings.embedding.provider_id !== "local-onnx" && !source.remote_profile_fingerprint)} onClick={() => void act(async () => {
              if (!client) return;
              await advanceKnowledgeEmbeddings(
                (resetRetry) => client.embedKnowledgeSource(activeBot, source.id, source.revision, resetRetry),
                (result) => setEmbeddingProgress((previous) => ({ ...previous, [source.id]: result }))
              );
            })}>{t("knowledge.embed")}</Button>
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => { setRenaming(source); setRenameLabel(source.label); }}>{t("knowledge.rename")}</Button>
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => setRemoving(source)}>{t("knowledge.remove")}</Button>
          </div>
        </li>)}
      </ul>
      {sources.length === 0 && settings ? <p className={form.hint}>{t("knowledge.empty")}</p> : null}
      {removing ? <section className={styles.review} aria-label={t("knowledge.remove")}>
        <strong>{t("knowledge.removeTitle", { source: removing.label })}</strong>
        <p>{t("knowledge.removeHint")}</p>
        <div className={styles.actions}>
          <Button disabled={busy} onClick={() => void act(async () => { await client!.deleteKnowledgeSource(activeBot, removing.id, removing.revision); setRemoving(null); })}>{t("knowledge.confirmRemove")}</Button>
          <Button variant="secondary" disabled={busy} onClick={() => setRemoving(null)}>{t("common.cancel")}</Button>
        </div>
      </section> : null}
      {renaming ? <section className={styles.review} aria-label={t("knowledge.rename")}>
        <label className={form.field}>{t("knowledge.newName")}<input className={form.input} value={renameLabel} maxLength={200} onChange={(e) => setRenameLabel(e.target.value)} /></label>
        <div className={styles.actions}>
          <Button disabled={busy || !renameLabel.trim()} onClick={() => void act(async () => { await client!.updateKnowledgeSource(activeBot, renaming.id, { action: "rename", expected_revision: renaming.revision, label: renameLabel.trim(), profile_fingerprint: null }); setRenaming(null); })}>{t("knowledge.saveName")}</Button>
          <Button variant="secondary" disabled={busy} onClick={() => setRenaming(null)}>{t("common.cancel")}</Button>
        </div>
      </section> : null}
      {rebinding ? <section className={styles.review} aria-label={t("knowledge.reselect")}>
        <strong>{rebinding.label}</strong><p>{t("knowledge.reselectHint")}</p>
        <label className={form.field}>{t("knowledge.path")}<input className={form.input} value={bindingPath} onChange={(e) => setBindingPath(e.target.value)} /></label>
        <div className={styles.actions}>
          <Button disabled={busy || !bindingPath.trim()} onClick={() => void act(async () => { await client!.updateKnowledgeSource(activeBot, rebinding.id, { action: "rebind", expected_revision: rebinding.revision, label: null, path: bindingPath, profile_fingerprint: null }); setRebinding(null); })}>{t("knowledge.confirmPath")}</Button>
          <Button variant="secondary" disabled={busy} onClick={() => setRebinding(null)}>{t("common.cancel")}</Button>
        </div>
      </section> : null}
      {grant && settings?.embedding ? <section className={styles.review} aria-label={t("knowledge.grantRemote")}>
        <strong>{t("knowledge.consentTitle", { source: grant.label })}</strong>
        <p>{t("knowledge.consentDescription")}</p>
        <code className={styles.locator}>{settings.embedding.endpoint} · {settings.embedding.model}</code>
        <code className={styles.locator}>{settings.embedding.profile_fingerprint}</code>
        <div className={styles.actions}>
          <Button disabled={busy} onClick={() => void change(grant, "grant_remote")}>{t("knowledge.confirmConsent")}</Button>
          <Button variant="secondary" disabled={busy} onClick={() => setGrant(null)}>{t("common.cancel")}</Button>
        </div>
      </section> : null}
      <details className={styles.group}>
        <summary>{t("knowledge.add")}</summary>
        <div className={styles.form}>
          <label className={form.field}>{t("knowledge.kind")}<select className={form.select} value={kind} onChange={(e) => setKind(e.target.value as KnowledgeSourceKind)}>{KINDS.map((item) => <option key={item} value={item}>{t(`knowledge.kinds.${item}`)}</option>)}</select></label>
          <label className={form.field}>{t("knowledge.label")}<input className={form.input} value={label} placeholder={kind === "shared_brain" ? t("knowledge.defaultBrainName") : undefined} maxLength={200} onChange={(e) => setLabel(e.target.value)} /></label>
          {kind === "file" || kind === "folder" ? <label className={form.field}>{t("knowledge.path")}<input className={form.input} value={path} onChange={(e) => setPath(e.target.value)} /><span className={form.hint}>{t("knowledge.pathHint")}</span></label> : null}
          {kind === "saved_web" ? <><label className={form.field}>{t("knowledge.url")}<input className={form.input} type="url" value={url} onChange={(e) => setUrl(e.target.value)} /></label><label><input type="checkbox" checked={html} onChange={(e) => setHtml(e.target.checked)} /> {t("knowledge.html")}</label><p className={form.hint}>{t("knowledge.savedWebHint")}</p></> : null}
          {kind === "saved_web" || kind === "note" ? <label className={form.field}>{t("knowledge.content")}<textarea className={form.textarea} value={content} onChange={(e) => setContent(e.target.value)} /></label> : null}
          {kind === "conversation" ? <label className={form.field}>{t("knowledge.conversation")}<select className={form.select} value={chatId} onChange={(e) => setChatId(e.target.value)}><option value="">{t("knowledge.select")}</option>{chats.map((chat) => <option key={chat.id} value={chat.id}>{chat.title}</option>)}</select></label> : null}
          {kind === "email" ? <><p className={form.hint}>{t("knowledge.emailHint")}</p>{emailError ? <p role="status" className={form.error}>{t("knowledge.emailUnavailable", { error: emailError })}</p> : null}<label className={form.field}>{t("knowledge.emailCapture")}<select className={form.select} value={captureId} disabled={Boolean(emailError)} onChange={(e) => setCaptureId(e.target.value)}><option value="">{t("knowledge.select")}</option>{captures.map((capture) => <option key={capture.tool_call_id} value={capture.tool_call_id}>{capture.account} · {new Date(capture.created_ms).toLocaleString()}</option>)}</select></label></> : null}
          {kind === "shared_brain" ? <p className={form.hint}>{t("knowledge.brainHint")}</p> : null}
          <Button disabled={busy || (kind !== "shared_brain" && !label.trim())} onClick={() => void add()}>{t("knowledge.add")}</Button>
        </div>
      </details>
      {settings ? <EmbeddingSettings key={activeBot + JSON.stringify(settings)} settings={settings} providerIds={providers?.configured.filter((p) => p.enabled && (p.custom || providers.catalog.some((entry) => entry.id === p.id && ["gemini", "openai_compatible"].includes(entry.kind)))).map((p) => p.id) ?? []} busy={busy} save={(value) => act(() => client!.updateKnowledgeSettings(activeBot, value))} /> : null}
    </>}
  </Card>;
}

function EmbeddingSettings({ settings, providerIds, busy, save }: { settings: KnowledgeSettings; providerIds: string[]; busy: boolean; save: (value: KnowledgeSettingsInput) => Promise<void> }) {
  const { t } = useTranslation();
  const current = settings.embedding;
  const [enabled, setEnabled] = useState(Boolean(current));
  const [auto, setAuto] = useState(settings.auto_retrieve);
  const [provider, setProvider] = useState(current?.provider_id ?? providerIds[0] ?? "");
  const [model, setModel] = useState(current?.model ?? "gemini-embedding-001");
  const [dimensions, setDimensions] = useState(current?.dimensions ?? 768);
  const [requestDimensions, setRequestDimensions] = useState(current?.request_dimensions ?? true);
  const [rpm, setRpm] = useState(current?.requests_per_minute ?? 100);
  const [rpd, setRpd] = useState(current?.requests_per_day ?? 1000);
  const [query, setQuery] = useState(current?.query_remote ?? false);
  const [manifest, setManifest] = useState(JSON.stringify(current?.local_artifacts ?? Object.fromEntries(["runtime", "model", "tokenizer", "config", "special_tokens_map", "tokenizer_config"].map((name) => [name, { path: "", sha256: "" }])), null, 2));
  const local = provider === "local-onnx";
  const artifacts = parseArtifacts(manifest);
  const reviewed = Boolean(current && current.provider_id === provider && current.model === model && current.dimensions === dimensions && current.request_dimensions === requestDimensions);
  return <details className={styles.group}>
    <summary>{t("knowledge.settings")}</summary>
    <div className={styles.form}>
      <label><input type="checkbox" checked={auto} onChange={(e) => setAuto(e.target.checked)} /> {t("knowledge.auto")}</label>
      <label><input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} /> {t("knowledge.embeddings")}</label>
      <p className={form.hint}>{t("knowledge.lexicalHint")}</p>
      {enabled ? <>
        <label className={form.field}>{t("knowledge.provider")}<select className={form.select} value={provider} onChange={(e) => { setProvider(e.target.value); setQuery(false); }}><option value="">{t("knowledge.select")}</option>{providerIds.map((id) => <option key={id} value={id}>{id}</option>)}<option value="local-onnx">{t("knowledge.local")}</option></select></label>
        {local ? <>
          <p className={form.hint}>{t("knowledge.localHint")}</p>
          {!settings.local_embeddings_available ? <p role="status" className={form.hint}>{t("knowledge.localUnavailable")}</p> : null}
          <label className={form.field}>{t("knowledge.artifacts")}<textarea className={form.textarea} rows={18} spellCheck={false} value={manifest} onChange={(e) => setManifest(e.target.value)} /></label>
          {!artifacts ? <p className={form.hint}>{t("knowledge.artifactsInvalid")}</p> : null}
        </> : <><label className={form.field}>{t("knowledge.model")}<input className={form.input} value={model} onChange={(e) => { setModel(e.target.value); setQuery(false); }} /></label>
        <label className={form.field}>{t("knowledge.dimensions")}<input className={form.input} type="number" min={1} max={4096} value={dimensions} onChange={(e) => { setDimensions(Number(e.target.value)); setQuery(false); }} /></label>
        <label><input type="checkbox" checked={requestDimensions} onChange={(e) => { setRequestDimensions(e.target.checked); setQuery(false); }} /> {t("knowledge.requestDimensions")}</label>
        <label className={form.field}>{t("knowledge.rpm")}<input className={form.input} type="number" min={1} value={rpm} onChange={(e) => setRpm(Number(e.target.value))} /></label>
        <label className={form.field}>{t("knowledge.rpd")}<input className={form.input} type="number" min={1} value={rpd} onChange={(e) => setRpd(Number(e.target.value))} /></label>
        <p className={form.hint}>{t("knowledge.quotaHint")}</p>
        {current ? <code className={styles.locator}>{current.endpoint} · {current.profile_fingerprint}</code> : null}
        <label><input type="checkbox" disabled={!reviewed} checked={query && reviewed} onChange={(e) => setQuery(e.target.checked)} /> {t("knowledge.queryConsent")}</label>
        <p className={form.hint}>{t("knowledge.queryHint")}</p>
        </>}
      </> : null}
      <Button disabled={busy || (enabled && (!provider || (local ? !artifacts : !model.trim())))} onClick={() => void save({ auto_retrieve: auto, embedding: enabled ? { provider_id: provider, model: local ? "multilingual-e5-small" : model, dimensions: local ? 384 : dimensions, request_dimensions: local ? false : requestDimensions, requests_per_minute: rpm, requests_per_day: rpd, query_remote: !local && query && reviewed, consent_profile_fingerprint: !local && query && reviewed ? current?.profile_fingerprint ?? null : null, local_artifacts: local ? artifacts : null } : null })}>{t("knowledge.save")}</Button>
    </div>
  </details>;
}

function parseArtifacts(raw: string): KnowledgeLocalArtifacts | null {
  try {
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object") return null;
    const record = value as Record<string, unknown>;
    for (const key of ["runtime", "model", "tokenizer", "config", "special_tokens_map", "tokenizer_config"]) {
      const artifact = record[key];
      if (!artifact || typeof artifact !== "object") return null;
      const fields = artifact as Record<string, unknown>;
      if (typeof fields.path !== "string" || !fields.path.trim() || typeof fields.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(fields.sha256)) return null;
    }
    return value as KnowledgeLocalArtifacts;
  } catch { return null; }
}
