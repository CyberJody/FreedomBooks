'use strict';
// SIM-FIX item 1 smoke (run under xvfb-run, as a non-root user): every button the v1 simulation found
// "doing nothing" is clicked ONCE with a real (trusted, CDP-dispatched) mouse event at the place the user
// saw it, and exactly one command must reach the host (fb-node `run` for a POST, `render` for a link).
//
// Root cause (v1, TRIAGE.md item 1): no script guard swallowed the clicks. "Add line" inserts a line above
// the action row and the review re-render inserts the review section above it, so the buttons moved away
// from where they were aimed. The desktop preload keeps the action row under the pointer after Add line,
// scrolls a review's Record button into view, and drops a second submit while one is in flight.
// SIM-FIX item 5 checks (report presets, same preload) run at the end with the same counters.
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const shots = process.env.FB_SMOKE_SHOTS || path.join(root, 'staging-run');
fs.mkdirSync(shots, { recursive: true });
const userData = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-submit-once-'));
let step = 0;
const pass = (text) => console.log(`PASS ${String(++step).padStart(2, '0')}: ${text}`);
const heading = async (page) => (await page.locator('main h1').first().textContent()).trim();

(async () => {
  assert.ok(process.env.DISPLAY, 'run under xvfb-run');
  const app = await electron.launch({ chromiumSandbox: true,
    args: [root, '--password-store=basic'], cwd: root, timeout: 60000,
    env: { ...process.env, XDG_CONFIG_HOME: path.join(userData, 'config'), FB_DESKTOP_USER_DATA: userData, ELECTRON_ENABLE_LOGGING: '0' },
  });
  const dialogs = [];
  try {
    const page = await app.firstWindow();
    page.on('dialog', async (d) => { dialogs.push(d.message()); await d.dismiss(); });
    page.on('pageerror', (e) => console.log(`PAGE ERROR ${e.message}`));
    await page.setViewportSize({ width: 1440, height: 1000 });
    page.setDefaultTimeout(120000);
    page.setDefaultNavigationTimeout(120000);
    await page.waitForSelector('main h1');

    // Count what reaches the host: the main process calls these two native exports for every POST/GET.
    await app.evaluate(({ app: a }) => {
      const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
      const native = require(process.env.FB_NATIVE_MODULE || require('node:path').join(a.getAppPath(), 'native', 'fb-node.node'));
      const run = native.run, render = native.render;
      globalThis.__host = { runs: [], renders: [] };
      native.run = (json) => { globalThis.__host.runs.push(JSON.parse(json)); return run(json); };
      native.render = (target) => { globalThis.__host.renders.push(target); return render(target); };
    });
    const host = () => app.evaluate(() => {
      const h = globalThis.__host;
      const out = { submits: h.runs.filter((c) => c.type === 'SubmitForm').map((c) => ({ path: c.path, action: (c.fields.find(([k]) => k === 'action') || [])[1] || '' })), renders: h.renders.slice() };
      h.runs.length = 0; h.renders.length = 0;
      return out;
    });
    const centre = async (selector) => {
      const box = await page.locator(selector).first().boundingBox();
      assert.ok(box, `${selector} is laid out`);
      return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    };
    const inView = async (selector) => page.locator(selector).first().evaluate((el) => {
      const r = el.getBoundingClientRect();
      return r.top >= -1 && r.bottom <= window.innerHeight + 1 && document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) === el;
    });
    // Two animation frames: what a person (or a screenshot-driven tester) sees before the next click.
    const frame = () => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    const nav = async (selector, pattern) => Promise.all([page.waitForURL(pattern), page.click(selector)]);

    // Book, chart, directory and accounting setup through the real forms (setup only, not counted).
    await page.fill('#id_owner_name', 'Dana Owner');
    await page.fill('#id_company_name', 'Kettle Pond Supply');
    assert.equal(await page.isChecked('#id_protect'),false);
    await page.selectOption('#id_starter_chart', 'empty');
    await nav('button:has-text("Create books")', /\/home\/$/);
    const company = /\/c\/([0-9a-f-]{36})\/home\/$/.exec(page.url())[1];
    const base = `fb://app/c/${company}`;
    assert.ok((await host()).renders.length > 0, 'host counter sees renders');
    for (const [code, name, type] of [['1000', 'Checking', 'BANK'], ['1100', 'Accounts receivable', 'RECEIVABLE'], ['1200', 'Undeposited funds', 'OTHER_CURRENT_ASSET'], ['4000', 'Lawn care income', 'INCOME']]) {
      await page.goto(`${base}/accounts/new/`);
      await page.fill('#id_code', code);
      await page.fill('#id_name', name);
      await page.selectOption('#id_detail_type', type);
      await nav('button:has-text("Save")', /\/accounts\/$/);
    }
    const income = await app.evaluate(({ app: a }, c) => {
      const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
      const native = require(process.env.FB_NATIVE_MODULE || require('node:path').join(a.getAppPath(), 'native', 'fb-node.node'));
      return JSON.parse(native.query(JSON.stringify({ type: 'Accounts', company_id: c }))).find((r) => r.code === '4000').id;
    }, company);
    await page.goto(`${base}/lists/customers/new/`);
    await page.fill('#id_display_name', 'Maple & Birch Café');
    await nav('button:has-text("Save")', /lists\/customers\/[0-9a-f-]{36}\/$/);
    const customer = /customers\/([0-9a-f-]{36})/.exec(page.url())[1];
    await page.goto(`${base}/lists/items/new/`);
    await page.fill('#id_code', 'MOW');
    await page.fill('#id_description', 'Lawn mowing');
    await page.selectOption('#id_kind', 'SERVICE');
    await page.fill('#id_sale_rate', '45.00');
    await page.selectOption('#id_income_account', income);
    await nav('button:has-text("Save")', /lists\/items\/[0-9a-f-]{36}\/$/);
    const item = /items\/([0-9a-f-]{36})/.exec(page.url())[1];
    await page.goto(`${base}/setup/start/`);
    await nav('button:has-text("Fill Accounting setup")', /\/fill\/$/);
    await nav('button:has-text("Use these choices")', /\/setup\/start\/$/);
    pass('book, chart, customer, item and accounting setup created through the real forms');

    const estimateForm = async () => {
      await page.goto(`${base}/estimates/new/`);
      await page.selectOption('#id_customer', customer);
      await page.fill('#id_date', '2026-10-20');
      await page.selectOption('#id_lines-0-item', item);
      await page.fill('#id_lines-0-quantity', '2');
      await page.locator('#add-line').scrollIntoViewIfNeeded();
      await host();
    };

    // 1. "Save estimate": aimed, then "Add line", then one click where the user saw it.
    await estimateForm();
    const save = await centre('button:has-text("Save estimate")');
    await page.mouse.click(...Object.values(await centre('#add-line')));
    await frame();
    assert.equal(await page.inputValue('#id_lines-TOTAL_FORMS'), '4');
    assert.deepEqual((await host()).submits, [], 'Add line sends nothing');
    const now = await centre('button:has-text("Save estimate")');
    assert.ok(Math.abs(now.y - save.y) < 1 && Math.abs(now.x - save.x) < 1, `Save estimate stays under the pointer after Add line: ${JSON.stringify({ save, now })}`);
    await Promise.all([page.waitForURL(/estimates\/[0-9a-f-]{36}\/$/), page.mouse.click(save.x, save.y)]);
    let seen = await host();
    assert.deepEqual(seen.submits, [{ path: `/c/${company}/estimates/new/`, action: '' }]);
    pass('Save estimate: one real click after Add line -> exactly one SubmitForm');

    // 2. "Estimate list" (a link): same sequence -> exactly one GET of the list, no command.
    await estimateForm();
    const list = await centre('a:has-text("Estimate list")');
    await page.mouse.click(...Object.values(await centre('#add-line')));
    await frame();
    await host();
    await Promise.all([page.waitForURL(/\/estimates\/$/), page.mouse.click(list.x, list.y)]);
    seen = await host();
    assert.deepEqual(seen.submits, []);
    assert.deepEqual(seen.renders.filter((t) => t.startsWith(`/c/${company}/estimates/`)), [`/c/${company}/estimates/`]);
    pass('Estimate list: one real click after Add line -> exactly one list render, no command');

    // 3. A double click on Save estimate still sends one command and shows no "form has expired" alert.
    await estimateForm();
    const again = await centre('button:has-text("Save estimate")');
    await Promise.all([page.waitForURL(/estimates\/[0-9a-f-]{36}\/$/), page.mouse.dblclick(again.x, again.y)]);
    await page.waitForTimeout(500);
    seen = await host();
    assert.equal(seen.submits.length, 1, JSON.stringify(seen.submits));
    assert.deepEqual(dialogs, []);
    pass('Save estimate double click -> exactly one SubmitForm, no expired-form alert');

    // 4. Sales receipt: Review then "Record sales receipt — books only", one real click each.
    await page.goto(`${base}/sales-receipts/new/`);
    await page.selectOption('#id_customer', customer);
    await page.fill('#id_date', '2026-10-22');
    await page.selectOption('#id_method', 'CASH');
    await page.selectOption('#id_lines-0-item', item);
    await page.fill('#id_lines-0-quantity', '1');
    await page.selectOption('#id_lines-0-income_account', income);
    await page.locator('button:has-text("Review sales receipt")').scrollIntoViewIfNeeded();
    await host();
    const review = await centre('button:has-text("Review sales receipt")');
    await page.mouse.click(review.x, review.y);
    await page.waitForSelector('#review-total');
    seen = await host();
    assert.deepEqual(seen.submits, [{ path: `/c/${company}/sales-receipts/new/`, action: 'review' }]);
    pass('Review sales receipt: one real click -> exactly one SubmitForm (action=review)');
    const recordSel = 'button:has-text("Record sales receipt — books only")';
    assert.equal(await inView(recordSel), true, 'Record button is on screen after the review re-render');
    assert.notEqual(await page.evaluate(() => document.activeElement && document.activeElement.value), 'record', 'focus not moved to Record');
    await page.screenshot({ path: path.join(shots, 'sim-fix-01-sales-receipt-review.png') });
    const record = await centre(recordSel);
    await Promise.all([page.waitForURL(/sales-receipts\/[0-9a-f-]{36}\/$/), page.mouse.click(record.x, record.y)]);
    seen = await host();
    assert.deepEqual(seen.submits, [{ path: `/c/${company}/sales-receipts/new/`, action: 'record' }]);
    assert.match(await heading(page), /Sales receipt/);
    pass('Record sales receipt — books only: on screen after review; one real click -> exactly one SubmitForm (action=record)');
    assert.deepEqual(dialogs, []);

    // SIM-FIX item 5 (same preload): a report preset fills From/To before Run; typed dates switch to
    // Custom; a stale query shows the dates the report used in the address. One real Run -> one render.
    const pl = `/c/${company}/reports/profit-and-loss/`;
    await page.goto(`fb://app${pl}?preset=custom&start=2026-01-01&end=2026-01-31`);
    const last = await page.locator('#report-preset option[value=last_month]').evaluate((o) => [o.dataset.start, o.dataset.end]);
    assert.match(last[0], /^\d{4}-\d{2}-01$/);
    await page.selectOption('#report-preset', 'last_month');
    assert.deepEqual([await page.inputValue('input[name=start]'), await page.inputValue('input[name=end]')], last, 'From/To follow the preset before Run');
    await host();
    const run = await centre('button:has-text("Run report")');
    await Promise.all([page.waitForURL((u) => u.search.includes('preset=last_month')), page.mouse.click(run.x, run.y)]);
    seen = await host();
    assert.equal(new URL(page.url()).searchParams.get('start'), last[0]);
    assert.equal(new URL(page.url()).searchParams.get('end'), last[1]);
    assert.deepEqual(seen.renders.filter((t) => t.startsWith(pl)).length, 1);
    pass('report preset: Last month fills From/To before Run; one real Run click -> one render with matching dates');
    await page.fill('input[name=start]', '2026-02-03');
    await page.locator('input[name=start]').dispatchEvent('change');
    assert.equal(await page.inputValue('#report-preset'), 'custom', 'typed dates switch the preset to Custom');
    await page.goto(`fb://app${pl}?preset=last_month&start=2026-01-01&end=2026-01-31&basis=ACCRUAL`);
    const address = new URL(page.url());
    assert.deepEqual([address.searchParams.get('start'), address.searchParams.get('end'), address.searchParams.get('basis')], [...last, 'ACCRUAL']);
    const csv = new URL(await page.locator('a:has-text("Download CSV")').getAttribute('href'), page.url());
    assert.deepEqual([csv.searchParams.get('start'), csv.searchParams.get('end')], last);
    pass('report preset: typed dates -> Custom; a stale query shows the resolved dates in the address and the CSV link');
    console.log(`SMOKE SUBMIT-ONCE OK: ${step} checks`);
  } catch (error) {
    console.error(`SMOKE FAILED at step ${step + 1}: ${error.stack || error}`);
    process.exitCode = 1;
  } finally {
    await app.close();
    fs.rmSync(userData, { recursive: true, force: true });
  }
})();

