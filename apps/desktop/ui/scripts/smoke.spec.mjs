// The flows of the real-app smoke run (card 263). Driven by scripts/smoke.mjs; no test framework:
// each step is a function, a thrown error fails it, and every step ends with a screenshot plus a
// check for error banners, danger toasts, console errors and failed requests.
/* global window, document, location, fetch, URL, console */
import process from 'node:process';
import { join } from 'node:path';

const SETTINGS = JSON.stringify({ onboardingCompleted: true, onboardingSkipped: false, language: 'en' });
const EN = JSON.stringify({ language: 'en' });

/** A browser context whose page believes it runs inside the Tauri app (see smoke.mjs for the shim). */
async function newSession(ctx, { onboarded }) {
  const context = await ctx.browser.newContext({ viewport: { width: 1280, height: 800 } });
  await context.exposeFunction('__smokeInvoke', (cmd, args) => ctx.invoke(cmd, args));
  await context.addInitScript(({ settings, fresh }) => {
    try { if (!window.localStorage.getItem('desktop_ui_settings')) window.localStorage.setItem('desktop_ui_settings', settings); } catch { /* storage unavailable */ }
    void fresh;
    window.__errs = [];
    window.addEventListener("error", (e) => window.__errs.push(`error: ${e.message} ${e.error && e.error.stack}`));
    window.addEventListener("unhandledrejection", (e) => window.__errs.push(`rejection: ${e.reason && (e.reason.stack || e.reason)}`));
    const origReport = window.reportError && window.reportError.bind(window);
    window.reportError = (e) => { window.__errs.push(`reportError: ${e && e.stack}`); if (origReport) origReport(e); };
    const ce = console.error.bind(console);
    console.error = (...a) => { window.__errs.push(a.map((x) => (x && x.stack) || String(x)).join(' ').slice(0, 1500)); ce(...a); };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    let cb = 1;
    window.__TAURI_INTERNALS__ = {
      invoke: (cmd, args) => window.__smokeInvoke(cmd, args ?? {}),
      transformCallback: () => cb++,
      unregisterCallback: () => {},
      convertFileSrc: (p) => p,
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } }
    };
  }, { settings: onboarded ? SETTINGS : EN, fresh: !onboarded });
  const page = await context.newPage();
  const problems = [];
  if (process.env.SMOKE_DEBUG) page.on("console", (m) => process.stdout.write(`[console.${m.type()}] ${m.text().slice(0, 1500)} @ ${m.location().url}:${m.location().lineNumber}${String.fromCharCode(10)}`));
  if (process.env.SMOKE_DEBUG) page.on('requestfailed', (r) => process.stdout.write(`[requestfailed] ${r.url()} ${r.failure()?.errorText}
`));
  page.on('console', (m) => { if (m.type() === 'error' && !m.location().url.endsWith('/favicon.ico')) problems.push(`console: ${m.text().slice(0, 300)}`); });
  page.on('pageerror', (e) => problems.push(`pageerror: ${String(e).slice(0, 300)}`));
  page.on('response', (r) => { if (r.status() >= 500 && r.url().includes('/v1/')) problems.push(`HTTP ${r.status()} ${r.request().method()} ${new URL(r.url()).pathname}`); });
  return { context, page, problems };
}

/** What a person would call "an error on screen" right now. */
async function visibleErrors(page) {
  return page.evaluate(() => {
    const out = [];
    for (const el of document.querySelectorAll('[role="alert"],[data-testid="error-notice"],[data-testid="toast"][class*="danger"],[data-testid="fr-error"],[data-testid="daemon-offline-empty-state"],[data-testid="fr-service-down"]')) {
      const r = el.getBoundingClientRect();
      if (r.width > 0 && r.height > 0 && el.textContent?.trim()) out.push(el.textContent.trim().slice(0, 240));
    }
    return out;
  });
}

export async function flows(ctx) {
  const results = [];
  let n = 0;
  const run = async (session, name, fn, opts = {}) => {
    if (ctx.only && !name.includes(ctx.only)) return;
    n += 1;
    const slug = `${String(n).padStart(2, '0')}-${name.replace(/[^a-z0-9]+/gi, '-').toLowerCase()}`;
    const { page, problems } = session;
    problems.length = 0;
    let error = null;
    try {
      await fn(page);
      await page.waitForTimeout(400);
      const errs = await visibleErrors(page);
      if (errs.length && !opts.expectError) throw new Error(`error on screen: ${errs.join(' | ')}`);
      // A step that expects the daemon to refuse something sees a 4xx in the console: that is the point of it.
      const real = opts.expectError ? problems.filter((p) => !p.startsWith('console: Failed to load resource')) : problems;
      if (real.length) throw new Error(`browser problems: ${[...new Set(real)].join(' | ')}`);
    } catch (e) {
      error = String(e.message ?? e).split('\n')[0];
    }
    if (error && process.env.SMOKE_DEBUG) {
      const dump = await page.evaluate(() => `${location.href} root=${document.getElementById("root")?.innerHTML.length} ${(window.__errs ?? []).join(" --- ")}`).catch((e) => String(e));
      process.stdout.write(`[debug] ${dump.slice(0, 4000)}${String.fromCharCode(10)}`);
    }
    await page.screenshot({ path: join(ctx.shots, `${slug}.png`) }).catch(() => {});
    results.push({ name, ok: !error, error });
    ctx.log(`${error ? 'FAIL' : 'ok  '} ${name}${error ? `  - ${error}` : ''}`);
  };
  const T = { timeout: 15000 };

  // ---- first run: skip path, then the API-key path against the stub
  const first = await newSession(ctx, { onboarded: false });
  await run(first, 'first run: wizard opens once (no loop) and Skip leaves a usable app', async (page) => {
    await page.goto(ctx.uiUrl);
    await page.getByTestId('onboarding-view').waitFor(T);
    await page.getByTestId('onboarding-skip').click();
    await page.getByTestId('finish-setup-hint').waitFor(T);
    await page.reload();
    await page.getByTestId('finish-setup-hint').waitFor(T);
    if (await page.getByTestId('onboarding-view').isVisible()) throw new Error('wizard came back after Skip and reload');
  });
  await first.context.close();

  const wiz = await newSession(ctx, { onboarded: false });
  await run(wiz, 'first run: API-key path connects, picks a model and creates the bot', async (page) => {
    await page.goto(ctx.uiUrl);
    await page.getByTestId('onboarding-view').waitFor(T);
    await page.getByTestId('fr-api-openai').click();
    await page.getByTestId('fr-key').fill('sk-smoke-test-key-123456');
    await page.getByTestId('fr-connect').click();
    await page.getByTestId('fr-connected').waitFor(T);
    if (!(await page.getByTestId('fr-model').textContent())?.trim()) throw new Error('no model chosen after connecting');
    await page.getByTestId('fr-next').click();
    await page.getByTestId('fr-level-balanced').waitFor(T);
    await page.getByTestId('fr-next').click();
    await page.getByTestId('new-bot-name').fill('Atlas');
    await page.getByTestId('fr-next').click();
    await page.getByTestId('bot-empty-state').waitFor(T);
  });
  await wiz.context.close();

  // ---- the day-to-day flows (wizard done)
  const s = await newSession(ctx, { onboarded: true });
  const composer = (page) => page.getByRole('combobox', { name: /message/i });
  const send = async (page, text) => {
    const c = composer(page);
    await c.fill(text);
    await c.press('Enter');
  };
  const openFirstChat = async (page) => {
    await page.getByTestId('bot-empty-new-chat').or(composer(page)).first().waitFor(T);
    if (await page.getByTestId('bot-empty-new-chat').isVisible()) await page.getByTestId('bot-empty-new-chat').click();
    await composer(page).waitFor(T);
  };

  const gotoChat = async (page, name) => {
    if (!(await composer(page).isVisible().catch(() => false))) {
      await page.locator('[class*="railItem"]', { hasText: name }).first().click();
      await openFirstChat(page);
    }
  };

  await run(s, 'app loads with bots and no wizard', async (page) => {
    await page.goto(ctx.uiUrl);
    await page.getByTestId('home-empty-state').or(page.getByTestId('bot-empty-state')).or(page.getByTestId('chat-timeline')).waitFor(T);
    if (await page.getByTestId('onboarding-view').isVisible()) throw new Error('wizard shown although setup is done');
  });
  await run(s, 'status chip menu opens', async (page) => {
    await page.getByTestId('computer-picker').click();
    await page.getByTestId('home-status').waitFor(T);
    await page.keyboard.press('Escape');
  });
  await run(s, 'create a second bot from the New-bot modal', async (page) => {
    await page.getByRole('button', { name: /new bot/i }).first().click();
    await page.getByTestId('new-bot-name').fill('Nova');
    await page.getByTestId('new-bot-submit').click();
    await page.getByTestId('bot-empty-state').waitFor(T);
  });
  await run(s, 'open a chat and send a message; the scripted reply arrives', async (page) => {
    await page.locator('[class*="railItem"]', { hasText: 'Atlas' }).first().click();
    await openFirstChat(page);
    await send(page, 'hello smoke');
    await page.getByText(/echo: hello smoke/).first().waitFor(T);
  });
  await run(s, 'model picker: pick another model, it is still chosen after reload', async (page) => {
    await page.getByTestId('model-picker').click();
    const option = page.getByRole('option', { name: /large/i }).first();
    await option.waitFor(T);
    await option.click();
    await page.waitForTimeout(800);
    await page.reload();
    await gotoChat(page, 'Atlas');
    const text = (await page.getByTestId('model-picker').textContent()) ?? '';
    if (!/large/i.test(text)) throw new Error(`model not saved, picker shows "${text.trim()}"`);
  });
  await run(s, 'model picker: a coding-CLI group is never shown with 0 models', async (page) => {
    await page.getByTestId('model-picker').click();
    await page.getByRole('dialog').waitFor(T);
    const counts = await page.locator('[data-testid^="model-picker-group-cli:"] [class*="groupCount"]').allTextContents();
    if (counts.some((c) => c.trim() === '0')) throw new Error(`a CLI group is rendered with 0 models: ${counts.join(',')}`);
    await page.keyboard.press('Escape');
  });
  await run(s, 'a pure-read tool call runs and its result is shown', async (page) => {
    await gotoChat(page, 'Atlas');
    await send(page, `call:fs_read ${JSON.stringify({ path: join(ctx.workDir, 'hello.txt') })}`);
    await page.getByText(/tool said: .*hello from the smoke run/).first().waitFor(T);
    if (await page.getByText(/step failed/i).first().isVisible().catch(() => false)) throw new Error('a tool step failed: ' + ((await page.getByText(/tool said/).last().textContent()) ?? ''));
  });
  await run(s, 'a tool that needs approval shows the card; Allow works', async (page) => {
    await send(page, `call:fs_write ${JSON.stringify({ path: join(ctx.workDir, 'smoke-out.txt'), content: 'hi' })}`);
    const allow = page.getByRole('button', { name: /allow once|^allow/i }).first();
    await allow.waitFor(T);
    await allow.click();
    await page.getByText(/tool said: wrote 2 bytes/).first().waitFor(T);
    if (await page.getByText(/step failed/i).first().isVisible().catch(() => false)) throw new Error('a tool step failed after Allow');
  });
  await run(s, 'bot panel opens with its sections', async (page) => {
    await page.getByTestId('bot-chip').click();
    await page.getByTestId('bot-panel').waitFor(T);
  });
  await run(s, 'edit the bot (name, permission) and save twice in a row', async (page) => {
    for (const [name, preset] of [['Atlas Two', 'full'], ['Atlas Three', 'read-and-draft']]) {
      await page.getByTestId('bot-panel-edit').click();
      await page.getByTestId('bot-edit-name').fill(name);
      await page.getByText('Permissions', { exact: true }).first().click();
      await page.getByTestId(`bot-edit-preset-${preset}`).click();
      await page.getByTestId('bot-edit-save').click();
      await page.getByTestId('bot-edit-dialog').waitFor({ state: 'hidden', timeout: 15000 });
    }
    await page.getByText('Atlas Three').first().waitFor(T);
    // What the daemon stored, not just what the screen shows.
    const bots = await (await fetch(`${ctx.base}/v1/bots`, { headers: { authorization: `Bearer ${ctx.token}` } })).json();
    const atlas = bots.find((x) => x.name === 'Atlas Three');
    if (!atlas) throw new Error('the daemon does not have the renamed bot');
    if (!JSON.stringify(atlas.config).includes('read-and-draft')) throw new Error('the permission level was not stored');
  });
  await run(s, '@OtherBot mention is answered as the other bot', async (page) => {
    await send(page, '@Nova say hi');
    await page.getByTestId('guest-author').first().waitFor(T);
  });
  await run(s, 'Settings > AI lists providers and the CLI engines section renders', async (page) => {
    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByText('AI', { exact: true }).first().click();
    await page.getByTestId('api-key-paste').waitFor(T);
    await page.getByTestId('detected-accounts-block').waitFor(T);
    await page.getByText('stub', { exact: false }).first().waitFor(T);
    await page.keyboard.press('Escape');
  });
  await run(s, 'Apps modal opens at once and shows tiles', async (page) => {
    await page.getByTestId('open-apps').click();
    await page.getByTestId('apps-grid').waitFor({ timeout: 4000 });
    await page.keyboard.press('Escape');
  });
  await run(s, 'add-server wizard opens and validates input without SSH', async (page) => {
    await page.getByTestId('computer-picker').click();
    await page.getByTestId('computer-add').click();
    await page.getByTestId('add-server').waitFor(T);
    const start = page.getByTestId('add-server-start');
    if (await start.isEnabled()) throw new Error('Start is enabled with an empty host');
    await page.getByTestId('add-server-host').fill('not a host !!');
    // The daemon judges the name: a bad one must come back as a human sentence, with no SSH attempt.
    await start.click();
    const alert = page.getByTestId('add-server').getByRole('alert').first();
    await alert.waitFor(T);
    const text = ((await alert.textContent()) ?? '').trim();
    if (!text || /error:|panic|ssh:|exception|\bat \w+\./i.test(text)) throw new Error(`not a human error: "${text}"`);
    await page.keyboard.press('Escape');
  }, { expectError: true });
  await run(s, 'owner routes refuse the general token alone and accept the owner key (card 262)', async () => {
    // The UI flows above already did these things with the owner key; here the same routes are
    // called the way a CLI engine or a script holding only the API token would call them.
    const post = (path, body, owner) => fetch(`${ctx.base}${path}`, {
      method: 'POST',
      headers: { authorization: `Bearer ${ctx.token}`, 'content-type': 'application/json', ...(owner ? { 'x-praxi-owner-key': ctx.ownerKey } : {}) },
      body: JSON.stringify(body)
    });
    const ghost = '00000000-0000-0000-0000-000000000001';
    for (const [path, body] of [
      [`/v1/approvals/${ghost}`, { decision: 'approve', scope: 'always' }],
      ['/v1/providers/cli/enable', { id: 'claude' }],
      ['/v1/secrets/smoke/extra-provider-key', { value: 'k' }]
    ]) {
      const bare = await post(path, body, false);
      const code = (await bare.json().catch(() => ({})))?.error?.code;
      if (bare.status !== 403 || code !== 'owner_key_required') throw new Error(`${path}: the general token alone got ${bare.status} ${code}`);
      const owner = await post(path, body, true);
      const ownerCode = (await owner.json().catch(() => ({})))?.error?.code;
      if (ownerCode === 'owner_key_required') throw new Error(`${path}: refused even with the owner key`);
    }
  });
  await s.context.close();
  return results;
}
