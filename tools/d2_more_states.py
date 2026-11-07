"""Component/CSV parity and service-refusal screen coverage in addition to field errors."""
from pathlib import Path
import json


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


p=Path('crates/fb-screendiff/screens.json');s=json.loads(p.read_text())
s['print-toolbar-note']={'template':'ledger/includes/print_toolbar_note.html','context':{'audience':'internal'},'path':'/help/report/'}
s['history-csv']={'path':'/c/{company}/history/?doc=journal&format=csv','media':'csv'}
s['setup-business-invalid']={'path':'/c/{company}/accounting-setup/','post':{'expected_revision':'1','receivables':'{account:1100}','undeposited':'{account:1100}'},'status':400}
for name,account,revision in [('journal-control-invalid','{account:1100}','0'),('journal-stale-invalid','{account:5000}','0')]:
    editing=name=='journal-stale-invalid'
    s[name]={'path':'/c/{company}/journals/{journal:J000003}/edit/' if editing else '/c/{company}/journals/new/','post':{'request_key':'00000000-0000-0000-0000-0000000000cc','expected_revision':revision,'date':'2026-10-31','memo':'Test refusal','entry_number':'JE-3' if editing else 'Unique number','lines-TOTAL_FORMS':'2','lines-INITIAL_FORMS':'2' if editing else '0','lines-0-account':account,'lines-0-debit':'1','lines-0-credit':'0','lines-1-account':'{account:1590}','lines-1-debit':'0','lines-1-credit':'1'},'status':400}
p.write_text(json.dumps(s,indent=2)+'\n')

replace('crates/fb-screendiff/src/main.rs','''        let rendered = if let Some(post)''','''        let rendered = if let Some(template)=screen["template"].as_str(){
            let mut vm=screen["context"].clone();
            vm["note"]=serde_json::json!(fb_templates::django::print_toolbar_note(vm["audience"].as_str().unwrap(),false));
            fb_app::screens::Rendered{html:app.screens.render(template,&vm).unwrap(),status:200,route:String::new()}
        }else if let Some(post)''')
replace('crates/fb-screendiff/src/main.rs','''        let a = normalize::normalize(&django, &py_map);
        let b = normalize::normalize(&rendered.html, &rs_map);''','''        let normalize=if screen["media"]=="csv" {normalize::normalize_csv}else{normalize::normalize};
        let a = normalize(&django, &py_map);
        let b = normalize(&rendered.html, &rs_map);''')
replace('crates/fb-screendiff/py/render_django.py',"        if 'post' in screen:",'''        if 'template' in screen:
            from django.template.loader import render_to_string
            from django.http import HttpResponse
            response = HttpResponse(render_to_string(screen['template'], screen['context']))
        elif 'post' in screen:''')
replace('electron/main/main.cjs','''    return page(r.html, r.status);''','''    if (r.route === 'history' && url.searchParams.get('format') === 'csv' && r.html.startsWith('\\uFEFF')) {
      return new Response(r.html, { status: 200, headers: { 'content-type': 'text/csv; charset=utf-8', 'content-disposition': 'attachment; filename="audit-history.csv"', 'content-security-policy': guards.CSP, 'cache-control': 'no-store' } });
    }
    return page(r.html, r.status);''')
replace('electron/scripts/smoke.cjs',"    await shot(page, 'd2-11-history');", """    await shot(page, 'd2-11-history');
    const csvFile = path.join(userData, 'audit-history.csv');
    await app.evaluate(({ dialog }, file) => { dialog.showSaveDialog = async () => ({ canceled: false, filePath: file }); }, csvFile);
    await page.click('a:has-text("Download this list")');
    await page.waitForFunction(async () => true);
    for (let i = 0; i < 50 && !fs.existsSync(csvFile); i++) await page.waitForTimeout(50);
    const csv = fs.readFileSync(csvFile, 'utf8');
    assert.ok(csv.startsWith('\\uFEFF# Times shown in America/New_York\\nWhen,User,Action,Document,Reason\\r\\n'));
    assert.match(csv, /Journal reversed/);
    pass('audit CSV exported through the typed command and native Save dialog');""")
replace('electron/scripts/smoke.cjs',"    refused = await page.evaluate(() => window.fb.books.run({ type: 'SubmitForm'", """    // Shared print-note include: capture its real Rust-rendered output, without adding a product route.
    const note = fs.readFileSync(path.join(root, '../target/screendiff/rust/print-toolbar-note.html'), 'utf8');
    await page.setContent(`<html><head><link rel="stylesheet" href="fb://app/static/ledger/app.css"></head><body>${note}</body></html>`);
    await shot(page, 'd2-19-print-toolbar-note');

    refused = await page.evaluate(() => window.fb.books.run({ type: 'SubmitForm'""")
