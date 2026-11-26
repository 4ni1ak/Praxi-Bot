import test from "node:test";
import assert from "node:assert/strict";
import { canOfferWorkflow, EMPTY_REVIEW, reviewReducer } from "./workflowReview.ts";
import type { WorkflowProposal } from "../api/types.ts";

const proposal: WorkflowProposal = {
  run_id: "run-1",
  workflow: { name: "reviewed", description: "<script>untrusted page text</script>", source: null, revision: 1, params: [
    { name: "password", type: "string", default: null, description: "Required" }
  ], steps: [{ kind: "shell", command: "echo 'full command including trailing arguments'" }, { kind: "tool", input: { text: "untrusted data" } }] },
  step_provenance: [],
  warnings: [], skipped: [], required_params: ["password"]
};

test("offer requires server can_remember and at least two recorded mutations", () => {
  assert.equal(canOfferWorkflow(false, 3), false);
  assert.equal(canOfferWorkflow(true, 0), false);
  assert.equal(canOfferWorkflow(true, 1), false);
  assert.equal(canOfferWorkflow(true, 2), true);
});

test("loading and opening never save; explicit review is required", () => {
  const loaded = reviewReducer(EMPTY_REVIEW, { type: "loaded", proposal });
  assert.equal(loaded.open, false);
  assert.equal(loaded.phase, "ready");
  assert.deepEqual(reviewReducer(loaded, { type: "save" }), loaded);
  const opened = reviewReducer(loaded, { type: "open" });
  assert.equal(opened.phase, "ready");
  assert.deepEqual(opened.proposal?.workflow.steps, proposal.workflow.steps);
  assert.deepEqual(opened.draft?.steps, proposal.workflow.steps);
  assert.equal(opened.draft?.description, proposal.workflow.description);
  assert.equal(opened.draft?.params[0].default, null);
  const saving = reviewReducer(opened, { type: "save" });
  assert.equal(saving.phase, "saving");
  assert.deepEqual(reviewReducer(saving, { type: "close" }), saving);
  const saved = reviewReducer(saving, { type: "saved" });
  assert.equal(saved.open, false);
  assert.equal(saved.phase, "saved");
});

test("cancel retains edits for review, failures permit retry without losing data", () => {
  let state = reviewReducer(EMPTY_REVIEW, { type: "loaded", proposal });
  state = reviewReducer(state, { type: "open" });
  state = reviewReducer(state, { type: "edit", draft: { ...state.draft!, name: "edited" } });
  state = reviewReducer(state, { type: "close" });
  assert.equal(state.draft?.name, "edited");
  state = reviewReducer(state, { type: "open" });
  state = reviewReducer(state, { type: "save" });
  state = reviewReducer(state, { type: "failed", error: "name exists" });
  assert.equal(state.open, true);
  assert.equal(state.phase, "ready");
  assert.equal(state.error, "name exists");
  assert.equal(state.draft?.name, "edited");
});

test("external influence requires an explicit acknowledgement for every affected step", () => {
  const influenced = { ...proposal, step_provenance: [{step_index:0,needs_confirmation:true,origin:"run_content_or_model",parameters:[],untrusted_read_before:true}] };
  let state = reviewReducer(EMPTY_REVIEW, {type:"loaded", proposal:influenced});
  state = reviewReducer(state, {type:"open"});
  assert.equal(reviewReducer(state, {type:"save"}).phase, "ready");
  state = reviewReducer(state, {type:"edit",draft:{...state.draft!, confirmed_steps:[0]}});
  assert.equal(reviewReducer(state, {type:"save"}).phase, "saving");
});
