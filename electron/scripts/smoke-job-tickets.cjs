'use strict';
// Job tickets and billable time in the real sandboxed Electron window, on a fresh synthetic book:
// open a ticket for "Harbor Lane Tavern", add 3.5h and 1h of "On-Site Support", invoice them as a draft,
// edit the draft (a changed time line is refused; removing the 3.5h line releases exactly that time), void the
// draft and see all the time released. No provider, network or installed profile is used.
const { _electron: electron } = require('playwright');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const shots = process.env.FB_SMOKE_SHOTS;
assert.ok(shots && path.isAbsolute(shots), 'set FB_SMOKE_SHOTS to durable evidence');
assert.ok(process.env.TMPDIR && path.isAbsolute(process.env.TMPDIR), 'set a private TMPDIR');
assert.ok(process.env.DISPLAY && ![':0', ':0.0'].includes(process.env.DISPLAY), 'Use private Xvfb');
fs.mkdirSync(shots, { recursive: true });
const data = fs.mkdtempSync(path.join(process.env.TMPDIR, 'fb-job-tickets-'));
const checks = [];
const pass = text => { checks.push(text); console.log(`PASS ${checks.length}: ${text}`); };
(async () => {
  const app = await electron.launch({ chromiumSandbox: true, args: [root, '--password-store=basic'], cwd: root, timeout: 60000,
    env: { ...process.env, HOME: path.join(data, 'home'), XDG_CONFIG_HOME: path.join(data, 'config'), XDG_DATA_HOME: path.join(data, 'share'), XDG_CACHE_HOME: path.join(data, 'cache'), FB_DESKTOP_USER_DATA: data } });
  let page;
  try {
    page = await app.firstWindow();
    page.setDefaultTimeout(120000);
    page.setDefaultNavigationTimeout(120000);
    await page.setViewportSize({ width: 1440, height: 1000 });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    page.on('dialog', async d => { errors.push(d.message()); await d.dismiss(); });
    const nav = (selector, url) => Promise.all([page.waitForURL(url), page.locator(selector).click()]);
    const shot = name => page.screenshot({ path: path.join(shots, `${name}.png`), fullPage: true });
    const main = () => page.locator('main').innerText();
    await page.fill('#id_owner_name', 'Synthetic job owner');
    await page.fill('#id_company_name', 'Synthetic IT services');
    await nav('button:has-text("Create books")', /\/desktop\/sign-in\/$/);
    await require('./smoke-identity.cjs').signIn(page, 'Synthetic job owner42');
    const company = /\/c\/([0-9a-f-]{36})\//.exec(page.url())[1];
    const base = `fb://app/c/${company}`;
    const income = await app.evaluate(({ app: a }, c) => {
      const require = process.getBuiltinModule('module').createRequire(a.getAppPath() + '/package.json');
      const native = require(require('node:path').join(a.getAppPath(), 'native/fb-node.node'));
      return JSON.parse(native.query(JSON.stringify({ type: 'Accounts', company_id: c }))).find(r => r.code === '4000').id;
    }, company);
    await page.goto(`${base}/lists/customers/new/`);
    await page.fill('#id_display_name', 'Harbor Lane Tavern');
    await nav('button:has-text("Save")', /\/lists\/customers\/[0-9a-f-]{36}\/$/);
    await page.goto(`${base}/lists/items/new/`);
    await page.fill('#id_code', 'On-Site Support');
    await page.fill('#id_description', 'Technician on site');
    await page.selectOption('#id_kind', 'SERVICE');
    await page.fill('#id_sale_rate', '125.00');
    await page.selectOption('#id_income_account', income);
    await nav('button:has-text("Save")', /\/lists\/items\/[0-9a-f-]{36}\/$/);
    await page.goto(`${base}/setup/start/`);
    await nav('button:has-text("Fill Accounting setup")', /\/fill\/$/);
    await nav('button:has-text("Use these choices")', /\/setup\/start\/$/);
    pass('customer "Harbor Lane Tavern", service "On-Site Support" at 125.00 and accounting setup made through forms');

    await page.goto(`${base}/`);
    await nav('nav.appnav a:has-text("Job tickets")', /\/job-tickets\/$/);
    assert.match(await page.locator('nav.appnav a[aria-current=page]').innerText(), /Job tickets/);
    assert.match(await page.locator('nav.page-tabs').innerText(), /Job tickets[\s\S]*Time entries/);
    assert.match(await main(), /No open job tickets/);
    await shot('job-tickets-empty');
    pass('Customers › Job tickets opens from the sidebar with Job tickets and Time entries tabs');

    await nav('.jobs-actions a.button:has-text("New job ticket")', /\/job-tickets\/new\/$/);
    const tavern = await page.locator('#id_customer option').evaluateAll(o => o.find(x => x.textContent.trim() === 'Harbor Lane Tavern')?.value);
    assert.ok(tavern, 'the customer is offered');
    await page.selectOption('#id_customer', tavern);
    await page.fill('#id_title', 'Network outage at the bar');
    await page.fill('#id_description', 'POS terminals offline after a power cut.');
    await nav('button:has-text("Open ticket")', /\/job-tickets\/[0-9a-f-]{36}\/$/);
    const ticketUrl = page.url();
    assert.match(await page.locator('h1').innerText(), /Ticket JT-000001\s+Open/);
    pass('a numbered job ticket JT-000001 is opened for the tavern');

    const support = await page.locator('#id_item option').evaluateAll(o => o.find(x => x.textContent.startsWith('On-Site Support'))?.value);
    await page.selectOption('#id_item', support);
    await page.fill('#id_hours', 'three');
    await nav('button:has-text("Add time")', ticketUrl);
    assert.match(await main(), /Enter hours such as 3\.5/);
    await page.fill('#id_hours', '3.5h');
    await page.fill('#id_notes', 'Replaced the PoE switch');
    const ticketWithDate = new RegExp('^' + ticketUrl.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '(?:\\?date=\\d{4}-\\d{2}-\\d{2})?$');
    await nav('button:has-text("Add time")', ticketWithDate);
    await page.locator('main').getByText('Added 3.50 h of On-Site Support (437.50).').waitFor();
    assert.match(await main(), /Added 3\.50 h of On-Site Support \(437\.50\)/);
    assert.equal(await page.locator('#ticket-hours').innerText(), '3.50');
    assert.equal(await page.locator('#ticket-unbilled').innerText(), '437.50');
    assert.match(await page.locator('tr[data-entry]').innerText(), /On-Site Support[\s\S]*3\.50[\s\S]*125\.00[\s\S]*437\.50[\s\S]*Unbilled/);
    await shot('ticket-with-time');
    pass('3.5h of On-Site Support at the service price shows 437.50 unbilled; a bad hours value is refused on the form');

    await page.selectOption('#id_item', support);
    await page.fill('#id_hours', '1');
    await page.fill('#id_notes', 'Follow-up visit');
    await nav('button:has-text("Add time")', ticketWithDate);
    await page.locator('main').getByText('Added 1.00 h of On-Site Support (125.00).').waitFor();
    assert.equal(await page.locator('#ticket-unbilled').innerText(), '562.50');
    assert.equal(await page.locator('tr[data-entry]').count(), 2);
    pass('a second 1h entry brings the unbilled time to 562.50');

    await nav('a.button:has-text("Create invoice")', /\/invoice\/$/);
    assert.equal(await page.locator('[name=entries]:checked').count(), 2);
    await shot('ticket-invoice-form');
    await nav('button:has-text("Create draft invoice")', /\/invoices\/[0-9a-f-]{36}\/$/);
    const invoiceUrl = page.url();
    const invoiceText = await main();
    assert.match(invoiceText, /Draft invoice INV-\d+ created from ticket JT-000001/);
    assert.match(invoiceText, /Harbor Lane Tavern/);
    assert.match(invoiceText, /562\.50/);
    await shot('draft-invoice');
    pass('Create invoice makes a draft invoice for the tavern totalling 562.50 (two lines) through the invoice service');

    await page.goto(ticketUrl);
    assert.match(await page.locator('h1').innerText(), /Invoiced/);
    for (const row of await page.locator('tr[data-entry]').allInnerTexts()) assert.match(row, /INV-\d+ \(draft\)/);
    assert.equal(await page.locator('a.button:has-text("Create invoice")').count(), 0);
    await page.goto(`${ticketUrl}invoice/`);
    assert.match(await main(), /There is no unbilled billable time/);
    pass('the billed time links to its invoice and cannot be invoiced a second time');

    await page.goto(`${base}/time/`);
    assert.match(await page.locator('nav.page-tabs a[aria-current=page]').innerText(), /Time entries/);
    assert.match((await page.locator('tr[data-entry]').allInnerTexts()).join('\n'), /Harbor Lane Tavern[\s\S]*JT-000001[\s\S]*On-Site Support[\s\S]*3\.50[\s\S]*437\.50/);
    pass('Time entries lists the tavern time with its ticket');

    // Editing the draft: a time line keeps its quantity while it bills time.
    await page.goto(`${invoiceUrl}edit/`);
    assert.match(await page.inputValue('[name="lines-0-quantity"]'), /^3\.50*$/);
    await page.fill('[name="lines-0-quantity"]', '1');
    await page.getByRole('button', { name: 'Save draft', exact: true }).click();
    await page.getByText('bills recorded time from job ticket JT-000001', { exact: false }).waitFor();
    await shot('draft-edit-refused');
    pass('changing the quantity of a time line on the draft is refused, naming the ticket');

    // Removing the 3.5h line releases exactly that time; the 1h stays billed on the draft.
    await page.goto(`${invoiceUrl}edit/`);
    await page.locator('fieldset').nth(0).getByRole('button', { name: 'Remove line', exact: true }).click();
    await Promise.all([page.waitForURL(invoiceUrl), page.getByRole('button', { name: 'Save draft', exact: true }).click()]);
    assert.match(await main(), /125\.00/);
    assert.doesNotMatch(await main(), /562\.50/);
    await page.goto(ticketUrl);
    assert.equal(await page.locator('#ticket-unbilled').innerText(), '437.50');
    const rows = await page.locator('tr[data-entry]').allInnerTexts();
    assert.match(rows.find(r => /3\.50/.test(r)), /Unbilled/);
    assert.match(rows.find(r => /1\.00/.test(r)), /INV-\d+ \(draft\)/);
    assert.match(await main(), /can be invoiced again/);
    assert.match(await main(), /1 entry · 125\.00 USD · Draft/);
    assert.equal(await page.locator('a.button:has-text("Create invoice")').count(), 1);
    await shot('ticket-line-removed');
    pass('removing the 3.5h line from the draft invoice releases that time (437.50 unbilled) and keeps the 1h billed');

    await page.goto(`${invoiceUrl}void/`);
    if (await page.locator('#id_reason').count()) await page.fill('#id_reason', 'Synthetic smoke: invoice again later');
    await nav('button:has-text("Delete draft")', /\/invoices\/[0-9a-f-]{36}\/$/);
    await page.goto(ticketUrl);
    assert.match(await main(), /can be invoiced again/);
    assert.match(await main(), /Voided \(its time was released\)/);
    assert.equal(await page.locator('#ticket-unbilled').innerText(), '562.50');
    for (const row of await page.locator('tr[data-entry]').allInnerTexts()) assert.match(row, /Unbilled/);
    assert.equal(await page.locator('a.button:has-text("Create invoice")').count(), 1);
    await shot('ticket-released');
    pass('deleting the draft invoice through the invoice workflow releases the time for invoicing again');

    assert.deepEqual(await page.evaluate(() => ({ require: typeof window.require, process: typeof window.process })), { require: 'undefined', process: 'undefined' });
    assert.deepEqual(errors, []);
    fs.writeFileSync(path.join(shots, 'result.json'), JSON.stringify({ synthetic: true, production_profile_accessed: false, provider_calls: false, invoice: invoiceUrl.replace(base, ''), checks }, null, 2));
    console.log(`job tickets smoke: ${checks.length} checks passed`);
  } catch (error) {
    if (page && !page.isClosed()) { console.error(page.url(), await page.locator('body').innerText()); await page.screenshot({ path: path.join(shots, 'failure.png'), fullPage: true }); }
    throw error;
  } finally {
    await app.close();
    fs.rmSync(data, { recursive: true, force: true });
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
