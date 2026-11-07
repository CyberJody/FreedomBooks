"""Asserted D2 extension of the real Electron smoke; D1 steps stay in place."""
from pathlib import Path


def replace(path, old, new):
    p=Path(path);text=p.read_text();assert old in text,(path,old);p.write_text(text.replace(old,new))


f='electron/scripts/smoke.cjs'
replace(f,"        userData: a.getPath('userData'),", "        userData: a.getPath('userData'),\n        name: a.getName(),\n        linuxClass: process.platform === 'linux' ? a.commandLine.getSwitchValue('class') : null,")
replace(f,"    assert.equal(info.userData, userData);", "    assert.equal(info.userData, userData);\n    assert.equal(info.name, 'FreedomBooks');\n    if (process.platform === 'linux') assert.equal(info.linuxClass, 'app.freedombooks.FreedomBooks');")
start="    // Lists the D1 screens don't have yet (directory, setup) are filled with typed commands."
end="    let refused = await page.evaluate"
p=Path(f);text=p.read_text();a=text.index(start);b=text.index(end,a)
text=text[:a]+'''    // D2: chart and accounting setup now use their real screens. Directory stays typed until D3.
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
      await Promise.all([page.waitForURL(/\\/accounts\\/$/), page.click('button:has-text("Save")')]);
      assert.equal(await heading(page), 'Chart of accounts');
    }
    const ids = await page.evaluate(async (c) => {
      const rows = await window.fb.books.query({ type: 'Accounts', company_id: c });
      const acct = (code) => rows.find((r) => r.code === code).id;
      const run = (cmd) => window.fb.books.run({ company_id: c, ...cmd }).then((r) => r.data);
      const income = acct('4000');
      const customer = (await run({ type: 'SaveCustomer', display_name: 'Maple & Birch Café' })).id;
      const item = (await run({ type: 'SaveItem', code: 'MOW', description: 'Lawn mowing', kind: 'SERVICE', sale_rate: '45.00', income_account_id: income })).id;
      return { bank: acct('1000'), ar: acct('1100'), expense: acct('5000'), income, customer, item };
    }, company);
    await page.goto(`fb://app/c/${company}/setup/start/`);
    assert.equal(await heading(page), 'Finish setting up');
    await shot(page, 'd2-02-setup-start');
    await Promise.all([page.waitForURL(/\\/fill\\/$/), page.click('button:has-text("Fill Accounting setup")')]);
    assert.equal(await heading(page), 'Review Accounting setup');
    assert.match(await page.locator('main').textContent(), /1000 · Checking/);
    await shot(page, 'd2-03-setup-fill');
    await Promise.all([page.waitForURL(/\\/setup\\/start\\/$/), page.click('button:has-text("Use these choices")')]);
    await Promise.all([page.waitForURL(home), page.click('button:has-text("Finish later")')]);
    await page.goto(`fb://app/c/${company}/accounting-setup/`);
    assert.equal(await heading(page), 'Accounting setup');
    assert.match(await page.locator('main').textContent(), /Saved revision 1/);
    await shot(page, 'd2-04-accounting-setup');
    await Promise.all([page.waitForURL(/accounting-setup/), page.click('button:has-text("Save accounting setup")')]);
    pass('chart created through account forms; fill-from-chart preview/apply and Finish later; accounting setup form saved');
''' + text[b:]
p.write_text(text)
replace(f,"pass('chart, customer, item and accounting setup saved with typed commands; unknown fields refused');","pass('directory fixtures saved with typed commands; unknown fields refused');")
# The old smoke reloaded whichever setup page it was on before checking Home.
replace(f,"    await page.reload();\n    await page.waitForSelector('main h1');", "    await page.goto(home);\n    await page.waitForSelector('main h1');")
insert='''    // D2 general journal: add a line, preserve invalid input, save/edit/post/reverse via action forms.
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
    await Promise.all([page.waitForURL(/\\/journals\\/[0-9a-f-]{36}\\/$/), page.click('button:has-text("Save draft")')]);
    const journalUrl = page.url();
    assert.match(await heading(page), /Draft/);
    assert.equal((await page.locator('#detail-total-debit').textContent()).trim(), '12.35');
    await shot(page, 'd2-07-journal-draft-and-events');
    await Promise.all([page.waitForURL(/\\/edit\\/$/), page.click('a:has-text("Edit draft")')]);
    await page.fill('#id_memo', 'Supplies adjustment revised');
    await Promise.all([page.waitForURL(journalUrl), page.click('button:has-text("Save draft")')]);
    await Promise.all([page.waitForURL(journalUrl), page.click('button:has-text("Post journal")')]);
    await page.waitForFunction(() => document.querySelector('main h1').textContent.includes('Posted'));
    await shot(page, 'd2-08-journal-posted');
    await page.click('summary:has-text("Reverse this journal")');
    await page.fill('form.narrow #id_date', '2026-11-01');
    await page.fill('#id_reason', 'Correct entry');
    await Promise.all([page.waitForURL((u) => u.href !== journalUrl && /\\/journals\\/[0-9a-f-]{36}\\/$/.test(u.href)), page.click('button:has-text("Create reversal")')]);
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
    await page.goto('fb://app/help/report/');
    assert.equal(await heading(page), 'Report a problem');
    await shot(page, 'd2-16-report-problem');
    await page.goto('fb://app/');
    assert.equal(await heading(page), 'Your companies');
    await shot(page, 'd2-17-companies');
    await Promise.all([page.waitForURL(/companies\\/new/), page.click('a:has-text("Create company")')]);
    await page.fill('#id_name', 'Second synthetic company');
    await shot(page, 'd2-18-company-new');
    await Promise.all([page.waitForURL(/setup\\/start/), page.click('button:has-text("Save")')]);
    assert.equal(await heading(page), 'Finish setting up');
    pass('journal filters, audit history, trial balance, error, locked account type, company list/create/edit and problem-report screens');

'''
replace(f,"    refused = await page.evaluate(() => window.fb.books.run({ type: 'SubmitForm'",insert+"    refused = await page.evaluate(() => window.fb.books.run({ type: 'SubmitForm'")
