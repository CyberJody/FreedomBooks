"""Additional OFF, populated-reminder and tax compatibility states, plus include screenshots."""
from pathlib import Path
import json
p=Path('crates/fb-screendiff/screens.json');s=json.loads(p.read_text())
s['reminders-activity']={'path':'/c/{company:Activity synthetic books}/reminders/'}
s['tax-settings-off']={'path':'/c/{company:Other synthetic books}/sales-tax/'}
s['tax-list-off']={'path':'/c/{company:Other synthetic books}/sales-tax/rates/'}
s['tax-bulk-unavailable']={'path':'/c/{company:Other synthetic books}/sales-tax/turn-on/','status':302,'media':'location'}
p.write_text(json.dumps(s,indent=2)+'\n')
p=Path('electron/scripts/smoke.cjs');s=p.read_text();old='    for (const [name,rate] of [[\'County\',\'6.25\'],[\'City\',\'1.5\']]) {';assert old in s
s=s.replace(old,'''    await page.goto(`fb://app/c/${company}/lists/customers/${ids.customer}/edit/`);
    await page.selectOption('#id_tax_status', 'EXEMPT');
    await Promise.all([page.waitForURL(/lists\\/customers\\/[0-9a-f-]{36}\\/$/), page.click('button:has-text("Save")')]);
    await page.goto(`fb://app/c/${company}/lists/items/new/`);
    await page.fill('#id_code', 'SEED');
    await page.selectOption('#id_kind', 'NONSTOCK');
    await page.waitForFunction(() => document.querySelector('#id_taxable').checked);
    await page.uncheck('#id_taxable');
    await page.fill('#id_sale_rate', '9.995');
    await Promise.all([page.waitForURL(/lists\\/items\\/[0-9a-f-]{36}\\/$/), page.click('button:has-text("Save")')]);
    const goodsUrl = page.url();
    for (const [name,rate] of [['County','6.25'],['City','1.5']]) {''')
old='''    await shot(page, 'd3-15-bulk-step');''';assert old in s;s=s.replace(old,'''    assert.match(await page.locator('main').textContent(), /1 customer.*currently Exempt/s);
    assert.match(await page.locator('main').textContent(), /1 item.*currently not taxable/s);
'''+old)
old="    await page.goto(`fb://app/c/${company}/sales-tax/rates/`);\n    await shot(page, 'd3-16-tax-list');";assert old in s;s=s.replace(old,"""    await page.goto(goodsUrl);
    assert.match(await page.locator('main').textContent(), /TaxableYes/);
    await page.goto(`fb://app/c/${company}/lists/customers/${ids.customer}/edit/`);
    assert.equal(await page.inputValue('#id_tax_status'), 'TAXABLE');
    await page.goto(`fb://app/c/${company}/sales-tax/rates/`);
    await shot(page, 'd3-16-tax-list');""")
old='    // Shared print-note include: capture its real Rust-rendered output, without adding a product route.';assert old in s
s=s.replace(old,'''    // The tax-members include has no standalone product route. Capture the real Rust-rendered include.
    for (const mode of ['group','rate']) {
      const fragment = fs.readFileSync(path.join(root, '../target/screendiff/rust/tax-members-' + mode + '.html'), 'utf8');
      await page.setContent(`<html><head><link rel="stylesheet" href="fb://app/static/ledger/app.css"><link rel="stylesheet" href="fb://app/static/salestax/salestax.css"></head><body>${fragment}</body></html>`);
      await shot(page, 'd3-21-tax-members-' + mode);
    }

'''+old)
p.write_text(s)
