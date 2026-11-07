'use strict';
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');

const root = path.resolve(__dirname, '..');
const shots = process.env.FB_SMOKE_SHOTS;
assert.ok(shots && path.isAbsolute(shots), 'set FB_SMOKE_SHOTS to on-disk scratch');
fs.mkdirSync(shots, { recursive: true });
let checks = 0;
const pass = text => console.log(`PASS ${++checks}: ${text}`);
const password = 'starter-chart-test-only';
const companyId = page => /\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];

async function accounts(app, company) {
  return app.evaluate(({ app: a }, c) => {
    const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
    const native = require(require('node:path').join(a.getAppPath(), 'native', 'fb-node.node'));
    return JSON.parse(native.query(JSON.stringify({ type: 'Accounts', company_id: c })));
  }, company);
}
async function shot(page, name) {
  await page.screenshot({ path: path.join(shots, `${name}.png`), fullPage: false });
}
async function run(choice) {
  const data = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-starter-chart-'));
  const app = await electron.launch({ chromiumSandbox: true, args: [root], cwd: root, timeout: 180000,
    env: { ...process.env, HOME: path.join(data, 'home'), XDG_CONFIG_HOME: path.join(data, 'config'), XDG_DATA_HOME: path.join(data, 'data'), XDG_CACHE_HOME: path.join(data, 'cache'), FB_DESKTOP_USER_DATA: data, ELECTRON_ENABLE_LOGGING: '0' } });
  try {
    const page = await app.firstWindow();
    page.setDefaultTimeout(300000);
    page.setDefaultNavigationTimeout(300000);
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.waitForSelector('#id_starter_chart');
    assert.equal(await page.inputValue('#id_starter_chart'), 'standard');
    assert.deepEqual(await page.locator('#id_starter_chart option').allTextContents(), [
      'Standard small-business chart (recommended)', 'Start with an empty chart (for accountants)',
    ]);
    assert.equal(await page.isChecked('#id_protect'), false);
    assert.match(await page.locator('#built-in-warning').innerText(), /Anyone who gets a copy of this file can open it/);
    assert.doesNotMatch(await page.locator('main').innerText(), /encrypted/i);
    const prefs = await app.evaluate(({ BrowserWindow }) => {
      const p = BrowserWindow.getAllWindows()[0].webContents.getLastWebPreferences();
      return { sandbox: p.sandbox, contextIsolation: p.contextIsolation, nodeIntegration: p.nodeIntegration };
    });
    assert.deepEqual(prefs, { sandbox: true, contextIsolation: true, nodeIntegration: false });
    if (choice === 'standard') await shot(page, '01-first-run');
    await page.selectOption('#id_starter_chart', choice);
    if (choice === 'empty') {
      await page.click('button:has-text("Create books")');
      await page.waitForSelector('.errorlist[role=alert]');
      assert.equal(await page.inputValue('#id_starter_chart'), 'empty');
      pass('first-run error retains the explicit empty-chart choice');
    }
    await page.fill('#id_owner_name', 'Morgan Owner');
    await page.fill('#id_company_name', `${choice} synthetic books`);
    await page.check('#id_protect');
    await page.fill('#id_lock_password', password);
    await page.fill('#id_lock_password2', password);
    await Promise.all([page.waitForURL(/\/settings\/recovery-key\/$/), page.click('button:has-text("Create books")')]);
    await page.getByRole('button', { name: 'Remind me later', exact: true }).click();
    await page.waitForURL(/\/settings\/book-protection\/$/);
    await Promise.all([page.waitForURL(/\/home\/$/), page.locator('a[href$="/home/"]').first().click()]);
    const first = companyId(page);
    const original = await accounts(app, first);
    assert.equal(original.length, choice === 'standard' ? 34 : 0);
    pass(`first-run ${choice} chart creates ${original.length} accounts`);
    await page.goto(`fb://app/c/${first}/accounts/`);
    if (choice === 'standard') {
      await shot(page, '02-standard-chart');
      assert.ok(original.some(a => a.name === "Owner's Draw"));
      assert.equal(await page.locator('tr[data-code="3200"] .badge').textContent(), 'Contra');
      assert.ok(!original.some(a => a.name === 'Retained Earnings'));
    } else {
      await page.waitForSelector('text=No accounts yet.');
      await shot(page, '06-empty-first-run-chart');
    }

    if (choice === 'standard') {
      for (const [chart, expected] of [['empty', 0], ['standard', 34]]) {
        await page.goto('fb://app/companies/new/');
        assert.equal(await page.inputValue('#id_starter_chart'), 'standard');
        if (chart === 'empty') await shot(page, '03-new-company');
        await page.fill('#id_name', `${chart} second company`);
        await page.selectOption('#id_starter_chart', chart);
        await Promise.all([page.waitForURL(/\/c\/[0-9a-f-]{36}\//), page.click('button:has-text("Save")')]);
        const created = companyId(page);
        assert.equal((await accounts(app, created)).length, expected);
        assert.deepEqual(await accounts(app, first), original);
        await page.goto(`fb://app/c/${created}/accounts/`);
        await shot(page, chart === 'empty' ? '04-new-empty-chart' : '05-new-standard-chart');
        pass(`new-company ${chart} chart creates ${expected} accounts and leaves existing company intact`);
      }
    }
    await Promise.all([page.waitForURL('fb://app/'), app.evaluate(({ Menu }) => {
      Menu.getApplicationMenu().items.find(i => i.label === 'File').submenu.items.find(i => i.label === 'Lock').click();
    })]);
    await page.waitForSelector('#id_password');
    await page.fill('#id_password', password);
    await Promise.all([page.waitForURL(/\/home\/$/), page.click('button:has-text("Unlock")')]);
    assert.deepEqual(await accounts(app, first), original);
    pass(`reopening ${choice} books leaves the chart unchanged`);
  } finally {
    await app.close();
    console.log(`Synthetic chart fixture retained at ${data}`);
  }
}
(async () => {
  await run('standard');
  await run('empty');
  console.log(`STARTER CHART SMOKE OK: ${checks} checks`);
})().catch(e => { console.error(e); process.exitCode = 1; });
