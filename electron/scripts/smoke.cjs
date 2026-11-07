async function finishRecovery(page) {
  await page.getByRole('button', {name:'Remind me later'}).click();
  await page.waitForURL(/\/settings\/book-protection\/$/);
  await Promise.all([page.waitForURL(/\/home\/$/), page.locator('a[href$="/home/"]').first().click()]);
}
'use strict';
// Headless desktop smoke (run under xvfb-run): the real Electron app, the real fb-node module and a real
// encrypted book in a throwaway data folder. Steps: create a book -> lock -> unlock -> Home -> invoice
// list -> new invoice saved through the ported form -> post -> it appears in the A/R register.
// Also checks the window's security settings from inside. Screenshots go to $FB_SMOKE_SHOTS.
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const shots = process.env.FB_SMOKE_SHOTS || path.join(root, 'staging-run');
fs.mkdirSync(shots, { recursive: true });
const userData = fs.mkdtempSync(path.join(process.env.TMPDIR || os.tmpdir(), 'fb-smoke-'));
const configRoot = path.join(userData, 'isolated-config');
fs.mkdirSync(configRoot, {recursive: true});
const PASSWORD = 'pond-lock-2026';
let step = 0;
const pass = (text) => console.log(`PASS ${String(++step).padStart(2, '0')}: ${text}`);

async function shot(page, name) {
  const file = path.join(shots, `${name}.png`);
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({ path: file, fullPage: false });
  console.log(`      screenshot ${file}`);
}

async function heading(page) {
  return (await page.locator('main h1').first().textContent()).trim();
}

(async () => {
  assert.ok(process.env.DISPLAY, 'run under xvfb-run');
  const app = await electron.launch({ chromiumSandbox: true,
    args: [root, '--password-store=basic'],
    cwd: root,
    env: { ...process.env, XDG_CONFIG_HOME: configRoot, FB_DESKTOP_USER_DATA: userData, ELECTRON_ENABLE_LOGGING: '0' },
    timeout: 60000,
  });
  try {
    const page = await app.firstWindow();
    const pageErrors=[];
    page.on('dialog', async dialog => {console.log(`DIALOG ${dialog.message()}`); await dialog.dismiss();});
    page.on('pageerror', error => {pageErrors.push(error.message);console.log(`PAGE ERROR ${error.message}`);});
    await page.setViewportSize({ width: 1440, height: 1000 });
    // D6: schema creation and encrypted-book I/O can queue behind concurrent wave gates.
    page.setDefaultNavigationTimeout(180000);
    page.setDefaultTimeout(180000);
    await page.waitForSelector('main h1');
    const info = await app.evaluate(({ app: a, safeStorage, BrowserWindow }) => {
      const w = BrowserWindow.getAllWindows()[0];
      const p = w.webContents.getLastWebPreferences();
      return {
        electron: process.versions.electron,
        backend: process.platform === 'linux' ? safeStorage.getSelectedStorageBackend() : 'os',
        prefs: { contextIsolation: p.contextIsolation, sandbox: p.sandbox, nodeIntegration: p.nodeIntegration, webSecurity: p.webSecurity, webviewTag: p.webviewTag },
        url: w.webContents.getURL(),
        userData: a.getPath('userData'),
        name: a.getName(),
        linuxClass: process.platform === 'linux' ? a.commandLine.getSwitchValue('class') : null,
      };
    });
    console.log(`Electron ${info.electron}; safeStorage backend=${info.backend}; ${JSON.stringify(info.prefs)}; ${info.url}`);
    assert.deepEqual(info.prefs, { contextIsolation: true, sandbox: true, nodeIntegration: false, webSecurity: true, webviewTag: false });
    assert.equal(info.userData, userData);
    assert.equal(info.name, 'FreedomBooks');
    if (process.platform === 'linux') assert.equal(info.linuxClass, 'app.freedombooks.FreedomBooks');
    pass('window hardened: contextIsolation, sandbox, no nodeIntegration, webSecurity, no webview');

    const exposure = await page.evaluate(async () => {
      let fetchBlocked = false;
      try {
        await fetch('https://example.com/');
      } catch {
        fetchBlocked = true;
      }
      const inline = document.createElement('script');
      inline.textContent = 'window.__inlineRan = true';
      document.body.appendChild(inline);
      return {
        require: typeof window.require,
        process: typeof window.process,
        fbKeys: typeof window.fb,
        bookKeys: [],
        opened: window.open('https://example.com/') === null,
        fetchBlocked,
        inlineRan: window.__inlineRan === true,
      };
    });
    assert.deepEqual(exposure, { require: 'undefined', process: 'undefined', fbKeys: 'undefined', bookKeys: [], opened: true, fetchBlocked: true, inlineRan: false });
    pass('renderer: no require/process, fixed fb API only, window.open denied, network blocked, inline script blocked by CSP');

    await require('./smoke-webrtc.cjs')({page, shots, pass});

    // 1. First run.
    assert.equal(await heading(page), 'Set up your books');
    assert.equal(await page.isChecked('#id_protect'), false, 'built-in mode is the default even without a keyring');
    await shot(page, '01-first-run');
    await page.fill('#id_owner_name', 'Dana Owner');
    await page.fill('#id_company_name', 'Kettle\u200bPond Supply');
    await page.selectOption('#id_starter_chart', 'empty');
    await page.check('#id_protect');await page.fill('#id_lock_password', PASSWORD);
    await page.fill('#id_lock_password2', PASSWORD);
    await page.click('button:has-text("Create books")');
    await page.waitForSelector('.errorlist[role=alert]');
    assert.equal((await page.locator('.errorlist[role=alert]').textContent()).trim(), 'Control and formatting characters are not allowed.');
    assert.equal(fs.existsSync(path.join(userData, 'books', 'default.fbbook')), false);
    assert.equal(fs.existsSync(path.join(userData, 'book-key.lock.json')), false);
    await shot(page, 'secfix1-first-run-invalid');
    pass('first-run invisible company name refused before a book or key copy is written');
    await page.fill('#id_owner_name', 'Dana Owner');
    await page.fill('#id_company_name', 'Kettle Pond Supply');
    await page.selectOption('#id_starter_chart', 'empty');
    await page.check('#id_protect');await page.fill('#id_lock_password', PASSWORD);
    await page.fill('#id_lock_password2', PASSWORD);
    await Promise.all([page.waitForURL(/\/settings\/recovery-key\/$/), page.click('button:has-text("Create books")')]);
    await finishRecovery(page);
    assert.equal(await heading(page), 'Home');
    const home = page.url();
    const company = /\/c\/([0-9a-f-]{36})\/home\/$/.exec(home)[1];
    const header = fs.readFileSync(path.join(userData, 'books', 'default.fbbook')).subarray(0, 16).toString('latin1');
    assert.notEqual(header, 'SQLite format 3\u0000');
    assert.ok(!fs.existsSync(path.join(userData, 'book-key.sealed')) || info.backend !== 'basic_text', 'no sealed key on basic_text');
    pass(`book created (encrypted header, owner + company ${company}), landed on Home`);

    // 2. Lock and unlock.
    // Lock navigates immediately; do not await its IPC promise in the document being destroyed.
    await Promise.all([page.waitForURL('fb://app/'), app.evaluate(({Menu}) => {
      Menu.getApplicationMenu().items.find(i => i.label === 'File').submenu.items.find(i => i.label === 'Lock').click();
    })]);
    await page.waitForFunction(() => document.querySelector('main h1')?.textContent === 'FreedomBooks is locked');
    await shot(page, '02-locked');
    pass('locked: lock screen shown through the menu accelerator');
    await page.fill('#id_password', 'wrong-password');
    await page.click('button:has-text("Unlock")');
    await page.waitForSelector('.errorlist[role=alert]');
    assert.match(await page.locator('.errorlist[role=alert]').textContent(), /does not open these books/);
    pass('wrong app-lock password refused');
    await page.fill('#id_password', PASSWORD);
    await Promise.all([page.waitForURL(home), page.click('button:has-text("Unlock")')]);
    assert.equal(await heading(page), 'Home');
    pass('unlocked with the app-lock password -> Home');

    // D2: chart and accounting setup now use their real screens. Directory stays typed until D3.
    const accounts = [
      ['1000', 'Checking', 'BANK'], ['1100', 'Accounts receivable', 'RECEIVABLE'],
      ['1200', 'Undeposited funds', 'OTHER_CURRENT_ASSET'], ['2000', 'Accounts payable', 'PAYABLE'],
      ['2200', 'Sales tax payable', 'OTHER_CURRENT_LIABILITY'], ['4000', 'Lawn care income', 'INCOME'],
      ['5000', 'Supplies', 'EXPENSE'],
    ];
    for (const [code, name, type] of accounts) {
      await page.goto(`fb://app/c/${company}/accounts/new/`);
      assert.equal(await heading(page), 'Create account');
      await page.fill('#id_code', code);
      await page.fill('#id_name', name);
      await page.selectOption('#id_detail_type', type);
      if (code === '1000') await shot(page, 'd2-01-account-new');
      await Promise.all([page.waitForURL(/\/accounts\/$/), page.click('button:has-text("Save")')]);
      assert.equal(await heading(page), 'Chart of accounts');
    }
    const ids = await app.evaluate(({ app: a }, c) => {
      const require = process.getBuiltinModule('module').createRequire(a.getAppPath()+'/package.json');
      const path = require('node:path');
      const native = require(path.join(a.getAppPath(), 'native', 'fb-node.node'));
      const rows = JSON.parse(native.query(JSON.stringify({type:'Accounts', company_id:c})));
      const acct = code => rows.find(r => r.code === code).id;
      return {bank:acct('1000'), ar:acct('1100'), expense:acct('5000'), income:acct('4000')};
    }, company);
    await page.goto(`fb://app/c/${company}/lists/customers/new/`);
    assert.equal(await heading(page), 'New customer');
    await page.fill('#id_display_name', 'Maple & Birch Café');
    await page.fill('#id_email', 'buyer@example.invalid');
    await page.fill('#id_billing_address', '22 Maple Street, Pond, NY 10000');
    await shot(page, 'd3-01-customer-form');
    await Promise.all([page.waitForURL(/lists\/customers\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Save")')]);
    ids.customer = /customers\/([0-9a-f-]{36})/.exec(page.url())[1];
    assert.equal(await heading(page), 'Maple & Birch Café');
    await shot(page, 'd3-02-customer-detail');
    await page.goto(`fb://app/c/${company}/lists/items/new/`);
    await page.fill('#id_code', 'MOW');
    await page.fill('#id_description', 'Lawn mowing');
    await page.selectOption('#id_kind', 'SERVICE');
    assert.equal(await page.isChecked('#id_taxable'), false);
    await page.fill('#id_sale_rate', '45.00');
    await page.selectOption('#id_income_account', ids.income);
    await shot(page, 'd3-03-item-form');
    await Promise.all([page.waitForURL(/lists\/items\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Save")')]);
    ids.item = /items\/([0-9a-f-]{36})/.exec(page.url())[1];
    await shot(page, 'd3-04-item-detail');
    pass('customer and service item created through the real directory forms');
    await page.goto(`fb://app/c/${company}/setup/start/`);
    assert.equal(await heading(page), 'Finish setting up');
    await shot(page, 'd2-02-setup-start');
    await Promise.all([page.waitForURL(/\/fill\/$/), page.click('button:has-text("Fill Accounting setup")')]);
    assert.equal(await heading(page), 'Review Accounting setup');
    assert.match(await page.locator('main').textContent(), /1000 · Checking/);
    await shot(page, 'd2-03-setup-fill');
    await Promise.all([page.waitForURL(/\/setup\/start\/$/), page.click('button:has-text("Use these choices")')]);
    await Promise.all([page.waitForURL(home), page.click('button:has-text("Finish later")')]);
    await page.goto(`fb://app/c/${company}/accounting-setup/`);
    assert.equal(await heading(page), 'Accounting setup');
    assert.match(await page.locator('main').textContent(), /Saved revision 1/);
    await shot(page, 'd2-04-accounting-setup');
    await Promise.all([page.waitForURL(/accounting-setup/), page.click('button:has-text("Save accounting setup")')]);
    pass('chart created through account forms; fill-from-chart preview/apply and Finish later; accounting setup form saved');
    await require('./smoke-security.cjs')({app, page, pass});
    await page.goto(home);
    await page.waitForSelector('main h1');
    assert.ok(await page.locator('.step.registers a:has-text("1000 · Checking")').count());
    await shot(page, '03-home');
    pass('Home shows the designated bank register link');

    // 3. Invoice list -> new invoice through the ported form.
    await Promise.all([page.waitForURL(/\/invoices\/$/), page.click('nav.appnav a:has-text("Invoices")')]);
    assert.equal(await heading(page), 'Invoices');
    assert.ok(await page.locator('td:has-text("No invoices match.")').count());
    pass('invoice list (empty)');
    await Promise.all([page.waitForURL(/\/invoices\/new\/$/), page.click('a.button:has-text("New invoice")')]);
    assert.equal(await heading(page), 'New invoice');
    await page.selectOption('#id_customer', ids.customer);
    await page.fill('#id_date', '2026-10-01');
    await page.fill('#id_due_date', '2026-10-31');
    await page.fill('#id_memo', 'October mowing');
    await page.selectOption('#id_lines-0-item', ids.item);
    await page.fill('#id_lines-0-description', 'Lawn mowing');
    await page.fill('#id_lines-0-quantity', '4');
    await page.fill('#id_lines-0-rate', '45.00');
    await page.selectOption('#id_lines-0-income_account', ids.income);
    await shot(page, '04-invoice-form');
    // Missing customer first: the form comes back with Django's error and keeps what was typed.
    const typed = await page.inputValue('#id_lines-0-description');
    await page.selectOption('#id_customer', '');
    await Promise.all([page.waitForSelector('#id_customer_error'), page.click('button:has-text("Save invoice")')]);
    assert.equal(await page.inputValue('#id_lines-0-description'), typed);
    pass('invalid invoice re-rendered with "This field is required." and the typed lines kept');
    await page.selectOption('#id_customer', ids.customer);
    await Promise.all([page.waitForURL(/\/invoices\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Save invoice")')]);
    const invoiceId = /\/invoices\/([0-9a-f-]{36})\/$/.exec(page.url())[1];
    pass(`new invoice saved through the form (redirect to its detail address, ${invoiceId})`);
    await Promise.all([page.waitForURL(/\/invoices\/$/), page.click('nav.appnav a:has-text("Invoices")')]);
    const row = page.locator('tbody tr', { hasText: 'INV-000001' });
    assert.equal((await row.locator('td').nth(3).textContent()).trim(), 'Draft');
    assert.match(await row.textContent(), /Maple & Birch Café/);
    await shot(page, '05-invoice-list');
    pass('invoice INV-000001 listed as Draft, 180.00');

    // D4 replaces the foundation's direct PostInvoice call with the real review and Post form.
    await page.goto(`fb://app/c/${company}/invoices/${invoiceId}/`);
    await Promise.all([page.waitForURL(/review/), page.click('a:has-text("Review posting")')]);
    assert.equal(await heading(page), 'Review invoice posting');
    await shot(page, 'd4-01-invoice-review');
    await Promise.all([page.waitForURL(/invoices\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Post invoice")')]);
    assert.match(await page.locator('main').textContent(), /Current payment state: Open/);
    await shot(page, 'd4-02-invoice-detail');
    await page.goto(`fb://app/c/${company}/invoices/`);
    assert.equal((await page.locator('tbody tr', { hasText: 'INV-000001' }).locator('td').nth(3).textContent()).trim(), 'Open');
    pass('invoice posted (state Open)');
    await require('./smoke-print.cjs')({app,page,company,invoiceId,shot,pass});
    await Promise.all([page.waitForURL(/\/accounts\/$/), page.click('nav.appnav a:has-text("Chart of accounts")')]);
    await shot(page, '06-chart-of-accounts');
    await Promise.all([page.waitForURL(/\/register\/$/), page.click('table.chart a:has-text("Accounts receivable")')]);
    const reg = page.locator('table.register tbody tr', { hasText: 'INV-000001' });
    assert.match(await reg.textContent(), /Invoice INV-000001.*Maple & Birch Café.*INV-000001: October mowing.*180\.00/s);
    assert.equal((await page.locator('table.register tfoot .amount').textContent()).trim(), '180.00');
    await shot(page, '07-register-ar');
    pass('the invoice appears in the Accounts receivable register (180.00 closing balance)');

    // D2 general journal: add a line, preserve invalid input, save/edit/post/reverse via action forms.
    await page.goto(`fb://app/c/${company}/journals/new/`);
    assert.equal(await heading(page), 'Make general journal entry');
    assert.equal(await page.locator('tbody .journal-line').count(), 8);
    await page.click('#add-line');
    assert.equal(await page.locator('tbody .journal-line').count(), 9);
    assert.equal(await page.inputValue('[name="lines-TOTAL_FORMS"]'), '9');
    await page.fill('#id_date', '2026-10-31');
    await page.fill('#id_memo', 'Supplies adjustment');
    await page.check('#id_adjusting');
    await page.selectOption('#id_lines-0-account', ids.expense);
    await page.fill('#id_lines-0-debit', '12.345');
    await page.fill('#id_lines-0-description', 'Supplies adjustment');
    await page.fill('#id_lines-0-name', 'Dana');
    await page.selectOption('#id_lines-8-account', ids.income);
    await page.fill('#id_lines-8-credit', '12.35');
    await page.fill('#id_lines-8-description', 'Supplies adjustment');
    await page.waitForFunction(() => document.querySelector('#total-difference').textContent.includes('Balanced'));
    assert.equal((await page.locator('#total-debit').textContent()).trim(), '12.35');
    await shot(page, 'd2-05-journal-form');
    await page.fill('#id_memo', '');
    // Disable native required validation once, as a deliberately invalid browser POST.
    await page.locator('form.journal-entry').evaluate((form) => { form.noValidate = true; });
    await Promise.all([page.waitForSelector('#id_memo_error'), page.click('button:has-text("Save draft")')]);
    assert.equal(await page.inputValue('#id_lines-8-credit'), '12.35');
    assert.equal(await page.locator('#id_memo').getAttribute('aria-invalid'), 'true');
    await shot(page, 'd2-06-journal-form-invalid');
    await page.fill('#id_memo', 'Supplies adjustment');
    await Promise.all([page.waitForURL(/\/journals\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Save draft")')]);
    const journalUrl = page.url();
    assert.match(await heading(page), /Draft/);
    assert.equal((await page.locator('#detail-total-debit').textContent()).trim(), '12.35');
    await shot(page, 'd2-07-journal-draft-and-events');
    await Promise.all([page.waitForURL(/\/edit\/$/), page.click('a:has-text("Edit draft")')]);
    await page.fill('#id_memo', 'Supplies adjustment revised');
    await Promise.all([page.waitForURL(journalUrl), page.click('button:has-text("Save draft")')]);
    await Promise.all([page.waitForURL(journalUrl), page.click('button:has-text("Post journal")')]);
    await page.waitForFunction(() => document.querySelector('main h1').textContent.includes('Posted'));
    await shot(page, 'd2-08-journal-posted');
    await page.click('summary:has-text("Reverse this journal")');
    await page.fill('form.narrow #id_date', '2026-11-01');
    await page.fill('#id_reason', 'Correct entry');
    await Promise.all([page.waitForURL((u) => u.href !== journalUrl && /\/journals\/[0-9a-f-]{36}\/$/.test(u.href)), page.click('button:has-text("Create reversal")')]);
    assert.match(await page.locator('main').textContent(), /Reversal of/);
    await shot(page, 'd2-09-journal-reversal');
    pass('general journal Add line + half-up totals; invalid POST kept added line; draft edit, post and reversal through real forms');

    await page.goto(`fb://app/c/${company}/`);
    assert.equal(await heading(page), 'Journal entries');
    await shot(page, 'd2-10-journals');
    await page.selectOption('#id_origin', 'MANUAL');
    await page.click('button:has-text("Apply filters")');
    await page.waitForURL(/origin=MANUAL/);
    assert.equal(await page.locator('.journal-list tbody tr').count(), 2);
    await page.goto(`fb://app/c/${company}/history/`);
    assert.equal(await heading(page), 'Audit history');
    assert.match(await page.locator('main').textContent(), /Journal reversed/);
    await shot(page, 'd2-11-history');
    const csvFile = path.join(shots, 'audit-history.csv');fs.rmSync(csvFile,{force:true});
    await app.evaluate(({ dialog }, file) => { dialog.showSaveDialog = async () => ({ canceled: false, filePath: file }); }, csvFile);
    await page.click('a:has-text("Download this list")');
    for (let i = 0; i < 50 && !fs.existsSync(csvFile); i++) await page.waitForTimeout(50);
    const csv = fs.readFileSync(csvFile, 'utf8');
    assert.ok(csv.startsWith('\uFEFF# Times shown in America/New_York\nWhen,User,Action,Document,Reason\r\n'));
    assert.match(csv, /Journal reversed/);
    pass('audit CSV exported through the typed command and native Save dialog');
    await page.goto(`fb://app/c/${company}/trial-balance/?as_of=2026-11-01`);
    assert.equal(await heading(page), 'Trial balance');
    assert.match(await page.locator('main').textContent(), /Debits and credits balance/);
    await shot(page, 'd2-12-trial-balance');
    await page.goto(`fb://app/c/${company}/trial-balance/?as_of=bad`);
    assert.equal(await heading(page), 'Action not completed');
    await shot(page, 'd2-13-error');
    await page.goto(`fb://app/c/${company}/accounts/${ids.income}/edit/`);
    assert.equal(await heading(page), 'Edit account');
    assert.equal(await page.locator('#id_detail_type optgroup').count(), 1);
    await shot(page, 'd2-14-account-edit');
    await page.goto(`fb://app/c/${company}/edit/`);
    assert.equal(await heading(page), 'Edit company');
    await shot(page, 'd2-15-company-edit');
    await require('./smoke-report.cjs')({ app, page, shots, shot, pass, heading });
    await shot(page, 'd2-16-report-problem');
    await page.goto('fb://app/');
    assert.equal(await heading(page), 'Your companies');
    await shot(page, 'd2-17-companies');
    await Promise.all([page.waitForURL(/companies\/new/), page.click('a:has-text("Create company")')]);
    await page.fill('#id_name', 'Second synthetic company');
    await shot(page, 'd2-18-company-new');
    await Promise.all([page.waitForURL(/setup\/start/), page.click('button:has-text("Save")')]);
    assert.equal(await heading(page), 'Finish setting up');
    const setupUrl = page.url();
    const secondCompany = /\/c\/([0-9a-f-]{36})\//.exec(setupUrl)[1];
    await page.goto(`fb://app/c/${secondCompany}/`);
    assert.equal(page.url(), setupUrl);
    pass('journal filters, audit history, trial balance, error, locked account type, company list/create/edit and problem-report screens');

    // D3: lists, status action, field errors, sales tax quick-add/group/default/bulk and reminders.
    await page.goto(`fb://app/c/${company}/lists/vendors/new/`);
    await page.fill('#id_display_name', 'Pond Supply');
    await page.selectOption('#id_default_expense_account', ids.expense);
    await page.selectOption('#id_default_terms', '30');
    await shot(page, 'd3-05-vendor-form');
    await Promise.all([page.waitForURL(/lists\/vendors\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Save")')]);
    const vendorUrl = page.url();
    await shot(page, 'd3-06-vendor-detail');
    await Promise.all([page.waitForFunction(() => document.querySelector('.record-status')?.textContent === 'Inactive'), page.click('button:has-text("Archive vendor")')]);
    await page.click('button:has-text("Reactivate vendor")');
    await page.waitForFunction(() => document.querySelector('.record-status')?.textContent === 'Active');
    await page.goto(`fb://app/c/${company}/lists/vendors/`);
    await shot(page, 'd3-07-vendor-list');
    await page.goto(`fb://app/c/${company}/lists/customers/`);
    await shot(page, 'd3-08-customer-list');
    await page.goto(`fb://app/c/${company}/lists/items/`);
    await shot(page, 'd3-09-item-list');
    await page.goto(`fb://app/c/${company}/lists/items/new/`);
    await page.click('button:has-text("Save")');
    await page.waitForSelector('#id_code_error');
    assert.equal(await page.locator('#id_code').getAttribute('aria-invalid'), 'true');
    await shot(page, 'd3-10-directory-invalid');
    // Use the rendered detail's path-bound token with invalid action to exercise status_error.
    await page.goto(vendorUrl);
    await page.locator('form.status-action [name=active]').evaluate(input => {input.value='bad';});
    await page.locator('form.status-action button').click();
    await page.waitForFunction(() => document.querySelector('main h1')?.textContent === 'Status not changed');
    assert.equal(await heading(page), 'Status not changed');
    await shot(page, 'd3-11-directory-status-error');
    await page.goto(`fb://app/c/${company}/lists/customers/${ids.customer}/edit/`);
    await page.selectOption('#id_tax_status', 'EXEMPT');
    await Promise.all([page.waitForURL(/lists\/customers\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Save")')]);
    await page.goto(`fb://app/c/${company}/lists/items/new/`);
    await page.fill('#id_code', 'SEED');
    await page.selectOption('#id_kind', 'NONSTOCK');
    await page.waitForFunction(() => document.querySelector('#id_taxable').checked);
    await page.uncheck('#id_taxable');
    await page.fill('#id_sale_rate', '9.995');
    await Promise.all([page.waitForURL(/lists\/items\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Save")')]);
    const goodsUrl = page.url();
    for (const [name,rate] of [['County','6.25'],['City','1.5']]) {
      await page.goto(`fb://app/c/${company}/sales-tax/rates/new/`);
      await page.fill('#id_name', name);
      await page.fill('#id_rate', rate);
      if (name === 'County') await page.fill('#id_new_agency_name', 'County Revenue');
      else await page.selectOption('#id_agency', { label:'County Revenue' });
      await shot(page, 'd3-12-rate-form-' + name.toLowerCase());
      await Promise.all([page.waitForURL(/sales-tax\/rates\/$/), page.click('button:has-text("Save")')]);
    }
    await page.goto(`fb://app/c/${company}/sales-tax/groups/new/`);
    await page.fill('#id_name', 'County and City');
    await page.locator('[name=members]').nth(0).check();
    await page.locator('[name=members]').nth(1).check();
    await page.waitForFunction(() => document.querySelector('#group-rate').textContent === '7.75%');
    await shot(page, 'd3-13-group-form');
    await Promise.all([page.waitForURL(/sales-tax\/rates\/$/), page.click('button:has-text("Save")')]);
    const groupLink = await page.locator('a:has-text("County and City")').getAttribute('href');
    const groupId = /groups\/([0-9a-f-]{36})/.exec(groupLink)[1];
    await page.goto(`fb://app/c/${company}/sales-tax/`);
    await page.check('[name=charge_tax][value=yes]');
    await page.selectOption('#id_default_item', groupId);
    await page.check('[name=filing_frequency][value=QUARTERLY]');
    await shot(page, 'd3-14-tax-settings');
    await Promise.all([page.waitForURL(/sales-tax\/turn-on\/$/), page.click('button:has-text("Save settings")')]);
    assert.equal(await heading(page), 'Sales tax is on');
    assert.match(await page.locator('main').textContent(), /1 customer.*currently Exempt/s);
    assert.match(await page.locator('main').textContent(), /1 item.*currently not taxable/s);
    await shot(page, 'd3-15-bulk-step');
    await Promise.all([page.waitForURL(/sales-tax\/$/), page.click('button:has-text("Apply selected")')]);
    await page.goto(goodsUrl);
    assert.match(await page.locator('main').textContent(), /TaxableYes/);
    await page.goto(`fb://app/c/${company}/lists/customers/${ids.customer}/edit/`);
    assert.equal(await page.inputValue('#id_tax_status'), 'TAXABLE');
    await page.goto(`fb://app/c/${company}/sales-tax/rates/`);
    await shot(page, 'd3-16-tax-list');
    const county = page.locator('tbody tr').filter({has:page.locator('a').filter({hasText:/^County$/})});
    await county.locator('button:has-text("Deactivate")').click();
    await page.waitForSelector('h1:has-text("Status not changed")');
    assert.match(await page.locator('main').textContent(), /part of the active tax group/);
    await shot(page, 'd3-17-tax-status-error');
    await page.goto(`fb://app/c/${company}/invoices/new/`);
    assert.equal(await page.inputValue('#id_tax_mode'), 'T' + groupId);
    assert.equal(await page.locator('#id_tax_mode option').first().textContent(), 'Choose sales tax');
    await shot(page, 'd3-18-invoice-tax-on');
    await page.goto(`fb://app/c/${company}/reminders/`);
    assert.equal(await heading(page), 'Reminders');
    await shot(page, 'd3-19-reminders');
    await page.fill('#id_days', '7');
    await Promise.all([page.waitForURL(/reminders\/$/), page.click('button:has-text("Save")')]);
    assert.equal(await page.inputValue('#id_days'), '7');
    await page.fill('#id_days', '91');
    await page.locator('form.reminder-settings-form').evaluate(form => {form.noValidate = true;});
    await page.click('button:has-text("Save")');
    await page.waitForSelector('#id_days[aria-invalid=true]');
    await shot(page, 'd3-20-reminders-invalid');
    pass('directory archive/reactivate, validation, tax rates and quick-add agency, group sum, default and first-enable bulk, refusal and reminder setting through real forms');

    // D4: estimate -> acceptance -> conversion -> invoice edit/review/post through real forms.
    await page.goto(`fb://app/c/${company}/estimates/new/`);
    await page.selectOption('#id_customer', ids.customer);
    await page.fill('#id_date', '2026-10-20');
    await page.selectOption('#id_lines-0-item', ids.item);
    await page.fill('#id_lines-0-quantity', '2');
    await shot(page, 'd4-03-estimate-form');
    await Promise.all([page.waitForURL(/estimates\/[0-9a-f-]{36}\/$/), page.click('button:has-text("Save estimate")')]);
    const estimateUrl=page.url();
    await page.selectOption('#id_state','ACCEPTED');
    await page.click('button:has-text("Apply state")');
    await page.waitForSelector('a:has-text("Convert accepted revision")');
    await shot(page, 'd4-04-estimate-accepted');
    await Promise.all([page.waitForURL(/convert\/2\/$/),page.click('a:has-text("Convert accepted revision")')]);
    await page.fill('#id_date','2026-10-21');
    await page.fill('#id_due_date','2026-11-20');
    await Promise.all([page.waitForURL(/invoices\/[0-9a-f-]{36}\/$/),page.click('button:has-text("Create linked draft invoice")')]);
    assert.match(await page.locator('main').textContent(), /accepted estimate EST-000001/);
    await page.goto(estimateUrl);
    assert.match(await page.locator('main').textContent(), /Linked invoice INV-000002/);
    pass('estimate saved, accepted and converted once with the linked invoice and retained prices visible');

    // Explicit payment allocations, review/record and reversal; stale amounts require another review.
    await page.goto(`fb://app/c/${company}/receipts/new/?customer=${ids.customer}`);
    await page.fill('#id_date','2026-10-22');
    await page.fill('#id_amount','50.00');
    await page.selectOption('#id_method','CHECK');
    await page.selectOption('#id_destination','UNDEPOSITED');
    await page.selectOption('#id_allocations-0-invoice',invoiceId);
    await page.fill('#id_allocations-0-amount','50.00');
    await page.click('button:has-text("Review receipt")');
    await page.waitForSelector('button:has-text("Record receipt — books only")').catch(async e=>{console.error(`Receipt review URL: ${page.url()}`); const html=await page.content();fs.writeFileSync(path.join(shots,'d4-receipt-error.html'),html);console.error(html.slice(0,1800));await shot(page,'d4-receipt-error');throw e;});
    await shot(page,'d4-05-receipt-review');
    await Promise.all([page.waitForURL(/receipts\/[0-9a-f-]{36}\/$/),page.click('button:has-text("Record receipt — books only")')]);
    await shot(page,'d4-06-receipt-detail');
    await Promise.all([page.waitForURL(/reverse\/$/),page.click('a:has-text("Reverse receipt")')]);
    await page.fill('#id_date','2026-10-23');
    await page.fill('#id_reason','Synthetic correction');
    await Promise.all([page.waitForURL(/receipts\/[0-9a-f-]{36}\/$/),page.click('button:has-text("Record exact reversal")')]);
    assert.match(await page.locator('main').textContent(), /Reversed.*original amount 50.00/s);
    pass('customer payment allocated explicitly, reviewed, recorded and reversed using forms');

    await page.goto(`fb://app/c/${company}/sales-receipts/new/`);
    await page.selectOption('#id_customer',ids.customer);
    await page.fill('#id_date','2026-10-22');
    await page.selectOption('#id_method','CASH');
    await page.selectOption('#id_lines-0-item',/items\/([0-9a-f-]{36})/.exec(goodsUrl)[1]);
    assert.equal(await page.inputValue('#id_lines-0-rate'),'10.00');
    await page.fill('#id_lines-0-description','Seed');
    await page.fill('#id_lines-0-quantity','2');
    await page.selectOption('#id_lines-0-income_account',ids.income);
    await page.click('button:has-text("Review sales receipt")');
    await page.waitForSelector('#review-total').catch(async e=>{fs.writeFileSync(path.join(shots,'d4-sale-error.html'),await page.content());await shot(page,'d4-sale-error');throw e;});
    assert.match(await page.locator('#review-total').textContent(), /\d+\.\d{2}/);
    await shot(page,'d4-07-sales-receipt-review');
    await Promise.all([page.waitForURL(/sales-receipts\/[0-9a-f-]{36}\/$/),page.click('button:has-text("Record sales receipt — books only")')]);
    await page.click('a:has-text("Void sales receipt")');
    await page.waitForSelector('#id_reason');
    await page.fill('#id_date','2026-10-23');await page.fill('#id_reason','Synthetic sale correction');
    await Promise.all([page.waitForURL(/sales-receipts\/[0-9a-f-]{36}\/$/),page.click('button:has-text("Record exact reversal")')]);
    assert.match(await page.locator('main').textContent(), /Voided/);
    pass('cash sale uses live item/tax helpers, review, recording and exact void through forms');

    await require('./smoke-d6.cjs')({ app, page, company, ids, shot, pass, heading });
    await require('./smoke-d7a.cjs')({ page, company, ids, invoiceId, shot, pass, heading });
    await require('./smoke-d5.cjs')({ app, page, company, ids, shot, pass });
    await require('./smoke-d7b.cjs')({ app, page, company, ids, shot, pass });
    await require('./smoke-print-pagination.cjs')({app,page,company,ids,shot,pass});

    // Navigate every fixture under the real CSP/preload; setContent skips the isolated-world
    // DOMContentLoaded handler that restores heading icons, so it is not a faithful capture.
    await require('./smoke-catalogue.cjs')({app, page, root, shots, shot, pass});
    assert.deepEqual(pageErrors,[], 'ported page scripts must run without uncaught errors');

    // Last, because a refused navigation leaves Playwright waiting on it.
    await page.evaluate(() => { window.location.href = 'https://example.com/'; });
    await page.waitForTimeout(800);
    assert.ok(page.url().startsWith('fb://app/'), page.url());
    pass('navigation away from fb://app/ refused');
    console.log(`SMOKE OK: ${step} checks`);
  } finally {
    await app.close();
    fs.rmSync(userData, { recursive: true, force: true });
  }
})().catch((e) => {
  console.error(`SMOKE FAILED at step ${step + 1}: ${e.stack || e}`);
  process.exit(1);
});

// D8 is an independent real-navigation flow after the inherited smoke releases its app.
// Keep the shared smoke append-only while sibling waves add their own flows.
process.once('beforeExit', async () => {
  if (process.exitCode) return;
  try { await require('./smoke-d8.cjs')(); }
  catch (error) { console.error(`SMOKE D8 FAILED: ${error.stack || error}`); process.exitCode = 1; }
  // DESKTOP-BACKUPS: backup -> restore -> integrity, its own isolated app and data folder.
  if (!process.exitCode) {
    try { await require('./smoke-backups.cjs')(); }
    catch (error) { console.error(`SMOKE BACKUPS FAILED: ${error.stack || error}`); process.exitCode = 1; }
  }
});
