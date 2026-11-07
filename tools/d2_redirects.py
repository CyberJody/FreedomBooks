"""Preserve the first-run company's real redirect without changing the fixed bridge shape."""
from pathlib import Path
import json


def replace(path,old,new):
    p=Path(path);t=p.read_text();assert old in t,(path,old);p.write_text(t.replace(old,new))


replace('crates/fb-app/src/screens/d2.rs','''        return Ok((setup_start(app, route)?, 200));''','''        return Ok((url("setup_start", &[&c.to_db()]), 302));''')
replace('crates/fb-app/src/screens/d2.rs','''    // The company page redirects to the checklist until Finish later; the protocol follows redirects.''','''    // A 3xx Rendered carries its Location path in the existing body slot; main follows it locally.''')
replace('crates/fb-app/src/screens.rs','''pub struct Rendered {
    pub html: String,''','''pub struct Rendered {
    /// HTML for a page; for a 3xx response, the local Location path. The bridge shape stays fixed.
    pub html: String,''')
replace('electron/main/main.cjs', '''    if (r.route === 'history' ''','''    if (r.status >= 300 && r.status < 400) {
      const location = new URL(guards.target({ target: r.html }), ORIGIN);
      if (location.protocol !== 'fb:' || location.host !== 'app') throw new Error('Invalid local redirect');
      return Response.redirect(location.href, r.status);
    }
    if (r.route === 'history' ''')
replace('crates/fb-screendiff/py/render_django.py','''            fh.write(response.content)''','''            fh.write(response['Location'].encode() if 300 <= response.status_code < 400 else response.content)''')
replace('crates/fb-screendiff/src/main.rs','''let normalize=if screen["media"]=="csv" {normalize::normalize_csv}else{normalize::normalize};''','''let normalize:fn(&str,&normalize::IdMap)->String=match screen["media"].as_str(){Some("csv")=>normalize::normalize_csv,Some("location")=>normalize::normalize_location,_=>normalize::normalize};''')
p=Path('crates/fb-screendiff/scenarios/screens-d2.json');s=json.loads(p.read_text())
next(step for step in s['steps'] if step.get('ref')=='other')['open_first_run']=True
p.write_text(json.dumps(s,indent=2)+'\n')
p=Path('crates/fb-screendiff/screens.json');s=json.loads(p.read_text());s['journals-needs-setup']={'path':'/c/{company:Other synthetic books}/','status':302,'media':'location'};p.write_text(json.dumps(s,indent=2)+'\n')
replace('electron/scripts/smoke.cjs',"    assert.equal(await heading(page), 'Finish setting up');\n    pass('journal filters", "    assert.equal(await heading(page), 'Finish setting up');\n    const setupUrl = page.url();\n    const secondCompany = /\\/c\\/([0-9a-f-]{36})\\//.exec(setupUrl)[1];\n    await page.goto(`fb://app/c/${secondCompany}/`);\n    assert.equal(page.url(), setupUrl);\n    pass('journal filters")
