import os
os.chdir("src")
def sub(p, a, b):
    s = open(p, encoding="utf-8").read()
    assert s.count(a) == 1, (p, a, s.count(a))
    open(p, "w", encoding="utf-8").write(s.replace(a, b))

sub("App.tsx", 'import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";',
    'import { Fragment, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";')

sub("api/client.ts", """  /** Aborts every request of this client when its computer is dropped (`dispose`). */""", """  /** Name of the paired host this client talks to (null for this computer). */
  hostName: string | null = null;
  /** Turns a host refusal key into a sentence (installed by the app with i18n; tests leave it). */
  static refusalText: ((key: string, name: string) => string) | null = null;
  /** Aborts every request of this client when its computer is dropped (`dispose`). */""")
sub("api/client.ts", """      throw new ApiError(res.status, code, message, parsed);""", """      // A host's refusal is shown as one human sentence naming the server, on every screen.
      if (this.hostName !== null && ApiClient.refusalText) {
        const key = hostRefusalKey(res.status, message);
        if (key) {
          message = ApiClient.refusalText(key, this.hostName);
        }
      }
      throw new ApiError(res.status, code, message, parsed);""")
sub("api/client.ts", "/** A request that gets no answer", 'import { hostRefusalKey } from "../lib/hostMode.ts";\n\n/** A request that gets no answer')
sub("state/DaemonContext.tsx", """  const clientRef = useRef(data.client);
  clientRef.current = data.client;""", """  const clientRef = useRef(data.client);
  clientRef.current = data.client;
  if (data.client) {
    data.client.hostName = link?.name ?? local.activeHostName ?? "";
  }""")
