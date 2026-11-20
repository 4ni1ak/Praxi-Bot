import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8");

test("usage table: numeric cells are right-aligned like their headers (td rule is more specific than .num)", () => {
  const css = read("../views/UsageView.module.css");
  assert.match(css, /\.table td\.num[\s\S]*?text-align:\s*right/);
  assert.match(css, /\.table th\.num/);
});

test("modals focus their default button without scrolling a long dialog to its footer", () => {
  assert.match(read("../components/Modal.tsx"), /focus\(\{ preventScroll: true \}\)/);
});

test("the dictation row stacks its wide controls under the text, and chat tables never break words mid-letter", () => {
  assert.match(read("../views/settings/DictationCard.tsx"), /<SettingRow stacked /);
  assert.match(read("../components/Markdown.module.css"), /\.table td \{[^}]*overflow-wrap:\s*normal/);
});

test("usage: big stat numbers and the seven-column table shrink in a narrow pane instead of clipping or hiding the cost column", () => {
  const css = read("../views/UsageView.module.css");
  assert.match(css, /@container \(width < 90ch\)\s*\{\s*\.statValue/);
  assert.match(css, /\.tableCard \.table td/);
});

test("trusted folders list is styled with design tokens, not bare paragraphs", () => {
  assert.match(read("../components/TrustedFolders.tsx"), /className=\{formStyles\.hint\}/);
});

test("composer toolbar: the model chip and computer chip can shrink so Send stays visible in a narrow chat column", () => {
  assert.match(read("../views/ChatView.module.css"), /\.modelSlot \{[^}]*min-width:\s*var\(--space-0\)/);
  assert.match(read("../views/ChatView.tsx"), /className=\{styles\.modelSlot\}/);
  assert.match(read("../components/ModelPicker.module.css"), /max-width:\s*min\(100%,/);
  assert.match(read("../components/ComputerTargetChip.module.css"), /\.chip \{[^}]*white-space:\s*nowrap/);
});

test("shortcut keys never wrap inside their pill; the spending-limit 'off' value stays on one line height", () => {
  assert.match(read("../components/HelpModal.module.css"), /\.kbd \{[^}]*white-space:\s*nowrap/);
  assert.match(read("../components/SpendLimitCard.module.css"), /\.statOff \{/);
});
