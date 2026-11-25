import { invoke } from "@tauri-apps/api/core";
import { isMockMode } from "../api/mockMode";
import { isTauriRuntime } from "../overlay/events";
export { updateErrorKey } from "./updateErrors";

/** What the desktop shell says about updates (see apps/desktop/src/update.rs). */
export interface UpdateStatus {
  enabled: boolean;
  currentVersion: string;
  buildId: string;
  channel: UpdateChannel;
  previewAvailable: boolean;
  lastChecked: number | null;
  skippedVersion: string | null;
  installedNotes: UpdateInfo | null;
  previousVersion: string | null;
  rollbackAdvised: boolean;
  rollbackAvailable: boolean;
}

export type UpdateChannel = "stable" | "preview";

export interface UpdateInfo {
  version: string;
  /** Plain text in the user's language when the release has it; never HTML. */
  notes: string;
  date: string | null;
}

const OFF: UpdateStatus = {
  enabled: false,
  currentVersion: "",
  buildId: "",
  channel: "stable",
  previewAvailable: false,
  lastChecked: null,
  skippedVersion: null,
  installedNotes: null,
  previousVersion: null,
  rollbackAdvised: false,
  rollbackAvailable: false
};

const inApp = () => isTauriRuntime() && !isMockMode();

export async function updateStatus(lang = "en"): Promise<UpdateStatus> {
  if (!inApp()) {
    return OFF;
  }
  return invoke<UpdateStatus>("update_status", { lang });
}

export function setUpdateChannel(channel: UpdateChannel): Promise<void> {
  return invoke("update_set_channel", { channel });
}

export function skipUpdate(): Promise<void> {
  return invoke("update_skip");
}

/** `lang` is the UI language ("tr", "de", "en"); the shell picks the matching notes. */
export function checkForUpdate(lang: string): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>("update_check", { lang });
}

/** Downloads, verifies the signature and installs; the app restarts on Windows. */
export function installUpdate(): Promise<void> {
  return invoke("update_install");
}

export function rollbackUpdate(): Promise<void> {
  return invoke("update_rollback");
}
