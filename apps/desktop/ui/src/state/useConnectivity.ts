import { useEffect, useState } from "react";
import { useDaemon } from "./DaemonContext";
import { connectionState, type ConnectivityReport, type ConnectionState } from "../lib/connectivity";
import { ApiError } from "../api/client";

/** Poll only while mounted; responses of a previously selected host never apply here. */
export function useConnectivity() {
  const { client } = useDaemon();
  const [snapshot, setSnapshot] = useState<{ client: typeof client; report: ConnectivityReport | null; state: ConnectionState }>({ client: null, report: null, state: "unknown" });
  useEffect(() => {
    if (!client) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    const check = async () => {
      try {
        const report = await client.connectivity();
        if (live) setSnapshot({ client, report, state: connectionState(report) });
      } catch (e) {
        // Older hosts and owner refusals are unknown; do not queue indefinitely on a 403/404.
        const network = e instanceof ApiError ? e.code === "network" || e.status === 502 || e.status === 504 : e instanceof TypeError;
        if (live) setSnapshot({ client, report: null, state: network ? "host_unreachable" : "unknown" });
      }
      if (live) timer = setTimeout(() => void check(), 20_000);
    };
    void check();
    return () => { live = false; clearTimeout(timer); };
  }, [client]);
  return snapshot.client === client ? snapshot : { report: null, state: "unknown" as const };
}
