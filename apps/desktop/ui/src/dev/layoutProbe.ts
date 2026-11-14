/**
 * Layout probe (card 233, mock/dev only): after the shell settled, measure the shell boxes and
 * append them as `<script id="layout-probe" type="application/json">` so a headless
 * `--dump-dom` run can read them without a debugging connection.  Enabled by `?probe=layout`.
 */

interface Box {
  left: number;
  right: number;
  top: number;
  bottom: number;
  width: number;
}

const box = (el: Element | null): Box | null => {
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return { left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width };
};

function measure() {
  const part = (name: string) => document.querySelector(`[data-shell-part="${name}"]`);
  const rail = document.querySelector("[data-rail-tier]");
  const panel = part("panel");
  const inner = part("panel-inner");
  const cs = panel ? getComputedStyle(panel) : null;
  const borders = cs
    ? {
        left: parseFloat(cs.borderLeftWidth) || 0,
        right: parseFloat(cs.borderRightWidth) || 0,
        top: parseFloat(cs.borderTopWidth) || 0,
        bottom: parseFloat(cs.borderBottomWidth) || 0
      }
    : null;
  const panelBox = box(panel);
  const contentRight = panelBox && borders ? panelBox.right - borders.right : null;
  let worst: { tag: string; cls: string; right: number } | null = null;
  inner?.querySelectorAll("*").forEach((el) => {
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return;
    if (!worst || r.right > worst.right) {
      worst = { tag: el.tagName.toLowerCase(), cls: String((el as HTMLElement).className).slice(0, 60), right: r.right };
    }
  });
  const root = document.documentElement;
  return {
    viewport: { width: window.innerWidth, height: window.innerHeight },
    float: root.dataset.float ?? null,
    corners: root.dataset.corners ?? null,
    backdrop: root.dataset.backdrop ?? null,
    style: root.dataset.style ?? null,
    theme: root.dataset.theme ?? null,
    container: box(part("main")?.parentElement ?? null),
    rail: box(rail),
    main: box(part("main")),
    panel: panelBox,
    panelOpenAttr: panel?.getAttribute("data-open") ?? null,
    panelDescendants: inner
      ? Array.from(inner.querySelectorAll("*")).filter((e) => e.getBoundingClientRect().width > 0).length
      : 0,
    panelBorders: borders,
    panelContentRight: contentRight,
    panelInner: box(inner),
    panelInnerScroll: inner ? { scrollWidth: inner.scrollWidth, clientWidth: inner.clientWidth } : null,
    panelRightMostDescendant: worst,
    document: { scrollWidth: root.scrollWidth, clientWidth: root.clientWidth }
  };
}

const POLL_MS = 50;
const DEADLINE_MS = 15000;
const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));

/** The reasons the shell is not ready to be measured yet (empty = ready); the URL says what to expect. */
function notReady(): string[] {
  const q = new URLSearchParams(window.location.search);
  const wantOpen = q.get("panel") === "1";
  const root = document.documentElement;
  const part = (name: string) => document.querySelector(`[data-shell-part="${name}"]`);
  const missing: string[] = [];
  if (!part("main")) missing.push("main");
  if (!document.querySelector("[data-rail-tier]")) missing.push("rail");
  const panel = part("panel");
  if (!panel) missing.push("panel");
  const inner = part("panel-inner");
  if (!inner) missing.push("panel-inner");
  if (panel && panel.getAttribute("data-open") !== String(wantOpen)) missing.push(`panel data-open=${wantOpen}`);
  for (const [key, value] of [["theme", q.get("theme")], ["style", q.get("style")], ["corners", q.get("corners")]] as const) {
    if (value && root.dataset[key] !== value) missing.push(`${key}=${value}`);
  }
  if (wantOpen && inner && inner.querySelector("*") === null) missing.push("panel content");
  if (document.fonts && document.fonts.status !== "loaded") missing.push("fonts");
  return missing;
}

export async function runLayoutProbe(): Promise<void> {
  document.documentElement.dataset.probe = "started";
  const style = document.createElement("style");
  style.textContent = "*,*::before,*::after{transition:none!important;animation:none!important}";
  document.head.appendChild(style);
  // Wait for the requested state with a deadline (timers only: a busy backdrop canvas can starve
  // animation frames, and measuring forces layout anyway).  A missing shell is reported, not measured.
  let waited = 0;
  let missing = notReady();
  while (missing.length > 0 && waited < DEADLINE_MS) {
    await pause(POLL_MS);
    waited += POLL_MS;
    missing = notReady();
  }
  await pause(POLL_MS * 2);
  if (missing.length === 0) missing = notReady();
  const script = document.createElement("script");
  script.id = "layout-probe";
  script.type = "application/json";
  script.textContent = JSON.stringify(
    missing.length > 0 ? { error: `not ready after ${waited} ms: ${missing.join(", ")}` } : measure()
  );
  document.body.appendChild(script);
}
