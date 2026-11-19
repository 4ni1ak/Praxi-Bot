import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "ui";
import { useDaemon } from "../state/DaemonContext";
import { ownerKey } from "../lib/ownerKey";
import { humanLine } from "../lib/humanError";

export function TrustedFolders() {
  const { t } = useTranslation();
  const { localApi } = useDaemon();
  const [folders, setFolders] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (!localApi) return;
    let live = true;
    void localApi.trustedFolders().then((dto) => { if (live) setFolders(dto.folders); })
      .catch((e: unknown) => { if (live) setError(humanLine(e)); });
    return () => { live = false; };
  }, [localApi]);
  return <div>
    <p>{t("folderTrust.description")}</p>
    {folders.length === 0 ? <p>{t("folderTrust.empty")}</p> : null}
    {folders.map((folder) => <div key={folder}>
      <code>{folder}</code>{" "}
      <Button size="sm" variant="secondary" disabled={busy} onClick={() => {
        if (!localApi) return;
        setBusy(true);
        setError(null);
        void ownerKey(localApi.baseUrl).then((key) => {
          if (!key) throw new Error(t("folderTrust.ownerRequired"));
          return localApi.setFolderTrust(folder, false, key);
        }).then((dto) => setFolders(dto.folders))
          .catch((e: unknown) => setError(humanLine(e))).finally(() => setBusy(false));
      }}>{t("folderTrust.remove", { folder })}</Button>
    </div>)}
    {error ? <p role="alert">{error}</p> : null}
  </div>;
}
