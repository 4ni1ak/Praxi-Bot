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

/**
 * Admit a snapshot into state. A ticketed one (a GET) is dropped when something
 * newer was applied; an untagged one (an action response: native Pause override,
 * emergency Stop, release, an aborted re-read) is the newest state and
 * invalidates every older in-flight GET.
 */
export function admitSnapshot(
  seq: SnapshotSequencer,
  ticket: number | undefined
): boolean {
  if (ticket === undefined) {
    seq.markFresh();
    return true;
  }
  return seq.accept(ticket);
}
