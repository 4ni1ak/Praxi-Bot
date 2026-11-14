import type { SaveWorkflow, WorkflowProposal } from "../api/types.ts";

export function canOfferWorkflow(canRemember: boolean, mutatingSteps: number): boolean {
  return canRemember && mutatingSteps >= 2;
}

export type ReviewState = {
  proposal: WorkflowProposal | null;
  draft: SaveWorkflow | null;
  open: boolean;
  phase: "loading" | "ready" | "saving" | "saved" | "unavailable";
  error: string | null;
};
export const EMPTY_REVIEW: ReviewState = { proposal: null, draft: null, open: false, phase: "loading", error: null };
export type ReviewAction =
  | { type: "loaded"; proposal: WorkflowProposal }
  | { type: "unavailable" }
  | { type: "open" }
  | { type: "close" }
  | { type: "edit"; draft: SaveWorkflow }
  | { type: "save" }
  | { type: "saved" }
  | { type: "failed"; error: string };

export function reviewReducer(state: ReviewState, action: ReviewAction): ReviewState {
  switch (action.type) {
    case "loaded": return { ...EMPTY_REVIEW, phase: "ready", proposal: action.proposal, draft: {
      run_id: action.proposal.run_id,
      name: action.proposal.workflow.name,
      description: action.proposal.workflow.description,
      params: action.proposal.workflow.params.map(p => ({ ...p })),
      steps: action.proposal.workflow.steps
    } };
    case "unavailable": return { ...EMPTY_REVIEW, phase: "unavailable" };
    case "open": return state.phase === "ready" ? { ...state, open: true } : state;
    case "close": return state.phase === "saving" ? state : { ...state, open: false };
    case "edit": return state.phase === "ready" ? { ...state, draft: action.draft, error: null } : state;
    case "save": return state.open && state.phase === "ready" && state.draft?.name.trim() ? { ...state, phase: "saving", error: null } : state;
    case "saved": return state.phase === "saving" ? { ...state, phase: "saved", open: false } : state;
    case "failed": return state.phase === "saving" ? { ...state, phase: "ready", error: action.error } : state;
  }
}
