"""Integrate D7b with SEC-FIX1's unchanged renderer command allowlist."""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

def replace(name, old, new):
    p = ROOT / name
    text = p.read_text()
    if new in text:
        return
    assert text.count(old) == 1, (name, old)
    p.write_text(text.replace(old, new))

replace('tools/d7b_hand_fixes.py',
        "        ('{{ csrf_token }}', '{{ csrf_token|safe }}'),",
        "        ('{{ csrf_token }}', '{{ csrf_token|safe }}'),\n"
        "        ('>CSV</a>', '>CSV{{ export_token|safe }}</a>'),")
replace('crates/fb-app/src/screens/d7b.rs',
        '    Ok((draw(app, "banking/reconcile/report.html", &vm)?, 200))',
        '    vm["export_token"] = json!(token(app, &rec_url("reconcile:report", c, key)));\n'
        '    Ok((draw(app, "banking/reconcile/report.html", &vm)?, 200))')
replace('crates/fb-app/src/screens/d7b.rs',
        '    vm["csrf_token"] = json!(token(app, &rec_url("registers:cleared", c, aid)));',
        '    vm["export_token"] = json!(token(app, &rec_url("register", c, aid)));\n'
        '    vm["csrf_token"] = json!(token(app, &rec_url("registers:cleared", c, aid)));')
replace('crates/fb-app/src/screens/d7b.rs',
        '    if !matches!(\n        r.name.as_str(),\n        "registers:cleared"',
        '''    // CSV links submit a generated, path-bound token through the narrowed bridge.
    // Route arguments select the company/document; submitted filters cannot change them.
    if r.name == "register" || r.name == "reconcile:report" {
        let (id, filename) = if r.name == "register" {
            (&r.args["account_id"], "bank-register.csv")
        } else {
            (&r.args["reconciliation_id"], "reconciliation.csv")
        };
        return export(app, &r.name, &c.to_db(), id, p, filename);
    }
    if !matches!(
        r.name.as_str(),
        "registers:cleared"''')
replace('electron/preload/preload.cjs',
        "    const command = bank ? {type:'ExportBankRegister', company_id:bank[1], account_id:bank[2]} : {type:'ExportReconciliation', company_id:rec[1], reconciliation_id:rec[2]};\n    command.filters = Array.from(target.searchParams.entries());\n    await call('books.run', command);",
        "    const token = anchor.querySelector('input[name=fb_command_token]').value;\n"
        "    await call('books.run', {type:'SubmitForm', path:target.pathname, token, fields:Array.from(target.searchParams.entries())});\n"
        "    window.location.reload();")

# Keep the inherited smoke's entire workflow, adopting the hardened runner's boundary checks.
legacy = ROOT / 'electron/scripts/smoke.cjs'
legacy.write_text((ROOT / 'electron/scripts/smoke-secfix1.cjs').read_text())
for runner in ['smoke.cjs', 'smoke-secfix1.cjs']:
    replace('electron/scripts/' + runner,
            "    await require('./smoke-d5.cjs')({ app, page, company, ids, shot, pass });",
            "    await require('./smoke-d5.cjs')({ app, page, company, ids, shot, pass });\n"
            "    await require('./smoke-d7b.cjs')({ app, page, company, ids, shot, pass });")
replace('electron/scripts/smoke-d7b.cjs',
        "  const run = command => page.evaluate(c => window.fb.books.run(c), command);\n", '')
replace('electron/scripts/smoke-d7b.cjs',
        "  const result = await run({type:'SaveAccount',company_id:company,code:'1077',name:'Statement checking',detail_type:'BANK'});\n  const bank = result.data.id;",
        """  await go('/accounts/new/');
  await page.fill('#id_code','1077');
  await page.fill('#id_name','Statement checking');
  await page.selectOption('#id_detail_type','BANK');
  await submit('Save',/\\/accounts\\/$/);
  const bank = await page.locator('table.chart a', {hasText:'Statement checking'}).getAttribute('href').then(href=>/accounts\\/([0-9a-f-]{36})/.exec(href)[1]);""")
replace('electron/scripts/smoke-d7b.cjs',
        "    const saved = await run({type:'SaveJournal',company_id:company,journal_id:null,date,memo,entry_number:'',adjusting:false,request_key:null,expected_revision:null,lines});\n    await run({type:'PostJournal',company_id:company,journal_id:saved.data.id,expected_revision:1});",
        """    await go('/journals/new/');
    await page.fill('#id_date',date);
    await page.fill('#id_memo',memo);
    for (const [index,line] of lines.entries()) {
      for (const field of ['account','debit','credit','description','name']) {
        const selector=`#id_lines-${index}-${field}`;
        if(field==='account') await page.selectOption(selector,line[field]);
        else await page.fill(selector,line[field]);
      }
    }
    await submit('Save draft',/\\/journals\\/[0-9a-f-]{36}\\/$/);
    await page.getByRole('button',{name:'Post journal',exact:true}).click();
    await page.waitForFunction(()=>document.querySelector('main h1').textContent.includes('Posted'));""")
