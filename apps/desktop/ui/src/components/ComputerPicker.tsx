import { Check, ChevronDown, Monitor, Plus, Server, Square } from "lucide-react";
import { useCallback, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { hostAcceptsWork, type LinkState } from "../lib/hostState";
import { LOCAL_COMPUTER, useDaemon } from "../state/DaemonContext";
import styles from "./ComputerPicker.module.css";
import { AddServerModal } from "./AddServerModal";
import { PairHostModal } from "./PairHostModal";
import { Popover } from "./Popover";
import { useToast } from "./Toast";

function dotTone(state: LinkState | undefined): "ok" | "wait" | "bad" | "idle" {
  switch (state) {
    case "connected":
      return "ok";
    case "connecting":
      return "wait";
    case "offline":
    case "unreachable":
      return "bad";
    default:
      return "idle";
  }
}

/**
 * Header selector: "this computer" or a paired host. Selecting a host switches the
 * whole UI (bots, chats, events, live view) to that daemon through the paired channel.
 */
export function ComputerPicker() {
  const { t } = useTranslation();
  const toast = useToast();
  const {
    computers,
    activeComputerId,
    activeHostName,
    hostState,
    selectComputer,
    stopHostRuns
  } = useDaemon();
  const [open, setOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [pairOpen, setPairOpen] = useState(false);
  const [stopping, setStopping] = useState(false);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const close = useCallback(() => setOpen(false), []);
  const onHost = activeComputerId !== LOCAL_COMPUTER;
  const label = onHost
    ? (activeHostName ?? t("computerPicker.host"))
    : t("account.localComputer");
  const tone = onHost
    ? hostState?.kind === "connected" || hostState?.kind === "no_model_key"
      ? "ok"
      : hostState?.kind === "connecting"
        ? "wait"
        : hostState?.kind === "version_mismatch" && !hostState.blocking
          ? "wait"
          : "bad"
    : "ok";
  const Icon = onHost ? Server : Monitor;

  const choose = (id: string) => {
    setOpen(false);
    void selectComputer(id);
  };
  const stop = async () => {
    setOpen(false);
    setStopping(true);
    try {
      const n = await stopHostRuns();
      toast.show(
        n === 0 ? t("computerPicker.stopNone") : t("computerPicker.stopped", { count: n }),
        n === 0 ? "info" : "success"
      );
    } catch {
      toast.show(t("computerPicker.stopFailed"), "danger");
    } finally {
      setStopping(false);
    }
  };

  return (
    <>
      <button
        ref={trigger}
        type="button"
        className={styles.trigger}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={t("computerPicker.label", { name: label })}
        title={t("computerPicker.label", { name: label })}
        data-testid="computer-picker"
        data-computer={onHost ? "host" : "local"}
        onClick={() => setOpen((o) => !o)}
      >
        <i className={styles.dot} data-tone={tone} aria-hidden="true" />
        <Icon aria-hidden="true" className={styles.icon} />
        <span className={styles.name}>{label}</span>
        <ChevronDown aria-hidden="true" className={styles.chevron} />
      </button>
      <Popover
        open={open}
        anchor={trigger}
        onClose={close}
        placement="bottom-end"
        label={t("computerPicker.menu")}
        width={288}
        className={styles.menu}
      >
        <p className={styles.heading}>{t("computerPicker.menu")}</p>
        {computers.map((c) => {
          const active = c.id === activeComputerId;
          const name = c.kind === "local" ? t("account.localComputer") : c.name;
          const state = c.link?.state;
          return (
            <button
              key={c.id}
              type="button"
              role="menuitemradio"
              aria-checked={active}
              className={styles.item}
              data-testid={`computer-${c.kind === "local" ? "local" : "host"}`}
              onClick={() => choose(c.id)}
            >
              <i
                className={styles.dot}
                data-tone={c.kind === "local" ? "ok" : dotTone(state)}
                aria-hidden="true"
              />
              {c.kind === "local" ? (
                <Monitor aria-hidden="true" className={styles.icon} />
              ) : (
                <Server aria-hidden="true" className={styles.icon} />
              )}
              <span className={styles.itemText}>
                <b>{name}</b>
                <small>
                  {c.kind === "local"
                    ? t("computerPicker.localHint")
                    : t(`computerPicker.link.${state ?? "idle"}`)}
                </small>
              </span>
              {active ? <Check aria-hidden="true" className={styles.check} /> : null}
            </button>
          );
        })}
        <button
          type="button"
          role="menuitem"
          className={styles.item}
          data-testid="computer-add"
          onClick={() => {
            setOpen(false);
            setAddOpen(true);
          }}
        >
          <Plus aria-hidden="true" className={styles.icon} />
          <span className={styles.itemText}>
            <b>{t("computerPicker.add")}</b>
          </span>
        </button>
        {onHost && hostState && hostAcceptsWork(hostState) ? (
          <button
            type="button"
            role="menuitem"
            className={`${styles.item} ${styles.danger}`}
            data-testid="computer-stop-host"
            disabled={stopping}
            onClick={() => void stop()}
          >
            <Square aria-hidden="true" className={styles.icon} />
            <span className={styles.itemText}>
              <b>{t("computerPicker.stopHost")}</b>
              <small>{t("computerPicker.stopHostHint")}</small>
            </span>
          </button>
        ) : null}
      </Popover>
      <AddServerModal
        open={addOpen}
        onClose={() => setAddOpen(false)}
        onManual={() => {
          setAddOpen(false);
          setPairOpen(true);
        }}
      />
      <PairHostModal open={pairOpen} onClose={() => setPairOpen(false)} />
    </>
  );
}
