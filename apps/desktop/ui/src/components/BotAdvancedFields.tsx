import { type ChangeEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import type { ApiClient } from "../api/client";
import type { McpServer, ProvidersDto } from "../api/types";
import { avatarFromFile, AvatarFileError } from "../lib/avatar";
import { botHasAccess } from "../lib/mcpApps";
import formStyles from "./Form.module.css";
import styles from "./BotAdvancedFields.module.css";

/** Load provider catalog and per-app access snapshot when a bot dialog/form opens. */
export function useBotFormDeps(
  client: ApiClient | null,
  existingId: string | undefined,
  active: boolean
) {
  const [servers, setServers] = useState<McpServer[] | null>(null);
  const [allowed, setAllowed] = useState<Record<string, boolean>>({});
  const [initialApps, setInitialApps] = useState<Record<string, boolean>>({});
  const [providersDto, setProvidersDto] = useState<ProvidersDto | null>(null);

  useEffect(() => {
    if (!client || !active) {
      return undefined;
    }
    let cancelled = false;
    client
      .providers()
      .then((dto) => {
        if (!cancelled) {
          setProvidersDto(dto);
        }
      })
      .catch(() => {});
    client
      .listMcpServers()
      .then((list) => {
        if (!cancelled) {
          const snapshot = Object.fromEntries(
            list.map((s) => [s.name, botHasAccess(s, existingId)])
          );
          setServers(list);
          setAllowed(snapshot);
          setInitialApps(snapshot);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setServers([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [client, existingId, active]);

  return {
    providersDto,
    servers,
    setServers,
    allowed,
    setAllowed,
    initialApps,
    setInitialApps
  };
}

/** Custom uploaded avatar file control (PNG, JPEG, WebP, GIF) with reset back to 3D character. */
export function BotAvatarUploadField({
  avatar,
  onChange
}: {
  avatar: string | null;
  onChange: (avatar: string | null) => void;
}) {
  const { t } = useTranslation();
  const fileRef = useRef<HTMLInputElement | null>(null);
  const [avatarError, setAvatarError] = useState<string | null>(null);

  const onFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) {
      return;
    }
    setAvatarError(null);
    try {
      onChange(await avatarFromFile(file));
    } catch (err) {
      setAvatarError(
        err instanceof AvatarFileError && err.reason === "tooLarge"
          ? t("botForm.avatarTooLarge")
          : t("botForm.avatarUnreadable")
      );
    }
  };

  return (
    <div className={formStyles.field}>
      <span className={formStyles.label}>{t("botForm.avatar")}</span>
      <div className={formStyles.row}>
        <input
          ref={fileRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          className={styles.hiddenFile}
          onChange={(e) => void onFile(e)}
        />
        <Button size="sm" variant="secondary" onClick={() => fileRef.current?.click()}>
          {t("botForm.avatarChoose")}
        </Button>
        {avatar ? (
          <Button size="sm" variant="ghost" onClick={() => onChange(null)}>
            {t("botForm.avatarUseInitials")}
          </Button>
        ) : (
          <span className={formStyles.hint}>{t("botForm.avatarInitialsHint")}</span>
        )}
      </div>
      {avatarError ? (
        <p className={formStyles.error}>{avatarError}</p>
      ) : (
        <p className={formStyles.hint}>{t("botForm.avatarHint")}</p>
      )}
    </div>
  );
}
