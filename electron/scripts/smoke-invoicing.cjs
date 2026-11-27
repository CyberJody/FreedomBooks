'use strict';
// Isolated source-desktop acceptance. No provider account or external request is needed.
// Requires an invoice-delivery native build through FB_NATIVE_MODULE and an isolated Xvfb display.
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const password = 'Synthetic invoicing UI password';

async function main() {
  assert.ok(process.env.DISPLAY && ![':0', ':0.0', ':20', ':20.0'].includes(process.env.DISPLAY), 'isolated Xvfb required');
  assert.ok(path.isAbsolute(process.env.FB_NATIVE_MODULE || ''), 'explicit synthetic native candidate required');
  const base = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-invoicing-smoke-'));
  const home = path.join(base, 'home'), data = path.join(base, 'data');
  fs.mkdirSync(home, { recursive: true });
  const shots = process.env.FB_SMOKE_SHOTS || path.join(base, 'screenshots');
  fs.mkdirSync(shots, { recursive: true });
  let desktop, page, company;
  const checks = [];
  const pass = name => { checks.push(name); console.log(`PASS: ${name}`); };
  const native = (method, args = []) => desktop.evaluate((_electron, input) => {
    return process.mainModule.require(input.module)[input.method](...input.args);
  }, { module: process.env.FB_NATIVE_MODULE, method, args });
  async function launch() {
    desktop = await electron.launch({ args: [root, '--password-store=basic'], cwd: root,
      env: { ...process.env, HOME: home, XDG_CONFIG_HOME: path.join(home, 'config'),
        XDG_DATA_HOME: path.join(home, 'share'), XDG_CACHE_HOME: path.join(home, 'cache'),
        FB_DESKTOP_USER_DATA: data, ELECTRON_ENABLE_LOGGING: '0', UV_THREADPOOL_SIZE: '1' }, timeout: 180000 });
    page = await desktop.firstWindow();
    page.setDefaultTimeout(45000);
    page.on('dialog', async dialog => { console.log('UI message:', dialog.message()); await dialog.dismiss(); });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.waitForSelector('h1');
    assert.equal(await native('invoiceDeliveryEnabled'), true);
  }
  const settings = () => page.goto(`fb://app/c/${company}/edit/`);
  try {
    await launch();
    await page.fill('#id_owner_name', 'Synthetic invoice owner');
    await page.fill('#id_company_name', 'Synthetic invoice company');
    await page.selectOption('#id_starter_chart', 'empty');
    await page.getByRole('button', { name: 'Create books', exact: true }).click();
    await page.waitForURL(/\/desktop\/sign-in\/$/);
    await page.fill('#id_password', password);
    await page.fill('#id_password_repeat', password);
    if(await page.locator('input[name="confirm_owner"]').count())await page.check('input[name="confirm_owner"]');
    await page.getByRole('button', { name: 'Set password and continue', exact: true }).click();
    await page.waitForURL(/\/home\/$/);
    company = /\/c\/([^/]+)\//.exec(page.url())[1];
    await settings();
    assert.equal(await page.inputValue('#id_invoice_email_mode'), 'DRAFT');
    assert.equal(await page.inputValue('#id_invoice_stripe_mode'), 'TEST');
    assert.equal(await page.isChecked('#id_invoice_stripe_enabled'), false);
    assert.equal(await page.isChecked('#id_invoice_zelle_enabled'), false);
    pass('providers start disabled; Stripe is TEST and Gmail defaults to drafts');
    await page.check('#id_invoice_zelle_enabled');
    await page.fill('#id_invoice_zelle_name', 'Synthetic seller');
    await page.fill('#id_invoice_zelle_recipient', 'payments@example.test');
    await page.fill('#id_invoice_zelle_instructions', 'Include {invoice_number}.\nThank you.');
    await page.fill('#id_invoice_footer', 'Synthetic footer\nSecond line');
    await page.check('#id_invoice_stripe_enabled');
    await page.getByRole('button', { name: 'Save company settings', exact: true }).click();
    await page.getByText('Company profile saved.', {exact: true}).waitFor();
    await settings();
    assert.equal(await page.inputValue('#id_invoice_zelle_instructions'), 'Include {invoice_number}.\nThank you.');
    assert.equal(await page.inputValue('#id_invoice_footer'), 'Synthetic footer\nSecond line');
    assert.equal(await page.inputValue('#id_invoice_email_mode'), 'DRAFT');
    assert.match(await page.locator('body').innerText(), /keyring/);
    const keyInputs = page.locator('input[name="stripe_key"]');
    assert.deepEqual(await keyInputs.evaluateAll(inputs => inputs.map(i => [i.type, i.value])), [['password', ''], ['password', '']]);
    await page.screenshot({ path: path.join(shots, 'settings.png'), fullPage: true });
    pass('typed settings save multiline payment text and show empty masked credential fields');
    await page.goto(`fb://app/c/${company}/stripe/`);
    assert.match(await page.locator('body').innerText(), /No payments downloaded yet/);
    await page.getByRole('button', { name: 'Refresh payment status', exact: true }).click();
    await page.getByText(/Stripe connections need this computer’s keyring/).waitFor();
    await native('stripeRefreshAsync', [company]);
    const bridge = await page.evaluate(() => ({ keys: Object.keys(window.freedomBooks || {}), require: typeof require }));
    assert.equal(bridge.require, 'undefined');
    assert.ok(!bridge.keys.some(k => /stripeRefresh|invoiceEmailPrint|invoiceEmailFinish/.test(k)));
    await page.screenshot({ path: path.join(shots, 'payments.png'), fullPage: true });
    pass('manual refresh refuses an unusable keyring; background refresh safely skips it and exposes no privileged page methods');
    const error = await desktop.evaluate(async (_electron, input) => {
      const n = process.mainModule.require(input.module), crypto = process.mainModule.require('node:crypto');
      const occupied = new Promise((resolve, reject) => crypto.pbkdf2('synthetic', 'salt', 1000000, 32, 'sha256', e => e ? reject(e) : resolve()));
      const pending = n.stripeRefreshAsync(input.company); n.signOut();
      let message = ''; try { await pending; } catch (e) { message = e.message; }
      await occupied; return message;
    }, { module: process.env.FB_NATIVE_MODULE, company });
    assert.match(error, /active user changed/);
    pass('queued Stripe refresh rejects an ended user session');
    await desktop.close(); desktop = undefined;
    await launch();
    await page.waitForURL(/\/desktop\/sign-in\/$/);
    await page.fill('#id_username', 'Synthetic invoice owner');
    await page.fill('#id_password', password);
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await page.waitForURL(/\/home\/$/);
    await settings();
    assert.equal(await page.inputValue('#id_invoice_zelle_recipient'), 'payments@example.test');
    assert.equal(await page.inputValue('#id_invoice_email_mode'), 'DRAFT');
    pass('invoice settings survive closing and reopening the encrypted synthetic book');
    fs.writeFileSync(path.join(shots, 'verified.json'), JSON.stringify({ checks, base }, null, 2));
  } catch (error) {
    if (page && !page.isClosed()) {
      console.log('Failure URL:', page.url());
      console.log('Failure page:', await page.locator('body').innerText());
      await page.screenshot({ path: path.join(shots, 'failure.png'), fullPage: true });
    }
    throw error;
  } finally { if (desktop) await desktop.close(); }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
