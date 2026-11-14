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
    style: root.dataset.style ?? null,
    theme: root.dataset.theme ?? null,
    container: box(part("main")?.parentElement ?? null),
    rail: box(rail),
    main: box(part("main")),
    panel: panelBox,
    panelBorders: borders,
    panelContentRight: contentRight,
    panelInner: box(inner),
    panelInnerScroll: inner ? { scrollWidth: inner.scrollWidth, clientWidth: inner.clientWidth } : null,
    panelRightMostDescendant: worst,
    document: { scrollWidth: root.scrollWidth, clientWidth: root.clientWidth }
  };
}

export async function runLayoutProbe(): Promise<void> {
  document.documentElement.dataset.probe = "started";
  const style = document.createElement("style");
  style.textContent = "*,*::before,*::after{transition:none!important;animation:none!important}";
  document.head.appendChild(style);
  // Let React mount and apply the theme, then a short pause for layout to settle.
  // Timers only: a busy backdrop canvas can starve animation frames, and measuring forces layout anyway.
  await new Promise((done) => setTimeout(done, 100));
  await new Promise((done) => setTimeout(done, 100));
  const script = document.createElement("script");
  script.id = "layout-probe";
  script.type = "application/json";
  script.textContent = JSON.stringify(measure());
  document.body.appendChild(script);
}
