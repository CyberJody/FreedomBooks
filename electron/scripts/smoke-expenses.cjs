'use strict';
// Hardened packaged app, real forms, synthetic books, no provider requests or native test bridge.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const {spawn, execFileSync} = require('node:child_process');
const {setTimeout: delay} = require('node:timers/promises');
const {chromium} = require('playwright');
const {signIn} = require('./smoke-identity.cjs');
const [binary, out, packagedTree] = process.argv.slice(2);
assert.ok(binary && path.isAbsolute(binary) && out && path.isAbsolute(out) && !fs.existsSync(out));
assert.ok(process.env.DISPLAY && ![':0', ':0.0', ':20', ':20.0'].includes(process.env.DISPLAY), 'Use private Xvfb');
fs.mkdirSync(out, {recursive: true, mode: 0o700});
const env = {...process.env, FB_DESKTOP_USER_DATA: path.join(out, 'profile'),
  XDG_CONFIG_HOME: path.join(out, 'config'), XDG_DATA_HOME: path.join(out, 'data'), XDG_CACHE_HOME: path.join(out, 'cache')};
for (const key of ['FB_NATIVE_MODULE', 'NODE_OPTIONS', 'ELECTRON_RUN_AS_NODE']) delete env[key];
let child, browser, page;
const checks = [], errors = [];
const pass = text => { checks.push(text); console.log('PASS ' + text); };
const click = async (name, url) => Promise.all([page.waitForURL(url), page.getByRole('button', {name, exact: true}).click()]);
const shot = name => page.screenshot({path: path.join(out, name + '.png'), fullPage: true});
(async () => {
  try {
    assert.ok(packagedTree && path.isAbsolute(packagedTree), 'Supply the verified package tree');
    execFileSync(process.execPath, [path.resolve(__dirname, '../../packaging/verify-tree.cjs'), packagedTree, packagedTree, process.platform], {stdio: 'inherit'});
    const {extractFile} = await import('../../packaging/node_modules/@electron/asar/lib/asar.js');
    const metadata = JSON.parse(extractFile(path.join(packagedTree, 'resources/app.asar'), 'package.json'));
    assert.equal(metadata.freedomBooksFeatures.simplefin, true);
    pass('Packaged SimpleFIN capability and unpacked native helper hashes are verified');
    const probe = net.createServer();
    await new Promise(r => probe.listen(0, '127.0.0.1', r));
    const port = probe.address().port;
    await new Promise(r => probe.close(r));
    const log = fs.openSync(path.join(out, 'launch.log'), 'wx');
    child = spawn(binary, ['--remote-debugging-address=127.0.0.1', `--remote-debugging-port=${port}`, '--password-store=basic', '--disable-gpu'], {env, stdio: ['ignore', log, log], detached: true});
    fs.closeSync(log);
    for (let n = 0; n < 120; n++) {
      assert.equal(child.exitCode, null, 'App remains running');
      try { browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, {timeout: 1000}); break; }
      catch { await delay(250); }
    }
    assert.ok(browser, 'Packaged app starts');
    const context = browser.contexts()[0];
    page = context.pages().find(p => p.url().startsWith('fb://app/')) || await context.waitForEvent('page');
    page.setDefaultTimeout(60000);
    page.on('pageerror', e => errors.push(e.message));
    await page.waitForSelector('#id_owner_name');
    await page.setViewportSize({width: 1440, height: 1000});
    await page.fill('#id_owner_name', 'Synthetic expense owner');
    await page.fill('#id_company_name', 'Synthetic expense company');
    await click('Create books', /\/desktop\/sign-in\/$/);
    await signIn(page, 'SyntheticExpenseOwner42');
    const company = /\/c\/([^/]+)\//.exec(page.url())[1], base = `fb://app/c/${company}`;
    await page.getByRole('link', {name: 'Expenses', exact: true}).click();
    await page.waitForSelector('[data-expense-total]');
    assert.match(await page.locator('[data-expense-total]').innerText(), /^0\.00 USD$/);
    assert.match(await page.locator('nav.appnav a[aria-current=page]').innerText(), /Expenses/);
    assert.equal(await page.getByRole('link', {name: 'Review bank transactions', exact: true}).count(), 1);
    pass('Expenses sidebar opens the empty workspace with Bank Feeds enabled');
    await page.goto(base + '/setup/start/');
    await click('Fill Accounting setup from my chart', /\/fill\/$/);
    await click('Use these choices', /\/setup\/start\/$/);
    await page.goto(base + '/expenses/');
    await page.getByRole('link', {name: 'Record check / expense', exact: true}).click();
    await page.waitForSelector('#id_bank');
    const bank = await page.locator('#id_bank option').evaluateAll(options => options.find(o => o.value)?.value);
    const expense = await page.locator('#id_lines-0-account option').evaluateAll(options => options.find(o => /^6000\b/.test(o.textContent))?.value);
    assert.ok(bank && expense, 'Standard chart and setup offer bank and expense accounts');
    await page.selectOption('#id_bank', bank);
    await page.locator('[name=payee_kind][value=OTHER]').check();
    await page.fill('#id_payee_name', 'Synthetic stationery supplier');
    await page.check('#id_print_later');
    await page.fill('#id_memo', 'Workspace smoke expense');
    await page.selectOption('#id_lines-0-account', expense);
    await page.fill('#id_lines-0-description', 'Synthetic office supplies');
    await page.fill('#id_lines-0-rate', '12.34');
    await click('Save draft', /\/checks\/[0-9a-f-]{36}\/$/);
    const checkUrl = page.url();
    await page.goto(base + '/expenses/');
    assert.match(await page.locator('[data-expense-total]').innerText(), /^0\.00 USD$/);
    await page.goto(checkUrl);
    await page.getByRole('link', {name: 'Review recording', exact: true}).click();
    await page.waitForSelector('form.bill-post');
    await click('Record check', /\/checks\/[0-9a-f-]{36}\/$/);
    await page.goto(base + '/expenses/');
    assert.match(await page.locator('[data-expense-total]').innerText(), /^12\.34 USD$/);
    assert.match(await page.locator('[data-expense-entry]').innerText(), /Synthetic stationery supplier[\s\S]*Workspace smoke expense[\s\S]*12\.34/);
    await shot('expenses-recorded');
    pass('Existing check workflow records an exact expense once; drafts stay outside totals');
    await page.locator('.expenses-controls [name=q]').fill('stationery');
    await click('Show expenses', /q=stationery/);
    assert.match(await page.locator('[data-expense-total]').innerText(), /^12\.34 USD$/);
    await page.locator('.expenses-controls [name=q]').fill('absent synthetic payee');
    await click('Show expenses', /q=absent/);
    assert.match(await page.locator('[data-expense-total]').innerText(), /^0\.00 USD$/);
    await page.getByRole('link', {name: 'Reset', exact: true}).click();
    await page.waitForURL(base + '/expenses/');
    await page.locator('.expenses-categories tbody a').click();
    await page.waitForURL(/account=/);
    assert.equal(await page.inputValue('[name=account]'), expense);
    await page.selectOption('[name=basis]', 'CASH');
    await click('Show expenses', /basis=CASH/);
    assert.match(await page.locator('[data-expense-total]').innerText(), /^12\.34 USD$/);
    await page.locator('[data-expense-entry] a').click();
    assert.equal(page.url(), checkUrl);
    pass('Search, category drill-down, cash basis and source link preserve the recorded amount');
    await page.goto(base + '/expenses/');
    await page.getByRole('link', {name: 'Review bank transactions', exact: true}).click();
    await page.waitForURL(/\/bank-feeds\/review\/$/);
    await shot('bank-review');
    await page.goto(base + '/bank-feeds/');
    assert.match(await page.locator('main').innerText(), /Bank feeds/);
    await page.goto(base + '/bank-feeds/connect/');
    assert.equal(await page.locator('#id_setup_token').count(), 1);
    assert.match(await page.locator('main').innerText(), /SimpleFIN Bridge/);
    await shot('simplefin-setup');
    pass('SimpleFIN bank list, transaction review and setup form are present in the packaged build');
    assert.deepEqual(await page.evaluate(() => ({require: typeof window.require, process: typeof window.process, fb: typeof window.fb})), {require: 'undefined', process: 'undefined', fb: 'undefined'});
    assert.deepEqual(errors, []);
    fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify({binary, synthetic: true, production_profile_accessed: false, provider_calls: false, checks}, null, 2));
  } catch (error) {
    if (page && !page.isClosed()) { console.error(page.url(), await page.locator('body').innerText()); await shot('failure'); }
    throw error;
  } finally {
    if (browser) await browser.close();
    if (child && child.exitCode === null) {
      try { process.kill(-child.pid, 'SIGTERM'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
      for (let n = 0; n < 100 && child.exitCode === null; n++) await delay(100);
      if (child.exitCode === null) { try { process.kill(-child.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; } }
    }
  }
})().catch(error => {console.error(error); process.exitCode = 1;});
