'use strict';
// Find, "+ New" and Back/Forward in the real sandboxed window, on a fresh synthetic book seeded with a
// document of every kind. Run under xvfb with FB_SMOKE_SHOTS set to a durable evidence folder.
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');

const root = path.resolve(__dirname, '..');
const shots = process.env.FB_SMOKE_SHOTS;
assert.ok(shots && path.isAbsolute(shots), 'set FB_SMOKE_SHOTS to durable evidence');
assert.ok(process.env.FB_NATIVE_MODULE, 'set FB_NATIVE_MODULE to electron/native/fb-node.node');
assert.ok(process.env.DISPLAY && ![':0', ':0.0', ':1'].includes(process.env.DISPLAY), 'run under xvfb');
fs.mkdirSync(shots, { recursive: true });
const data = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-find-'));
const PASSWORD = 'SyntheticFindUser42';
let checks = 0;
const pass = text => console.log(`PASS ${++checks}: ${text}`);
const key = () => crypto.randomUUID();

(async () => {
  const app = await electron.launch({ chromiumSandbox: true, args: [root, '--password-store=basic'], cwd: root, timeout: 600000,
    env: { ...process.env, HOME: path.join(data, 'home'), XDG_CONFIG_HOME: path.join(data, 'config'), XDG_DATA_HOME: path.join(data, 'share'), XDG_CACHE_HOME: path.join(data, 'cache'), XDG_STATE_HOME: path.join(data, 'state'), FB_DESKTOP_USER_DATA: data } });
  const errors = [];
  let page;
  try {
    page = await app.firstWindow();
    page.setDefaultTimeout(900000);
    page.setDefaultNavigationTimeout(900000);
    await page.setViewportSize({ width: 1440, height: 900 });
    page.on('pageerror', e => errors.push(e.message));
    page.on('dialog', async d => { errors.push(d.message()); await d.dismiss(); });
    const shot = (name, full = false) => page.screenshot({ path: path.join(shots, `${name}.png`), fullPage: full });
    const settle = () => page.waitForLoadState('load');

    // First run: books, then the owner's password.
    await page.fill('#id_owner_name', 'Riley Sample');
    await page.fill('#id_company_name', 'Harbor Street Bakery');
    await Promise.all([page.waitForURL(/\/desktop\/sign-in\/$|\/home\/$/), page.getByRole('button', { name: 'Create books', exact: true }).click()]);
    if (page.url().endsWith('/desktop/sign-in/')) {
      await page.fill('#id_password', PASSWORD);
      if (await page.locator('#id_password_repeat').count()) await page.fill('#id_password_repeat', PASSWORD);
      if (await page.locator('[name="confirm_owner"]').count()) await page.check('[name="confirm_owner"]');
      await Promise.all([page.waitForURL(/\/(?:home|settings\/recovery-key)\/$/), page.getByRole('button', { name: /^Set password and (sign in|continue)$/ }).click()]);
      if (page.url().endsWith('/settings/recovery-key/')) await Promise.all([page.waitForURL(/\/home\/$/), page.locator('a[href$="/home/"]').first().click()]);
    }
    const company = /\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    const base = `fb://app/c/${company}`;

    // Seed through the app's typed command path (the same commands the forms run), in one call inside the
    // main process: one round trip instead of thirty keeps a heavily loaded machine from stalling it.
    // Test-only shim for an existing defect outside this change: main.cjs's 30-second Stripe refresh timer
    // calls native.appState() unguarded, which throws BUSY while an async native task (the seed below)
    // holds the books, and Electron's uncaught-exception dialog then freezes the main process. Answer
    // BUSY with the last state read instead, as a fix in that timer would.
    await app.evaluate(() => {
      const n = process.mainModule.require(process.env.FB_NATIVE_MODULE);
      const read = n.appState;
      let last = read();
      n.appState = () => {
        try { return (last = read()); } catch (e) { if (String(e.message) === 'BUSY' && last) return last; throw e; }
      };
    });
    const seeded = await app.evaluate(async (_, { company }) => {
      const n = process.mainModule.require(process.env.FB_NATIVE_MODULE);
      const key = () => process.mainModule.require('node:crypto').randomUUID();
      async function run(type, args) {
        console.log(`seed ${type}`);
        const g = JSON.parse(n.aiGrant(company, 'Find smoke seed', 'accounting_write'));
        try {
          const r = JSON.parse(await n.aiRequestAsync(g.token, JSON.stringify({ operation: `command_${type}`, arguments: args })));
          if (r.error || r._agent_error) throw new Error(`${type}: ${JSON.stringify(r)}`);
          return r.data;
        } finally { n.aiRevoke(g.id); }
      }
      const account = async (code, name, detail_type) => (await run('SaveAccount', { code, name, detail_type })).id;
      const bank = await account('1010', 'Harbor checking', 'BANK');
      const savings = await account('1020', 'Harbor savings', 'BANK');
      const ar = await account('1190', 'Receivables', 'RECEIVABLE');
      const uf = await account('1290', 'Undeposited funds', 'OTHER_CURRENT_ASSET');
      const ap = await account('2090', 'Payables', 'PAYABLE');
      const prepaid = await account('2190', 'Customer prepayments', 'OTHER_CURRENT_LIABILITY');
      const accrued = await account('2290', 'Accrued expenses', 'OTHER_CURRENT_LIABILITY');
      const income = await account('4990', 'Catering income', 'INCOME');
      const expense = await account('6990', 'Flour and supplies', 'EXPENSE');
      await run('SaveSetup', { expected_revision: 0, receivables_id: ar, undeposited_id: uf, payables_id: ap, customer_deposits_id: prepaid, bank_ids: [bank, savings] });
      const party = async (kind, name, email, phone) => {
        const values = { kind, display_name: name, business_name: name, contact_name: 'Casey Sample', email, phone, billing_address: '', notes: '' };
        Object.assign(values, kind === 'customer' ? { tax_status: 'TAXABLE', exemption_number: null } : { default_expense_account: null, default_terms: null, expense_shown: false, terms_shown: false });
        return (await run('SaveDirectoryRecord', { record_id: null, values })).id;
      };
      const lakeside = await party('customer', 'Lakeside Cafe', 'orders@lakeside.example', '(555) 010-2233');
      const maple = await party('customer', 'Maple Events', 'events@maple.example', '555-010-4455');
      const flour = await party('vendor', 'Valley Flour Supply', 'billing@valley.example', '555 010 6677');
      const line = (description, rate) => ({ description, quantity: '1', rate, income_account_id: income });
      const invoice = async (customer, date, memo, rate, post) => {
        const id = (await run('SaveInvoice', { values: { request_key: key(), customer_id: customer, date, due_date: date, memo, tax_mode: 'NONE', lines: [line('Catering', rate)] } })).id;
        if (post) await run('PostInvoice', { invoice_id: id, expected_revision: 1 });
        return id;
      };
      const wedding = await invoice(maple, '2026-10-19', 'Wedding cake, three tier', '725.00', true);
      const bread = await invoice(lakeside, '2026-10-05', 'Bread delivery week 40', '84.00', true);
      await invoice(lakeside, '2026-11-09', 'Bread delivery week 45', '84.00', false);
      const reviewed = async (review, record, args) => run(record, { ...args, review_token: (await run(review, args)).token });
      const payment = await reviewed('ReviewCustomerReceipt', 'RecordCustomerReceipt', { sales_receipt: false, values: { request_key: key(), customer_id: lakeside, date: '2026-10-20', method: 'CHECK', reference: 'Check 5521', memo: 'Paid at the counter', destination: 'UNDEPOSITED', bank_id: null, amount: '84.00', allocations: [{ invoice_id: bread, amount: '84.00' }] } });
      await reviewed('ReviewCustomerReceipt', 'RecordCustomerReceipt', { sales_receipt: true, values: { request_key: key(), customer_id: maple, date: '2026-10-22', method: 'CASH', reference: '', memo: 'Farmers market stall', destination: 'BANK', bank_id: bank, tax_mode: 'NONE', lines: [line('Pastries', '42.75')] } });
      await run('SaveEstimate', { estimate_id: null, expected_revision: 0, expiry: null, values: { request_key: key(), customer_id: maple, date: '2026-11-02', memo: 'Anniversary brunch', tax_mode: 'NONE', lines: [line('Brunch for 40', '960.00')] } });
      await reviewed('ReviewCreditMemo', 'RecordCreditMemo', { values: { request_key: key(), customer_id: lakeside, date: '2026-10-25', memo: 'Stale rolls returned', tax_mode: 'NONE', tax_item_id: null, lines: [line('Returned rolls', '12.00')] } });
      await reviewed('ReviewCustomerDeposit', 'RecordCustomerDeposit', { values: { request_key: key(), customer_id: maple, date: '2026-10-28', amount: '300.00', method: 'CHECK', reference: '', destination: 'BANK', bank_id: bank, estimate_id: null, order_reference: 'Holiday order 12', memo: 'Deposit for holiday trays' } });
      const bill = (await run('SaveBill', { values: { bill_id: null, expected_revision: 0, request_key: key(), vendor_id: flour, date: '2026-10-12', due_date: '2026-11-11', memo: 'Flour and sugar, October', reference: 'VF-88123', supersedes_id: null, reentry_reason: '', refresh_vendor: false, keep_archived: false,
        lines: [{ origin_position: null, item_id: null, expense_account_id: expense, description: 'Flour', quantity: '1', rate: '412.37', refresh_item: false, keep_archived: false }] } })).id;
      await run('PostBill', { bill_id: bill, expected_revision: 1, review_token: (await run('ReviewBill', { bill_id: bill, expected_revision: 1 })).token });
      await reviewed('ReviewBillPayment', 'RecordBillPayment', { values: { request_key: key(), vendor_id: flour, date: '2026-10-30', amount: '200.00', method: 'CHECK', reference: '1045', memo: 'Part payment', bank_id: bank, allocations: [{ bill_id: bill, amount: '200.00' }] } });
      const check = (await run('SaveCheck', { values: { check_id: null, expected_revision: 0, request_key: key(), bank_id: bank, date: '2026-10-28', check_number: '1042', print_later: false, payee_kind: 'OTHER', vendor_id: null, payee_name: 'City Power and Light', address: '', memo: 'Electric bill', refresh_payee: false, keep_archived: false,
        lines: [{ tab: 'EXPENSE', origin_position: null, account_id: expense, item_id: null, description: 'Power', quantity: '1', rate: '188.90', refresh_item: false, keep_archived: false }] } })).id;
      await run('RecordCheck', { check_id: check, expected_revision: 1, review_token: (await run('ReviewCheck', { check_id: check, expected_revision: 1 })).token });
      await reviewed('ReviewDeposit', 'RecordDeposit', { values: { request_key: key(), bank_id: bank, date: '2026-10-31', reference: 'Slip 77', memo: 'Counter takings', receipt_ids: [payment.id] } });
      await reviewed('ReviewTransfer', 'RecordTransfer', { values: { request_key: key(), date: '2026-11-03', from_account_id: bank, to_account_id: savings, amount: '500.00', memo: 'Tax reserve' } });
      const journal = (await run('SaveJournal', { journal_id: null, date: '2026-11-04', memo: 'Packaging accrual', entry_number: 'ADJ-7', adjusting: false, request_key: null, expected_revision: null,
        lines: [{ account: expense, debit: '96.15', credit: '0.00', description: 'Boxes', name: 'Paper goods' }, { account: accrued, debit: '0.00', credit: '96.15', description: '', name: '' }] })).id;
      await run('PostJournal', { journal_id: journal, expected_revision: 1 });
      return { wedding, bread, check, journal, maple, income };
    }, { company });
    const { wedding, maple, income } = seeded;
    pass('seeded invoices, a payment, a sales receipt, an estimate, a credit memo, a customer deposit, a bill, a bill payment, a check, a deposit, a transfer and a journal entry');

    await page.goto(`${base}/home/`);
    // Real X key presses (xdotool) go through the window like a person's; Playwright's are checked too.
    // Without a window manager the window has no X focus until it is given it.
    const xwin = String(execFileSync('xdotool', ['search', '--sync', '--onlyvisible', '--name', 'FreedomBooks'])).trim().split('\n')[0];
    const xkey = (...keys) => {
      execFileSync('xdotool', ['windowfocus', '--sync', xwin]);
      execFileSync('xdotool', ['key', '--clearmodifiers', ...keys]);
    };
    const field = page.locator('#fb-find');
    assert.equal(await field.getAttribute('placeholder'), 'Find invoice, bill, customer, amount');
    assert.equal(await page.locator('label[for="fb-find"]').innerText(), 'Find in Harbor Street Bakery: document number, name, amount or memo');
    await page.locator('h1').first().click();
    xkey('ctrl+f');
    await page.waitForFunction(() => document.activeElement && document.activeElement.id === 'fb-find', null, { timeout: 60000 });
    await page.locator('h1').first().click();
    xkey('ctrl+k');
    await page.waitForFunction(() => document.activeElement && document.activeElement.id === 'fb-find', null, { timeout: 60000 });
    pass('Ctrl+F and Ctrl+K focus the labelled Find field');

    const search = async (q, url) => {
      xkey('ctrl+k');
      await page.waitForFunction(() => document.activeElement && document.activeElement.id === 'fb-find', null, { timeout: 60000 });
      await page.keyboard.press('Control+a');
      await page.keyboard.type(q);
      await Promise.all([page.waitForURL(url), page.keyboard.press('Enter')]);
      await settle();
    };
    // An exact number opens the document.
    await search('INV-000001', new RegExp(`/invoices/${wedding}/$`));
    pass('an exact document number and Enter open that invoice');
    await search('lakesi', /\/find\/\?q=lakesi$/);
    const groups = async () => page.$$eval('section.find-group', s => s.map(g => [g.querySelector('h2').firstChild.textContent.trim(), [...g.querySelectorAll('tbody tr')].map(r => r.innerText.replace(/\s+/g, ' ').trim())]));
    let r = await groups();
    assert.deepEqual(r.map(g => g[0]), ['Invoices', 'Credit memos', 'Payments received', 'Customers'], JSON.stringify(r));
    assert.match(r[0][1][0], /^INV-000003 Lakeside Cafe 2026-11-09 84\.00 Draft/);
    for (const [w, h] of [[1440, 900], [1280, 720], [1024, 700]]) {
      await page.setViewportSize({ width: w, height: h });
      await shot(`results-${w}`);
    }
    await page.setViewportSize({ width: 1440, height: 900 });
    pass('a partial customer name finds the customer and their documents, grouped, newest first');
    await search('84.00', /\/find\/\?q=84\.00$/);
    r = await groups();
    assert.equal(r.find(g => g[0] === 'Invoices')[1].length, 2);
    assert.ok(r.find(g => g[0] === 'Deposits'), JSON.stringify(r));
    await shot('results-amount-1440', true);
    pass('an exact amount finds invoices, the payment and the deposit of that amount');
    await search('wedding', /\/find\/\?q=wedding$/);
    r = await groups();
    assert.deepEqual(r, [['Invoices', ['INV-000001 Maple Events 2026-10-19 725.00 Open Wedding cake, three tier']]]);
    pass('a memo word finds the invoice');

    // Open a result, then Back and Forward by every route.
    const historyButtons = () => page.$$eval('button[data-fb-nav]', b => b.map(x => [x.dataset.fbNav, x.disabled]));
    await Promise.all([page.waitForURL(new RegExp(`/invoices/${wedding}/$`)), page.locator('section.find-group a').first().click()]);
    await settle();
    assert.deepEqual(await historyButtons(), [['back', false], ['forward', true]]);
    xkey('alt+Left');
    await page.waitForURL(/\/find\/\?q=wedding$/);
    await settle();
    assert.deepEqual(await historyButtons(), [['back', false], ['forward', false]]);
    xkey('alt+Right');
    await page.waitForURL(new RegExp(`/invoices/${wedding}/$`));
    await settle();
    await page.locator('button[data-fb-nav="back"]').click();
    await page.waitForURL(/\/find\/\?q=wedding$/);
    await settle();
    await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].emit('app-command', {}, 'browser-forward'));
    await page.waitForURL(new RegExp(`/invoices/${wedding}/$`));
    await app.evaluate(({ Menu }) => Menu.getApplicationMenu().getMenuItemById('go-back').click());
    await page.waitForURL(/\/find\/\?q=wedding$/);
    await settle();
    pass('Alt+Left/Right, the title-bar buttons, the mouse back/forward command and Go › Back move through this window\'s pages');
    const go = await app.evaluate(({ Menu }) => Menu.getApplicationMenu().items.map(m => [m.label, m.submenu.items.filter(i => i.type !== 'separator').map(i => i.label)]));
    assert.deepEqual(go.map(m => m[0]), ['File', 'Settings', 'Edit', 'View', 'Go', 'Help']);
    assert.deepEqual(go.find(m => m[0] === 'Go')[1], ['Back', 'Forward', 'Home', 'Find…', 'Companies…']);
    assert.deepEqual(go.find(m => m[0] === 'Help')[1], ['Report a problem…', 'About FreedomBooks']);
    pass('the menu bar has File, Settings, Edit, View, Go and Help');

    // "+ New": open by keyboard, screenshots at three widths, every entry opens its form.
    await page.goto(`${base}/home/`);
    const summary = page.locator('details.titlebar-new > summary');
    await summary.focus();
    await page.keyboard.press('Enter');
    assert.equal(await page.locator('details.titlebar-new').evaluate(d => d.open), true);
    for (const [w, h] of [[1440, 900], [1280, 720], [1024, 700]]) {
      await page.setViewportSize({ width: w, height: h });
      await shot(`new-menu-${w}`);
      const boxes = await page.evaluate(() => Object.fromEntries(['.titlebar-company > summary', '.operator .menu > summary', '#fb-find', 'details.titlebar-new > summary', 'button[data-fb-nav="back"]'].map(s => { const b = document.querySelector(s).getBoundingClientRect(); return [s, [Math.round(b.left), Math.round(b.right), Math.round(b.top), Math.round(b.bottom)]]; })));
      for (const [s, [left, right, top, bottom]] of Object.entries(boxes)) assert.ok(left >= 0 && right <= w && top >= 0 && bottom <= 48, `${w}: ${s} ${JSON.stringify(boxes)}`);
      const menuBox = await page.locator('.new-menu').boundingBox();
      assert.ok(menuBox.x >= 0 && menuBox.x + menuBox.width <= w, `${w}: the menu fits`);
    }
    await page.setViewportSize({ width: 1440, height: 900 });
    pass('the title bar fits at 1440, 1280 and 1024 px with Find, + New, the company and user menus all on screen');
    await page.keyboard.press('Escape');
    assert.equal(await page.locator('details.titlebar-new').evaluate(d => d.open), false);
    assert.equal(await page.evaluate(() => document.activeElement.matches('details.titlebar-new > summary')), true);
    pass('Escape closes "+ New" and returns focus to its button');
    const entries = await page.$$eval('.new-menu a', a => a.map(x => [x.textContent, x.getAttribute('href')]));
    assert.deepEqual(entries.map(e => e[0]), ['Invoice', 'Receive payment', 'Sales receipt', 'Estimate', 'Credit memo', 'Bill', 'Pay bills', 'Write check', 'Deposit', 'Transfer', 'Journal entry']);
    const headings = [];
    for (const [label, href] of entries) {
      await page.locator('details.titlebar-new > summary').click();
      await Promise.all([page.waitForURL(`fb://app${href}`), page.locator('.new-menu a', { hasText: label }).first().click()]);
      await settle();
      headings.push([label, (await page.locator('main h1').first().innerText()).trim(), await page.locator('main form[method=post]').count()]);
    }
    for (const [label, h1, forms] of headings) assert.ok(forms > 0, `${label}: ${h1} has its form`);
    fs.writeFileSync(path.join(shots, 'new-menu-headings.json'), JSON.stringify(headings, null, 1));
    pass(`every "+ New" entry opens its form: ${headings.map(h => `${h[0]} → ${h[1]}`).join('; ')}`);

    // Make an invoice from "+ New", then go Back: the blank form comes back fresh and nothing is sent again.
    const count = () => app.evaluate((_, company) => {
      const n = process.mainModule.require(process.env.FB_NATIVE_MODULE);
      return JSON.parse(n.query(JSON.stringify({ type: 'Invoices', company_id: company }))).length;
    }, company);
    const before = await count();
    await page.goto(`${base}/home/`);
    await page.locator('details.titlebar-new > summary').click();
    await Promise.all([page.waitForURL(/\/invoices\/new\/$/), page.locator('.new-menu a', { hasText: 'Invoice' }).first().click()]);
    await settle();
    page.setDefaultTimeout(120000);
    const token = await page.inputValue('form[method=post] input[name=fb_command_token]');
    await page.selectOption('select[name=customer]', maple);
    await page.fill('input[name=date]', '2026-11-10');
    await page.fill('textarea[name=memo], input[name=memo]', 'Made from the New menu');
    if (!(await page.locator('[name="lines-0-description"]').count())) await page.locator('#add-line, .add-line').first().click();
    await page.fill('[name="lines-0-description"]', 'Rolls');
    await page.fill('[name="lines-0-quantity"]', '2');
    await page.fill('[name="lines-0-rate"]', '9.50');
    if (await page.locator('select[name="lines-0-income_account"]').count()) await page.selectOption('select[name="lines-0-income_account"]', income);
    const saveButton = page.locator('form[method=post] button[type=submit]').filter({ hasText: /Save/ }).first();
    await Promise.all([page.waitForURL(/\/invoices\/[0-9a-f-]{36}\/$/), saveButton.click()]);
    await settle();
    const made = page.url();
    assert.equal(await count(), before + 1);
    await shot('invoice-made-from-new');
    xkey('alt+Left');
    await page.waitForURL(/\/invoices\/new\/$/);
    await settle();
    console.log('after Back the page was loaded as:', await page.evaluate(() => performance.getEntriesByType('navigation')[0]?.type));
    assert.notEqual(await page.inputValue('form[method=post] input[name=fb_command_token]'), token, 'a fresh single-use token');
    assert.equal(await page.locator('[name="lines-0-description"]').count() ? await page.inputValue('[name="lines-0-description"]') : '', '', 'a blank form, not the one submitted');
    xkey('alt+Right');
    await page.waitForURL(made);
    await settle();
    assert.equal(await count(), before + 1, 'Back and Forward sent nothing');
    pass('an invoice made from "+ New"; Back shows a fresh blank form with a new token, and Back/Forward re-submit nothing');

    // Lock (or sign out, for a book without a password) and sign in again: Back does not cross it.
    await page.goto(`${base}/bills/`);
    await page.goto(`${base}/checks/`);
    await app.evaluate(({ Menu }) => Menu.getApplicationMenu().getMenuItemById('lock-books').click());
    await page.waitForURL(/fb:\/\/app\/(?:desktop\/sign-in\/)?$/);
    await settle();
    await shot('after-lock');
    xkey('alt+Left');
    await page.waitForTimeout(1500);
    assert.match(page.url(), /fb:\/\/app\/(?:desktop\/sign-in\/)?$/, 'Back from the lock screen goes nowhere');
    if (await page.locator('input[name=password]').count() && !page.url().endsWith('/desktop/sign-in/')) {
      await page.fill('input[name=password]', PASSWORD);
      await Promise.all([page.waitForURL(/sign-in|home/), page.locator('form[data-gate="unlock"] button').first().click()]);
    }
    if (page.url().endsWith('/desktop/sign-in/')) {
      if (await page.locator('#id_username').count()) await page.fill('#id_username', 'Riley Sample');
      await page.fill('#id_password', PASSWORD);
      await Promise.all([page.waitForURL(/\/home\/$|\/c\/[0-9a-f-]{36}\//), page.getByRole('button', { name: 'Sign in', exact: true }).click()]);
    }
    await settle();
    assert.deepEqual(await historyButtons(), [['back', true], ['forward', true]]);
    const backItem = await app.evaluate(({ Menu }) => Menu.getApplicationMenu().getMenuItemById('go-back').enabled);
    assert.equal(backItem, false);
    const signedIn = page.url();
    xkey('alt+Left');
    await page.waitForTimeout(1500);
    assert.equal(page.url(), signedIn, 'Back after signing in again stays on this page');
    await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].emit('app-command', {}, 'browser-backward'));
    await page.waitForTimeout(1500);
    assert.equal(page.url(), signedIn);
    const entries2 = await app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].webContents.navigationHistory.getAllEntries().map(e => e.url));
    assert.ok(!entries2.some(u => u.includes('/bills/') || u.includes('/checks/')), JSON.stringify(entries2));
    await shot('after-sign-in-again');
    pass('after the lock and signing in again, Back is disabled and the earlier pages are gone from history');
    assert.deepEqual(errors, []);
    pass('no page errors or alerts');
  } catch (e) {
    if (page) {
      await page.screenshot({ path: path.join(shots, 'failure.png'), fullPage: true }).catch(() => {});
      console.error('FAILED at', page.url(), await page.evaluate(() => [document.querySelector('h1')?.innerText, ...[...document.querySelectorAll('.errorlist, .notice')].map(e => e.innerText)]).catch(() => ''));
    }
    throw e;
  } finally {
    await app.close();
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
