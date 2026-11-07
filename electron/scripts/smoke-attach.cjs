'use strict';
// J-ATTACH: attach a file on a real invoice page. "Attach a file" and Attach post the form's text fields,
// the main process shows the native chooser (stubbed here, as in smoke-backups) and Rust stores the file.
// Chromium's own file chooser must never open. Isolated HOME/XDG and data folder; a scratch book created through desktop forms.
// Run under xvfb-run: node scripts/smoke-attach.cjs
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const root = path.resolve(__dirname, '..');
const PDF = '%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n';

module.exports = async () => {
  assert.ok(process.env.DISPLAY, 'run under xvfb-run');
  const base = process.env.FB_ATTACH_SMOKE_DIR || fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-attach-smoke-'));
  if (process.env.FB_ATTACH_SMOKE_DIR) {
    assert.ok(!fs.existsSync(base), "use a new isolated test directory");
    fs.mkdirSync(base, { recursive: true, mode: 0o700 });
  }
  const shots = process.env.FB_SMOKE_SHOTS || path.join(root, 'staging-run');
  fs.mkdirSync(shots, { recursive: true });
  const home = path.join(base, 'home');
  const data = path.join(base, 'data');
  const picked = path.join(base, 'picked');
  fs.mkdirSync(home, { recursive: true, mode: 0o700 });
  fs.mkdirSync(picked, { recursive: true });
  const pdf = path.join(picked, 'inv1.pdf');
  fs.writeFileSync(pdf, Buffer.from(PDF, 'latin1'));
  const big = path.join(picked, 'big.pdf');
  fs.writeFileSync(big, Buffer.concat([Buffer.from(PDF, 'latin1'), Buffer.alloc(10 * 1024 * 1024, 0x20)]));
  let checks = 0;
  const pass = text => console.log(`PASS ATTACH ${String(++checks).padStart(2, '0')}: ${text}`);
  const shot = async (page, name) => { const f = path.join(shots, `${name}.png`); await page.screenshot({ path: f, fullPage: true }); console.log(`      screenshot ${f}`); };

  const env = { ...process.env, HOME: home, XDG_CONFIG_HOME: path.join(home, '.config'), XDG_DATA_HOME: path.join(home, '.local/share'),
    XDG_CACHE_HOME: path.join(home, '.cache'), XDG_STATE_HOME: path.join(home, '.local/state'), FB_DESKTOP_USER_DATA: data, ELECTRON_ENABLE_LOGGING: '0' };
  const app = await electron.launch({ chromiumSandbox: true, args: [root, '--password-store=basic'], cwd: root, env, timeout: 60000 });
  try {
    const page = await app.firstWindow();
    await page.setViewportSize({ width: 1440, height: 1000 });
    page.setDefaultTimeout(180000);
    page.setDefaultNavigationTimeout(180000);
    const errors = [];
    const alerts = [];
    let chromiumChooser = 0;
    page.on('pageerror', e => errors.push(e.message));
    page.on('dialog', d => { alerts.push(d.message()); d.dismiss().catch(() => {}); });
    page.on('filechooser', () => { chromiumChooser += 1; });
    const chooser = result => app.evaluate(({ dialog }, r) => {
      globalThis.fbAttachAsked = [];
      dialog.showOpenDialog = async (_win, options) => { globalThis.fbAttachAsked.push(options); return r; };
    }, result);
    const asked = () => app.evaluate(() => globalThis.fbAttachAsked);
    const panel = () => page.locator('#attachments');
    const count = async () => Number(/\((\d+)\)/.exec(await page.locator('#attachments .attachments-count').innerText())[1]);

    await page.fill('#id_owner_name', 'Synthetic attachment owner');
    await page.fill('#id_company_name', 'Synthetic attachment company');
    await Promise.all([page.waitForURL(/\/home\/$/), page.getByRole('button', {name:'Create books',exact:true}).click()]);
    const company = /\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    const baseUrl = `fb://app/c/${company}`;
    const income = await app.evaluate(({ app }, c) => {
      const req = process.getBuiltinModule('module').createRequire(app.getAppPath() + '/package.json');
      const native = req(process.env.FB_NATIVE_MODULE || app.getAppPath() + '/native/fb-node.node');
      return JSON.parse(native.query(JSON.stringify({type:'Accounts',company_id:c}))).find(r=>r.code==='4000').id;
    }, company);
    await page.goto(`${baseUrl}/lists/customers/new/`);
    await page.fill('#id_display_name','Synthetic attachment customer');
    await Promise.all([page.waitForURL(/\/customers\/[0-9a-f-]{36}\/$/),page.getByRole('button',{name:'Save',exact:true}).click()]);
    const customer = /\/customers\/([0-9a-f-]{36})\//.exec(page.url())[1];
    await page.goto(`${baseUrl}/invoices/new/`);
    await page.selectOption('#id_customer',customer);
    await page.fill('#id_date','2026-11-06');await page.fill('#id_due_date','2026-12-06');
    await page.fill('#id_lines-0-description','Synthetic attachment service');
    await page.selectOption('#id_lines-0-income_account',income);
    await page.fill('#id_lines-0-quantity','1');await page.fill('#id_lines-0-rate','25.00');
    await Promise.all([page.waitForURL(/\/invoices\/[0-9a-f-]{36}\/$/),page.getByRole('button',{name:'Save invoice',exact:true}).click()]);
    pass('created an isolated synthetic book and invoice through desktop forms');
    const invoice = page.url();
    assert.match(invoice, /\/invoices\/[0-9a-f-]{36}\/$/);
    const before = await count();
    await panel().scrollIntoViewIfNeeded();
    await shot(page, 'attach-01-invoice-before');
    pass(`opened ${invoice.replace(/^.*\/c\//, '/c/')} with ${before} attachment(s)`);

    // "Attach a file" (the file input's label) opens the native chooser; the picked PDF is stored and listed.
    await chooser({ canceled: false, filePaths: [pdf] });
    await Promise.all([page.waitForEvent('load'), page.click('#attachments label.attachments-file')]);
    assert.equal(await count(), before + 1);
    assert.match(await panel().innerText(), /Attached inv1\.pdf\./);
    assert.match(await panel().locator('table.attachments-table').innerText(), /inv1\.pdf\s+PDF document/);
    const options = (await asked())[0];
    assert.deepEqual(options.properties, ['openFile']);
    assert.deepEqual(options.filters[0].extensions, ['pdf', 'png', 'jpg', 'jpeg', 'webp']);
    assert.equal(chromiumChooser, 0, "Chromium's own file chooser opened");
    assert.deepEqual(alerts, []);
    await panel().scrollIntoViewIfNeeded();
    await shot(page, 'attach-02-attached');
    pass('"Attach a file" opened the native chooser (PDF, PNG, JPEG or WebP) and the PDF is listed with "Attached inv1.pdf."');

    // The Attach button does the same; a cancelled chooser stores nothing and says nothing.
    await chooser({ canceled: true, filePaths: [] });
    await Promise.all([page.waitForEvent('load'), page.click('#attachments form.attachments-add button[type=submit]')]);
    assert.equal(await count(), before + 1);
    assert.equal(await panel().locator('.attachments-note').count(), 0);
    assert.equal((await asked()).length, 1);
    pass('Attach opened the chooser too; cancelling stored nothing and showed no note');

    // A file over 10 MB is refused by Rust before it is stored, with the panel's own error note.
    await chooser({ canceled: false, filePaths: [big] });
    await Promise.all([page.waitForEvent('load'), page.click('#attachments form.attachments-add button[type=submit]')]);
    assert.equal(await count(), before + 1);
    assert.match(await panel().locator('p[role=alert]').innerText(), /The file is larger than 10 MB, so it was not attached\./);
    assert.ok(!(await page.content()).includes('has expired'));
    await panel().scrollIntoViewIfNeeded();
    await shot(page, 'attach-03-too-large');
    pass('a 10 MB+ file is refused on the panel ("larger than 10 MB"), no "form has expired"');

    // A FIFO with no writer must be refused, leaving the next real attachment operation usable.
    if (process.platform === 'linux') {
      const fifo = path.join(picked, 'pipe.pdf');
      assert.equal(spawnSync('mkfifo', ['--', fifo]).status, 0);
      await chooser({ canceled:false, filePaths:[fifo] });
      const start = Date.now();
      await Promise.all([page.waitForEvent('load'), page.click('#attachments form.attachments-add button[type=submit]')]);
      assert.ok(Date.now()-start < 10000, 'FIFO refusal must return promptly');
      assert.equal(await count(), before+1);
      assert.match(await panel().locator('p[role=alert]').innerText(), /Choose a file, not a folder/);
      await shot(page, 'attach-04-fifo-refused');
      pass('FIFO without writer refused promptly; attachment count unchanged');
    }

    // Remove keeps working from the same page (its own per-file token).
    const row = panel().locator('tr', { hasText: 'inv1.pdf' });
    await row.locator('summary').click();
    await row.locator('input[name=reason]').fill('Wrong file');
    await Promise.all([page.waitForEvent('load'), row.locator('button:has-text("Remove")').click()]);
    assert.equal(await count(), before);
    assert.match(await panel().innerText(), /Removed inv1\.pdf\. It stays in the audit history and in backups\./);
    pass('removing the attached file works from the invoice page');
    assert.deepEqual(errors, []);
    assert.deepEqual(alerts, []);
    assert.equal(chromiumChooser, 0);
  } finally {
    await app.close();
  }
  console.log(`Synthetic attachment fixture retained at ${base}`);
};

if (require.main === module) module.exports().catch(error => { console.error(`SMOKE ATTACH FAILED: ${error.stack || error}`); process.exitCode = 1; });
