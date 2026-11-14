/**
 * Orders control snapshots by the moment their request was issued, so a GET sent
 * before an action (Clear pause, Bitirdim) can never overwrite the newer state
 * that action returned, even if it resolves later.
 */
export interface SnapshotSequencer {
  /** Take a ticket when a request is ISSUED. */
  issue(): number;
  /** True (and recorded) when nothing newer has been applied yet. */
  accept(ticket: number): boolean;
  /** An action response is the newest state: invalidates every older ticket. */
  markFresh(): void;
}

export function createSnapshotSequencer(): SnapshotSequencer {
  let issued = 0;
  let applied = 0;
  return {
    issue: () => ++issued,
    accept(ticket) {
      if (ticket < applied) {
        return false;
      }
      applied = ticket;
      return true;
    },
    markFresh() {
      applied = ++issued;
    }
  };
}
