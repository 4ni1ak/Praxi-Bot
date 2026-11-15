import type { ControlStateDto } from "../api/types.ts";

/**
 * The takeover half of a Stop (card 237, review r-94 #2 / #9). Pure; injected parts.
 *
 * - `stop()` SYNCHRONOUSLY revokes this app's takeover input: from that call on no
 *   pointer / key event is queued or sent and the lease heartbeat stops renewing,
 *   whatever the daemon answers, until the user takes control again (`resume()`).
 * - The `POST /v1/control/override` it owes is retained until it is delivered through
 *   the CURRENT client: with no client yet it waits for one; a failure is retried; a
 *   reply that arrives after the client was replaced is ignored and the override is
 *   sent again through the new client.
 * - "Only if controlled" Stops decide from a control snapshot of the current client;
 *   while there is none the decision waits (unknown is not "not controlled").
 */

export const OVERRIDE_RETRY_MS = 2000;

export interface TakeoverStopDeps<C> {
  /** The daemon client that is current right now (or null). */
  client(): C | null;
  /**
   * Whether an acquiring / active takeover exists according to a snapshot read from
   * the CURRENT client; `null` when no such snapshot exists yet.
   */
  controlled(): boolean | null;
  post(client: C): Promise<ControlStateDto>;
  /** Apply a delivered override reply (only called while `client` is still current). */
  apply(client: C, dto: ControlStateDto): void;
  schedule(fn: () => void, ms: number): unknown;
}

export class TakeoverStop<C> {
  private readonly deps: TakeoverStopDeps<C>;
  private halted = false;
  private owed: { force: boolean } | null = null;
  private inFlight = false;
  private retryPending = false;

  constructor(deps: TakeoverStopDeps<C>) {
    this.deps = deps;
  }

  /** A Stop decision. `force`: send the override even if no takeover is held (viewer Stop). */
  stop(force: boolean): void {
    this.halted = true;
    this.owed = { force: (this.owed?.force ?? false) || force };
    this.pump();
  }

  /** The user took control again (a new decision): input may flow again. */
  resume(): void {
    this.halted = false;
  }

  /** May this app send takeover input / renew the input lease right now? */
  inputAllowed(): boolean {
    return !this.halted;
  }

  /** An override is still owed (not yet delivered through the current client). */
  owes(): boolean {
    return this.owed !== null;
  }

  /** Try to deliver the owed override (call on Stop, client change and new snapshots). */
  pump(): void {
    const owed = this.owed;
    if (!owed || this.inFlight) {
      return;
    }
    const client = this.deps.client();
    if (!client) {
      return; // retained until a client exists
    }
    if (!owed.force) {
      const controlled = this.deps.controlled();
      if (controlled === null) {
        return; // unknown: wait for this client's snapshot
      }
      if (!controlled) {
        this.owed = null; // nothing to pause
        return;
      }
    }
    this.inFlight = true;
    this.deps.post(client).then(
      (dto) => {
        this.inFlight = false;
        if (this.deps.client() !== client) {
          this.pump(); // a replaced client's reply writes nothing; send again
          return;
        }
        if (this.owed === owed) {
          this.owed = null;
        }
        this.deps.apply(client, dto);
        this.pump(); // a Stop pressed meanwhile may owe another override
      },
      () => {
        this.inFlight = false;
        if (!this.retryPending) {
          this.retryPending = true;
          this.deps.schedule(() => {
            this.retryPending = false;
            this.pump();
          }, OVERRIDE_RETRY_MS);
        }
      }
    );
  }
}
