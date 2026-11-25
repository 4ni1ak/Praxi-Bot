(() => {
  "use strict";
  const norm = (s) => String(s || "").replace(/\s+/g, " ").trim().toLowerCase();

  // Well-known consent management platforms. `box` finds the dialog, `reject` / `accept` are
  // their own button ids / classes. Open shadow roots are searched too.
  const CMPS = [
    { name: "OneTrust", box: "#onetrust-banner-sdk, #onetrust-consent-sdk, #onetrust-pc-sdk",
      reject: "#onetrust-reject-all-handler, .ot-pc-refuse-all-handler, .ot-bnr-refuse-all-handler",
      accept: "#onetrust-accept-btn-handler" },
    { name: "Cookiebot", box: "#CybotCookiebotDialog",
      reject: "#CybotCookiebotDialogBodyButtonDecline, #CybotCookiebotDialogBodyButtonDeclineAll",
      accept: "#CybotCookiebotDialogBodyLevelButtonLevelOptinAllowAll, #CybotCookiebotDialogBodyButtonAccept" },
    { name: "Didomi", box: "#didomi-host, #didomi-popup, .didomi-popup-container, .didomi-notice-popup",
      reject: "#didomi-notice-disagree-button, .didomi-continue-without-agreeing",
      accept: "#didomi-notice-agree-button" },
    { name: "Quantcast", box: ".qc-cmp2-container, #qc-cmp2-container, .qc-cmp2-ui",
      reject: "", accept: ".qc-cmp2-summary-buttons button[mode='primary']" },
    { name: "TrustArc", box: "#truste-consent-track, #consent_blackbar, .truste_box_overlay",
      reject: "#truste-consent-required", accept: "#truste-consent-button" },
    { name: "Usercentrics", box: "#usercentrics-root, #usercentrics-cmp",
      reject: "[data-testid='uc-deny-all-button']", accept: "[data-testid='uc-accept-all-button']" },
    { name: "Klaro", box: ".klaro, .cookie-modal, .cookie-notice",
      reject: ".cn-decline, .cm-btn-decline", accept: ".cn-accept-all, .cm-btn-success, .cm-btn-accept-all" },
  ];

  const COOKIE_WORDS = /cookie|çerez|cerez|consent|gdpr|kvkk|privacy|gizlilik|datenschutz|tracking|tracker|rgpd|confidentialit|privacidad|privacidade|izin|einwilligung|zustimm/;
  const REJECT = [
    "reject", "decline", "deny", "refuse", "disagree", "opt out", "opt-out",
    "only necessary", "necessary only", "only essential", "essential only", "only required",
    "required only", "necessary cookies", "essential cookies", "continue without accepting",
    "continue without agreeing", "without accepting",
    "reddet", "yalnızca gerekli", "yalnızca zorunlu", "sadece gerekli", "sadece zorunlu",
    "yalnızca gerekli olan", "kabul etme", "kabul etmiyorum", "istemiyorum",
    "ablehnen", "nur notwendige", "nur erforderliche", "nur essenzielle", "nur essentielle",
    "refuser", "continuer sans accepter", "uniquement nécessaires", "seulement nécessaires",
    "rechazar", "solo necesarias", "solo las necesarias", "solo esenciales", "denegar",
    "rejeitar", "recusar", "apenas necessários", "apenas essenciais", "somente necessários",
    "rifiuta", "solo necessari",
  ];
  const ACCEPT_ALL = [
    "accept all", "allow all", "agree to all", "accept cookies", "accept and close",
    "tümünü kabul", "tüm çerezleri kabul", "hepsini kabul", "alle akzeptieren", "alle zulassen",
    "tout accepter", "aceptar todas", "aceptar todo", "aceitar tudo", "aceitar todos",
    "accetta tutti",
  ];
  const ACCEPT = [
    "accept", "agree", "allow", "got it", "i understand", "kabul", "anladım",
    "akzeptieren", "zustimmen", "einverstanden", "accepter", "j'accepte", "acepto",
    "aceptar", "aceitar", "aceito", "accetta",
  ];
  const ACCEPT_EXACT = ["ok", "okay", "tamam", "yes", "oui", "ja", "sí", "sim", "i agree"];
  const NOT_A_CHOICE = [
    "selected", "selection", "seçili", "seçilen", "auswahl", "manage", "settings", "customi",
    "preferences", "ayarlar", "tercih", "einstellung", "paramètres", "configurar", "more options",
    "learn more", "read more", "details", "policy", "politika", "daha fazla", "mehr",
    "save", "kaydet", "speichern", "enregistrer", "guardar",
  ];

  const classify = (label) => {
    if (!label || label.length > 70) return null;
    if (NOT_A_CHOICE.some((w) => label.includes(w))) return null;
    if (ACCEPT_ALL.some((w) => label.includes(w))) return "accept";
    if (REJECT.some((w) => label.includes(w))) return "reject";
    if (ACCEPT_EXACT.includes(label) || ACCEPT.some((w) => label.includes(w))) return "accept";
    return null;
  };

  // `node` and every open shadow root below it (or on it).
  const roots = (node) => {
    const out = [node];
    if (node.nodeType === 1 && node.shadowRoot) out.push(...roots(node.shadowRoot));
    for (const el of node.querySelectorAll("*")) {
      if (el.shadowRoot) out.push(...roots(el.shadowRoot));
    }
    return out;
  };
  const queryAll = (scope, sel) => {
    if (!sel) return [];
    const found = [];
    if (scope.nodeType === 1) {
      try { if (scope.matches(sel)) found.push(scope); } catch (_) {}
    }
    for (const s of roots(scope)) {
      try { found.push(...s.querySelectorAll(sel)); } catch (_) {}
    }
    return found;
  };

  const visible = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    const cs = getComputedStyle(el);
    return cs.visibility !== "hidden" && cs.display !== "none" && Number(cs.opacity || 1) > 0.05;
  };

  // A link that would leave the page is never clicked; neither is a submit button that posts
  // to another site.
  const safeToClick = (el) => {
    const tag = el.tagName.toLowerCase();
    if (tag === "a") {
      const href = (el.getAttribute("href") || "").trim().toLowerCase();
      const role = (el.getAttribute("role") || "").toLowerCase();
      if (!(role === "button" || href === "" || href.startsWith("#") || href.startsWith("javascript:"))) {
        return false;
      }
    }
    if (el.form) {
      try {
        const o = new URL(el.form.action, location.href).origin;
        if (o !== location.origin) return false;
      } catch (_) { return false; }
    }
    return true;
  };

  const labelOf = (el) =>
    norm(el.innerText || el.value || el.getAttribute("aria-label") || el.getAttribute("title") || "");

  const deepHit = (x, y) => {
    let h = document.elementFromPoint(x, y);
    while (h && h.shadowRoot) {
      const n = h.shadowRoot.elementFromPoint(x, y);
      if (!n || n === h) break;
      h = n;
    }
    return h;
  };

  // Scroll the element into view and return its centre when it is really what a click there
  // would hit.
  const target = (el) => {
    try { el.scrollIntoView({ block: "center", inline: "center", behavior: "instant" }); } catch (_) {}
    const r = el.getBoundingClientRect();
    const x = r.left + r.width / 2;
    const y = r.top + r.height / 2;
    if (x < 0 || y < 0 || x > innerWidth || y > innerHeight) return null;
    const hit = deepHit(x, y);
    if (!hit || !(hit === el || el.contains(hit))) return null;
    return { x, y };
  };

  const BUTTONS = "button, [role='button'], a, input[type='button'], input[type='submit']";
  const usable = (el) => visible(el) && safeToClick(el);
  // First element of `cands` whose label classifies as `kind` and that a click can reach.
  const pick = (cands, kind) => {
    for (const el of cands) {
      if (!usable(el)) continue;
      const label = labelOf(el);
      if (classify(label) !== kind) continue;
      const pt = target(el);
      if (pt) return { el, label, pt };
    }
    return null;
  };
  // First element of `cands` (found by a platform's own selector) a click can reach.
  const pickOwn = (cands, fallback) => {
    for (const el of cands) {
      if (!usable(el)) continue;
      const pt = target(el);
      if (pt) return { el, label: labelOf(el) || fallback, pt };
    }
    return null;
  };

  const reply = (cmp, kind, hit) => ({
    found: true, kind, cmp, label: hit.label.slice(0, 60), x: hit.pt.x, y: hit.pt.y,
    origin: location.origin,
  });

  // 1. Known platforms: their own ids first, then the button texts inside their dialog.
  let seenBox = false;
  for (const cmp of CMPS) {
    const boxes = queryAll(document, cmp.box);
    if (!boxes.length) continue;
    const inBox = (sel) => boxes.flatMap((b) => queryAll(b, sel));
    const buttons = inBox(BUTTONS);
    if (buttons.some(visible)) seenBox = true;
    let hit = pickOwn(inBox(cmp.reject), "reject");
    if (hit) return reply(cmp.name, "reject", hit);
    hit = pick(buttons, "reject");
    if (hit) return reply(cmp.name, "reject", hit);
    hit = pickOwn(inBox(cmp.accept), "accept");
    if (hit) return reply(cmp.name, "accept", hit);
    hit = pick(buttons, "accept");
    if (hit) return reply(cmp.name, "accept", hit);
  }

  // 2. Any banner-like element that talks about cookies / consent / privacy.
  const NAMED = "[role='dialog'], [aria-modal='true'], dialog, [id*='cookie' i], [class*='cookie' i], [id*='consent' i], [class*='consent' i], [id*='gdpr' i], [class*='gdpr' i], [id*='cmp' i], [class*='cmp-' i], [id*='privacy' i], [class*='privacy' i], [id*='cerez' i], [class*='cerez' i], [id*='kvkk' i], [class*='kvkk' i]";
  const floating = Array.from(document.querySelectorAll("div, section, aside, footer, header, form"))
    .filter((el) => {
      const p = getComputedStyle(el).position;
      return p === "fixed" || p === "sticky";
    });
  const isDialog = (el) => el.matches("[role='dialog'], [aria-modal='true'], dialog");
  const generic = Array.from(new Set([...queryAll(document, NAMED), ...floating]))
    .filter(visible)
    .filter((el) => {
      const text = norm(el.innerText);
      if (text.length > 2500 || !COOKIE_WORDS.test(text)) return false;
      const p = getComputedStyle(el).position;
      return p === "fixed" || p === "sticky" || p === "absolute" || isDialog(el);
    });
  if (generic.length) seenBox = true;
  // Smallest (innermost) dialogs first, so a page-wide wrapper does not decide.
  generic.sort((a, b) => a.getBoundingClientRect().width * a.getBoundingClientRect().height
    - b.getBoundingClientRect().width * b.getBoundingClientRect().height);
  const buttons = generic.flatMap((b) => queryAll(b, BUTTONS));
  let hit = pick(buttons, "reject");
  if (hit) return reply("generic", "reject", hit);
  hit = pick(buttons, "accept");
  if (hit) return reply("generic", "accept", hit);

  const frameHint = Array.from(document.querySelectorAll("iframe")).some((f) =>
    /consent|cookie|cmp|privacy|sourcepoint|sp_message|didomi|onetrust|cookiebot|trustarc|usercentrics/i
      .test((f.getAttribute("src") || "") + " " + (f.id || "") + " " + (f.title || "")));
  return { found: false, banner: seenBox, frame_hint: frameHint, origin: location.origin };
})()
