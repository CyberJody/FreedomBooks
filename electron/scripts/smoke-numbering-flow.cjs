'use strict';
// Focused onboarding -> starter chart -> numbering flow. Synthetic book, real sandboxed Electron.
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const shots = process.env.FB_SMOKE_SHOTS;
assert.ok(shots && path.isAbsolute(shots), 'set FB_SMOKE_SHOTS to durable evidence');
fs.mkdirSync(shots, { recursive: true });
const data = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-numbering-flow-'));
let checks = 0;
const pass = text => console.log(`PASS ${++checks}: ${text}`);
(async () => {
  const app = await electron.launch({ chromiumSandbox: true, args: [root, '--password-store=basic'], cwd: root, timeout: 60000,
    env: { ...process.env, HOME: path.join(data, 'home'), XDG_CONFIG_HOME: path.join(data, 'config'), XDG_DATA_HOME: path.join(data, 'share'), XDG_CACHE_HOME: path.join(data, 'cache'), FB_DESKTOP_USER_DATA: data } });
  try {
    const page = await app.firstWindow();
    page.setDefaultTimeout(120000);
    page.setDefaultNavigationTimeout(120000);
    await page.setViewportSize({ width: 1440, height: 1000 });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    page.on('dialog', async d => { errors.push(d.message()); await d.dismiss(); });
    const nav = (selector, url) => Promise.all([page.waitForURL(url), page.locator(selector).click()]);
    const shot = (p, name) => p.screenshot({ path: path.join(shots, `${name}.png`) });
    assert.equal(await page.inputValue('#id_starter_chart'), 'standard');
    assert.equal(await page.isChecked('#id_protect'), false);
    await page.fill('#id_owner_name', 'Numbering owner');
    await page.fill('#id_company_name', 'Synthetic numbered company');
    await nav('button:has-text("Create books")', /\/home\/$/);
    const company = /\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    const base = `fb://app/c/${company}`;
    const ids = await app.evaluate(({ app: a }, c) => {
      const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
      const native = require(require('node:path').join(a.getAppPath(), 'native/fb-node.node'));
      const rows = JSON.parse(native.query(JSON.stringify({ type: 'Accounts', company_id: c })));
      if (rows.length !== 34) throw new Error('Standard chart must contain all 34 accounts');
      const account = code => rows.find(r => r.code === code).id;
      return { bank: account('1000'), ar: account('1100'), income: account('4000'), expense: account('6000') };
    }, company);
    pass('built-in-key setup creates the standard chart through the protected creation worker');
    await page.goto(`${base}/lists/customers/new/`);
    await page.fill('#id_display_name', 'Synthetic customer');
    await nav('button:has-text("Save")', /\/lists\/customers\/[0-9a-f-]{36}\/$/);
    ids.customer = /\/customers\/([0-9a-f-]{36})\//.exec(page.url())[1];
    await page.goto(`${base}/lists/items/new/`);
    await page.fill('#id_code', 'SERVICE');
    await page.fill('#id_description', 'Synthetic service');
    await page.selectOption('#id_kind', 'SERVICE');
    await page.fill('#id_sale_rate', '25.00');
    await page.selectOption('#id_income_account', ids.income);
    await nav('button:has-text("Save")', /\/lists\/items\/[0-9a-f-]{36}\/$/);
    ids.item = /\/items\/([0-9a-f-]{36})\//.exec(page.url())[1];
    await page.goto(`${base}/setup/start/`);
    await nav('button:has-text("Fill Accounting setup")', /\/fill\/$/);
    await nav('button:has-text("Use these choices")', /\/setup\/start\/$/);
    pass('customer, service item and designated bank set up through forms');
    await require('./smoke-numbering.cjs')({ page, company, ids, shot, pass });
    assert.deepEqual(errors, []);
    console.log(`NUMBERING FLOW SMOKE OK: ${checks} checks`);
  } finally {
    await app.close();
    console.log(`Synthetic numbering fixture retained at ${data}`);
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
