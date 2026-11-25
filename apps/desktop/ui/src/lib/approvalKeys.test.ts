import assert from "node:assert/strict";
import { test } from "node:test";
import { approvalKeyAction, type ApprovalKeyEvent } from "./approvalKeys.ts";

/** A DOM element as far as the rules see it: which selectors it (or an ancestor) matches. */
function el(tag: string, ancestry: string[], value?: string) {
  return {
    tagName: tag,
    value,
    closest: (sel: string) => (ancestry.some((a) => sel.split(",").map((s) => s.trim()).includes(a)) ? {} : null)
  };
}

const ev = (key: string, target: unknown, extra: Partial<ApprovalKeyEvent> = {}): ApprovalKeyEvent => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  target,
  ...extra
});
const ready = { armed: true, canAllow: true };

test("Enter allows and Escape denies when the focus is on the page", () => {
  const body = el("BODY", []);
  assert.equal(approvalKeyAction(ev("Enter", body), ready), "allow");
  assert.equal(approvalKeyAction(ev("Escape", body), ready), "deny");
});

test("an empty composer lets the keys through, a composer with text never does", () => {
  const empty = el("TEXTAREA", ["[data-composer-input]", "textarea"], "");
  const typed = el("TEXTAREA", ["[data-composer-input]", "textarea"], "hello");
  assert.equal(approvalKeyAction(ev("Enter", empty), ready), "allow");
  assert.equal(approvalKeyAction(ev("Escape", empty), ready), "deny");
  assert.equal(approvalKeyAction(ev("Enter", typed), ready), null);
  assert.equal(approvalKeyAction(ev("Escape", typed), ready), null);
});

test("another field, a dialog, a modifier or an IME composition never decides a card", () => {
  const search = el("INPUT", ["input"], "");
  assert.equal(approvalKeyAction(ev("Escape", search), ready), null);
  const inModal = el("BODY", ["[role='dialog']"]);
  assert.equal(approvalKeyAction(ev("Enter", inModal), ready), null);
  assert.equal(approvalKeyAction(ev("Enter", el("BODY", []), { ctrlKey: true }), ready), null);
  assert.equal(approvalKeyAction(ev("Enter", el("BODY", []), { shiftKey: true }), ready), null);
  assert.equal(approvalKeyAction(ev("Enter", el("BODY", []), { isComposing: true }), ready), null);
  assert.equal(approvalKeyAction(ev("Enter", el("BODY", []), { defaultPrevented: true }), ready), null);
  // Somewhere else in the page (a sidebar button) is not "the page itself".
  assert.equal(approvalKeyAction(ev("Escape", el("BUTTON", ["button"])), ready), null);
});

test("inside the dock a focused button keeps its own Enter, Escape still denies", () => {
  const button = el("BUTTON", ["[data-approval-dock]", "button"]);
  assert.equal(approvalKeyAction(ev("Enter", button), ready), null);
  assert.equal(approvalKeyAction(ev("Escape", button), ready), "deny");
  const reason = el("INPUT", ["[data-approval-dock]", "input"], "");
  assert.equal(approvalKeyAction(ev("Escape", reason), ready), null);
});

test("Enter never allows a card that must be read or that has only just appeared", () => {
  const body = el("BODY", []);
  assert.equal(approvalKeyAction(ev("Enter", body), { armed: false, canAllow: true }), null);
  assert.equal(approvalKeyAction(ev("Enter", body), { armed: true, canAllow: false }), null);
  // Denying is always safe, so it needs neither.
  assert.equal(approvalKeyAction(ev("Escape", body), { armed: false, canAllow: false }), "deny");
});
