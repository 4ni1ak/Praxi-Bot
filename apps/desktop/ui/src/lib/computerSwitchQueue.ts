import type { ComputerSwitches } from "../api/client.ts";

type Send = (id: string, next: ComputerSwitches) => Promise<void>;

/**
 * Settings > Computer sends the WHOLE switch set of a server on every change. Built from the
 * render-time state, two quick toggles could send a just-revoked family as on again (or the
 * reverse). This queue sends changes one after the other, each built from the last state it sent
 * (or the server's state when nothing is pending), so a later change never undoes an earlier one.
 */
export class SwitchQueue {
  private readonly pending = new Map<string, { state: ComputerSwitches; count: number }>();
  private chain: Promise<void> = Promise.resolve();
  private readonly send: Send;
  private readonly onError: (e: unknown) => void;
  private readonly onChange: () => void;

  constructor(send: Send, onError: (e: unknown) => void, onChange: () => void = () => {}) {
    this.send = send;
    this.onError = onError;
    this.onChange = onChange;
  }

  /** What the switches show: the last state sent while one is pending, else the server's. */
  view(id: string, server: ComputerSwitches): ComputerSwitches {
    return this.pending.get(id)?.state ?? server;
  }

  /** Apply `patch` on top of the latest state and send the result after every earlier change. */
  change(id: string, server: ComputerSwitches, patch: Partial<ComputerSwitches>): Promise<void> {
    const next = { ...this.view(id, server), ...patch };
    this.pending.set(id, { state: next, count: (this.pending.get(id)?.count ?? 0) + 1 });
    this.onChange();
    this.chain = this.chain
      .then(() => this.send(id, next))
      .catch((e: unknown) => this.onError(e))
      .finally(() => {
        const entry = this.pending.get(id);
        if (entry) {
          entry.count -= 1;
          if (entry.count <= 0) this.pending.delete(id);
        }
        this.onChange();
      });
    return this.chain;
  }
}
