"""Replace directory fixture commands with UI forms and exercise the D3 lifecycle."""
from pathlib import Path
p=Path('electron/scripts/smoke.cjs');s=p.read_text()
old='''    const ids = await page.evaluate(async (c) => {'''
start=s.index(old);end=s.index('    await page.goto(`fb://app/c/${company}/setup/start/`);',start)
s=s[:start]+'''    const ids = await page.evaluate(async (c) => {
      const rows = await window.fb.books.query({ type: 'Accounts', company_id: c });
      const acct = (code) => rows.find((r) => r.code === code).id;
      return { bank: acct('1000'), ar: acct('1100'), expense: acct('5000'), income: acct('4000') };
    }, company);
    await page.goto(`fb://app/c/${company}/lists/customers/new/`);
    assert.equal(await heading(page), 'New customer');
    await page.fill('#id_display_name', 'Maple & Birch Café');
    await page.fill('#id_email', 'buyer@example.invalid');
    await page.fill('#id_billing_address', '22 Maple Street\\nPond, NY 10000');
    await shot(page, 'd3-01-customer-form');
    await Promise.all([page.waitForURL(/lists\\/customers\\/[0-9a-f-]{36}\\/$/), page.click('button:has-text("Save")')]);
    ids.customer = /customers\\/([0-9a-f-]{36})/.exec(page.url())[1];
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
    await Promise.all([page.waitForURL(/lists\\/items\\/[0-9a-f-]{36}\\/$/), page.click('button:has-text("Save")')]);
    ids.item = /items\\/([0-9a-f-]{36})/.exec(page.url())[1];
    await shot(page, 'd3-04-item-detail');
    pass('customer and service item created through the real directory forms');
''' +s[end:]
s=s.replace("pass('directory fixtures saved with typed commands; unknown fields refused');", "pass('typed command schema still refuses unknown fields');")
marker='    // Shared print-note include: capture its real Rust-rendered output, without adding a product route.'
assert marker in s
flow='''    // D3: lists, status action, field errors, sales tax quick-add/group/default/bulk and reminders.
    await page.goto(`fb://app/c/${company}/lists/vendors/new/`);
    await page.fill('#id_display_name', 'Pond Supply');
    await page.selectOption('#id_default_expense_account', ids.expense);
    await page.selectOption('#id_default_terms', '30');
    await shot(page, 'd3-05-vendor-form');
    await Promise.all([page.waitForURL(/lists\\/vendors\\/[0-9a-f-]{36}\\/$/), page.click('button:has-text("Save")')]);
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
    const badStatus = await page.evaluate(async () => {
      const form = document.querySelector('form.status-action');
      return window.fb.books.run({ type:'SubmitForm', path:new URL(form.action).pathname, token:form.querySelector('[name=fb_command_token]').value, fields:[['active','bad']] });
    });
    assert.equal(badStatus.outcome, 'invalid');
    await page.setContent(badStatus.html);
    assert.equal(await heading(page), 'Status not changed');
    await shot(page, 'd3-11-directory-status-error');
    for (const [name,rate] of [['County','6.25'],['City','1.5']]) {
      await page.goto(`fb://app/c/${company}/sales-tax/rates/new/`);
      await page.fill('#id_name', name);
      await page.fill('#id_rate', rate);
      if (name === 'County') await page.fill('#id_new_agency_name', 'County Revenue');
      else await page.selectOption('#id_agency', { label:'County Revenue' });
      await shot(page, 'd3-12-rate-form-' + name.toLowerCase());
      await Promise.all([page.waitForURL(/sales-tax\\/rates\\/$/), page.click('button:has-text("Save")')]);
    }
    await page.goto(`fb://app/c/${company}/sales-tax/groups/new/`);
    await page.fill('#id_name', 'County and City');
    await page.locator('[name=members]').nth(0).check();
    await page.locator('[name=members]').nth(1).check();
    await page.waitForFunction(() => document.querySelector('#group-rate').textContent === '7.75%');
    await shot(page, 'd3-13-group-form');
    await Promise.all([page.waitForURL(/sales-tax\\/rates\\/$/), page.click('button:has-text("Save")')]);
    const groupLink = await page.locator('a:has-text("County and City")').getAttribute('href');
    const groupId = /groups\\/([0-9a-f-]{36})/.exec(groupLink)[1];
    await page.goto(`fb://app/c/${company}/sales-tax/`);
    await page.check('[name=charge_tax][value=yes]');
    await page.selectOption('#id_default_item', groupId);
    await page.check('[name=filing_frequency][value=QUARTERLY]');
    await shot(page, 'd3-14-tax-settings');
    await Promise.all([page.waitForURL(/sales-tax\\/turn-on\\/$/), page.click('button:has-text("Save settings")')]);
    assert.equal(await heading(page), 'Sales tax is on');
    await shot(page, 'd3-15-bulk-step');
    await Promise.all([page.waitForURL(/sales-tax\\/$/), page.click('button:has-text("Apply selected")')]);
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
    await Promise.all([page.waitForURL(/reminders\\/$/), page.click('button:has-text("Save")')]);
    assert.equal(await page.inputValue('#id_days'), '7');
    await page.fill('#id_days', '91');
    await page.locator('form.reminder-settings-form').evaluate(form => {form.noValidate = true;});
    await page.click('button:has-text("Save")');
    await page.waitForSelector('#id_days[aria-invalid=true]');
    await shot(page, 'd3-20-reminders-invalid');
    pass('directory archive/reactivate, validation, tax rates and quick-add agency, group sum, default and first-enable bulk, refusal and reminder setting through real forms');

'''
s=s.replace(marker,flow+marker)
p.write_text(s)
