import {
  Check,
  CircleCheck,
  CircleDashed,
  CircleX,
  Copy,
  Loader2,
  Monitor,
  Server,
  ShieldQuestion,
  TriangleAlert
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { ApiError } from "../api/client";
import { isMockMode } from "../api/mockMode";
import type { HostSetupJob, SshKeyHint, StartHostSetupInput } from "../api/types";
import {
  afterKeyAnswer,
  copyDetails,
  failure,
  isActive,
  STAGES,
  stageStates,
  type StageState,
  wizardView
} from "../lib/hostSetup";
import { useDaemon } from "../state/DaemonContext";
import styles from "./AddServerModal.module.css";
import dialog from "./Dialog.module.css";
import { Callout, DialogFooter, DialogHeader, Field } from "./DialogParts";
import form from "./Form.module.css";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

const POLL_MS = 800;

/** The running job survives this dialog being rebuilt (the app re-renders around it). */
let resumeJobId: string | null = null;

/**
 * "Add server": connect a rented server over SSH and pair it, with no ticket to
 * copy. The SSH work runs in this computer's daemon; this modal only collects the
 * address, shows honest progress and asks the one question that must be a human's
 * (is this the right server? the host key fingerprint on first use).
 */
export function AddServerModal({
  open,
  onClose,
  onManual
}: {
  open: boolean;
  onClose: () => void;
  /** Open the old "paste a ticket" dialog instead. */
  onManual: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Modal open={open} onClose={onClose} size="md" label={t("addServer.title")}>
      {open ? <Content onClose={onClose} onManual={onManual} /> : null}
    </Modal>
  );
}

type Auth = "key" | "agent";

function Content({ onClose, onManual }: { onClose: () => void; onManual: () => void }) {
  const { t } = useTranslation();
  const toast = useToast();
  const { client, localApi, refreshHosts, selectComputer } = useDaemon();
  // The wizard always talks to THIS computer's daemon, never to a selected host.
  const api = isMockMode() ? client : localApi;

  const [host, setHost] = useState("");
  const [user, setUser] = useState("ubuntu");
  const [port, setPort] = useState("22");
  const [auth, setAuth] = useState<Auth>("key");
  const [keyPath, setKeyPath] = useState("");
  const [keys, setKeys] = useState<SshKeyHint[]>([]);
  const [job, setJob] = useState<HostSetupJob | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const input = useRef<StartHostSetupInput | null>(null);
  const jobId = job?.id ?? null;
  const active = job ? isActive(job) : false;
  resumeJobId = jobId && active ? jobId : jobId ? null : resumeJobId;

  // Whatever happens to the job, the form is never left in "starting".
  useEffect(() => {
    if (job) setBusy(false);
  }, [job]);

  // Picked up again after a rebuild: show the job that is still waiting or running.
  useEffect(() => {
    const id = resumeJobId;
    if (!api || !id) return undefined;
    let live = true;
    api
      .getHostSetup(id)
      .then((j) => {
        if (live) setJob(j);
      })
      .catch(() => {
        resumeJobId = null;
      });
    return () => {
      live = false;
    };
  }, [api]);

  useEffect(() => {
    let live = true;
    api
      ?.listSshKeys()
      .then((k) => {
        if (live) {
          setKeys(k);
          setKeyPath((p) => p || k[0]?.path || "");
        }
      })
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [api]);

  // Poll while the job is active.
  useEffect(() => {
    if (!api || !jobId || !active) {
      return undefined;
    }
    let live = true;
    const id = window.setInterval(() => {
      api
        .getHostSetup(jobId)
        .then((j) => {
          if (live) setJob(j);
        })
        .catch(() => undefined);
    }, POLL_MS);
    return () => {
      live = false;
      window.clearInterval(id);
    };
  }, [api, jobId, active]);

  // A finished setup: the new server shows up in the computer list.
  const succeeded = job?.state === "succeeded";
  useEffect(() => {
    if (succeeded) void refreshHosts();
  }, [succeeded, refreshHosts]);

  const start = useCallback(
    async (body: StartHostSetupInput) => {
      if (!api) {
        setError(t("addServer.noDaemon"));
        return;
      }
      setBusy(true);
      setError(null);
      try {
        input.current = body;
        setJob(await api.startHostSetup(body));
      } catch (e) {
        setError(e instanceof ApiError ? e.message : t("addServer.startFailed"));
      } finally {
        setBusy(false);
      }
    },
    [api, t]
  );

  const submit = () => {
    const portNumber = Number(port);
    if (!host.trim() || !Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535) {
      setError(t("addServer.formInvalid"));
      return;
    }
    if (auth === "key" && !keyPath.trim()) {
      setError(t("addServer.keyMissing"));
      return;
    }
    void start({
      host: host.trim(),
      user: user.trim() || "ubuntu",
      port: portNumber,
      key_path: auth === "key" ? keyPath.trim() : null
    });
  };

  const answerKey = (accept: boolean) => {
    if (!api || !jobId) return;
    setError(null);
    // Show the answer at once; the poll confirms it.
    setJob((j) => (j ? afterKeyAnswer(j, accept) : j));
    api
      .answerHostSetupKey(jobId, accept)
      .then(setJob)
      .catch(() => {
        // The answer did not arrive (the job is gone or no longer waiting): say so
        // on this page and show the job as the daemon has it.
        setError(t("addServer.answerFailed"));
        api
          .getHostSetup(jobId)
          .then(setJob)
          .catch(() => setJob(null));
      });
  };

  const cancelAndClose = () => {
    resumeJobId = null;
    if (api && jobId && active) {
      void api.cancelHostSetup(jobId).catch(() => undefined);
    }
    onClose();
  };

  const view = wizardView(job);
  const hasScreen = (job?.view_targets ?? 0) > 0;
  return (
    <div className={dialog.root} data-testid="add-server" data-view={view}>
      <DialogHeader
        title={t("addServer.title")}
        subtitle={job ? job.target : t("addServer.lead")}
        leading={<Server aria-hidden="true" className={styles.headIcon} />}
        onClose={cancelAndClose}
      />
      {view === "form" ? (
        <>
          <div className={dialog.body}>
            <div className={styles.row}>
              <Field label={t("addServer.host")} htmlFor="as-host">
                <input
                  id="as-host"
                  className={form.input}
                  value={host}
                  placeholder="203.0.113.7"
                  autoComplete="off"
                  spellCheck={false}
                  data-autofocus
                  data-testid="add-server-host"
                  onChange={(e) => setHost(e.target.value)}
                />
              </Field>
              <Field label={t("addServer.user")} htmlFor="as-user">
                <input
                  id="as-user"
                  className={form.input}
                  value={user}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(e) => setUser(e.target.value)}
                />
              </Field>
              <Field label={t("addServer.port")} htmlFor="as-port">
                <input
                  id="as-port"
                  className={form.input}
                  value={port}
                  inputMode="numeric"
                  autoComplete="off"
                  onChange={(e) => setPort(e.target.value)}
                />
              </Field>
            </div>
            <Field label={t("addServer.auth")}>
              <div className={styles.seg} role="radiogroup" aria-label={t("addServer.auth")}>
                {(["key", "agent"] as const).map((a) => (
                  <button
                    key={a}
                    type="button"
                    role="radio"
                    aria-checked={auth === a}
                    className={styles.segItem}
                    data-on={auth === a}
                    onClick={() => setAuth(a)}
                  >
                    {t(`addServer.auth_${a}`)}
                  </button>
                ))}
              </div>
            </Field>
            {auth === "key" ? (
              <Field label={t("addServer.keyFile")} htmlFor="as-key" hint={t("addServer.keyHint")}>
                {keys.length > 0 ? (
                  <div className={styles.keys} role="radiogroup" aria-label={t("addServer.keyFile")}>
                    {keys.map((k) => (
                      <button
                        key={k.path}
                        type="button"
                        role="radio"
                        aria-checked={keyPath === k.path}
                        className={styles.keyItem}
                        data-on={keyPath === k.path}
                        title={k.path}
                        onClick={() => setKeyPath(k.path)}
                      >
                        {keyPath === k.path ? <Check aria-hidden="true" /> : null}
                        {k.name}
                      </button>
                    ))}
                  </div>
                ) : null}
                <input
                  id="as-key"
                  className={`${form.input} ${styles.mono}`}
                  value={keyPath}
                  placeholder="~/.ssh/id_ed25519"
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(e) => setKeyPath(e.target.value)}
                />
              </Field>
            ) : (
              <Callout tone="info">{t("addServer.agentHint")}</Callout>
            )}
            <Callout tone="info">{t("addServer.releaseNote")}</Callout>
            {error ? (
              <p className={form.error} role="alert">
                {error}
              </p>
            ) : null}
          </div>
          <DialogFooter
            note={
              <button type="button" className={styles.link} onClick={onManual}>
                {t("addServer.manual")}
              </button>
            }
          >
            <Button variant="ghost" onClick={onClose}>
              {t("common.cancel")}
            </Button>
            <Button
              variant="primary"
              disabled={busy || !host.trim()}
              onClick={submit}
              data-testid="add-server-start"
            >
              {busy ? t("addServer.starting") : t("addServer.connect")}
            </Button>
          </DialogFooter>
        </>
      ) : null}
      {job && view !== "form" ? (
        <>
          <div className={dialog.body}>
            <Progress job={job} />
            {job.state === "awaiting_host_key" && job.host_key ? (
              <div
                className={styles.keyCard}
                role="group"
                aria-label={t("addServer.hostKeyTitle")}
              >
                <p className={styles.keyTitle}>{t("addServer.hostKeyTitle")}</p>
                <p className={styles.keyText}>
                  {t("addServer.hostKeyBody", { host: job.host_key.host })}
                </p>
                <code className={styles.fingerprint} data-testid="host-key-fingerprint">
                  {job.host_key.fingerprint}
                </code>
                <div className={styles.keyActions}>
                  <Button variant="ghost" onClick={() => answerKey(false)}>
                    {t("addServer.hostKeyNo")}
                  </Button>
                  <Button
                    variant="primary"
                    onClick={() => answerKey(true)}
                    data-testid="host-key-trust"
                  >
                    {t("addServer.hostKeyYes")}
                  </Button>
                </div>
              </div>
            ) : null}
            {error ? (
              <p className={form.error} role="alert">
                {error}
              </p>
            ) : null}
            {view === "failed" ? <Failure job={job} /> : null}
            {view === "done" ? (
              <Callout
                tone="success"
                title={t("addServer.doneTitle", { name: job.host_name ?? job.target })}
              >
                {hasScreen ? t("addServer.doneScreen") : t("addServer.doneNoScreen")}
              </Callout>
            ) : null}
          </div>
          <DialogFooter
            note={
              view === "progress" && job.state === "running" ? t("addServer.cancelNote") : undefined
            }
          >
            {view === "progress" ? (
              <Button variant="ghost" onClick={cancelAndClose} data-testid="add-server-cancel">
                {t("common.cancel")}
              </Button>
            ) : null}
            {view === "failed" ? (
              <>
                <Button
                  variant="ghost"
                  onClick={() => {
                    void navigator.clipboard
                      ?.writeText(copyDetails(job))
                      .then(() => toast.show(t("addServer.copied"), "success"))
                      .catch(() => toast.show(t("addServer.copyFailed"), "danger"));
                  }}
                  data-testid="add-server-copy"
                >
                  <Copy aria-hidden="true" />
                  {t("addServer.copy")}
                </Button>
                <Button variant="ghost" onClick={() => setJob(null)}>
                  {t("addServer.edit")}
                </Button>
                <Button
                  variant="primary"
                  disabled={busy || !input.current}
                  onClick={() => input.current && void start(input.current)}
                  data-testid="add-server-retry"
                >
                  {t("addServer.retry")}
                </Button>
              </>
            ) : null}
            {view === "done" ? (
              <>
                <Button variant="ghost" onClick={onClose}>
                  {t("common.close")}
                </Button>
                <Button
                  variant="primary"
                  disabled={!job.host_id}
                  onClick={() => {
                    if (job.host_id) void selectComputer(job.host_id);
                    onClose();
                  }}
                  data-testid="add-server-use"
                >
                  {hasScreen ? <Monitor aria-hidden="true" /> : <Server aria-hidden="true" />}
                  {hasScreen ? t("addServer.viewScreen") : t("addServer.useServer")}
                </Button>
              </>
            ) : null}
          </DialogFooter>
        </>
      ) : null}
    </div>
  );
}

function StateIcon({ state }: { state: StageState }) {
  switch (state) {
    case "running":
      return <Loader2 aria-hidden="true" className={styles.spin} />;
    case "waiting":
      return <ShieldQuestion aria-hidden="true" />;
    case "done":
    case "skipped":
      return <CircleCheck aria-hidden="true" />;
    case "warn":
      return <TriangleAlert aria-hidden="true" />;
    case "failed":
      return <CircleX aria-hidden="true" />;
    default:
      return <CircleDashed aria-hidden="true" />;
  }
}

function Progress({ job }: { job: HostSetupJob }) {
  const { t } = useTranslation();
  const states = stageStates(job);
  return (
    <ol className={styles.stages} aria-label={t("addServer.progress")}>
      {STAGES.map((stage) => (
        <li
          key={stage}
          className={styles.stage}
          data-state={states[stage]}
          data-testid={`stage-${stage}`}
        >
          <span className={styles.stageIcon}>
            <StateIcon state={states[stage]} />
          </span>
          <span className={styles.stageText}>
            <b>{t(`addServer.stage.${stage}`)}</b>
            <small>{t(`addServer.state.${states[stage]}`)}</small>
          </span>
        </li>
      ))}
    </ol>
  );
}

function Failure({ job }: { job: HostSetupJob }) {
  const { t } = useTranslation();
  const f = failure(job);
  if (!f) {
    return (
      <Callout tone="warning" title={t("addServer.cancelled")} role="status">
        {t("addServer.cancelledBody")}
      </Callout>
    );
  }
  return (
    <Callout tone="danger" title={t(`addServer.fail.${f.code}.msg`)} role="alert">
      {t(`addServer.fail.${f.code}.fix`)}
    </Callout>
  );
}
