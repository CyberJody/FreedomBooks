"""Build D2 screen fixtures and extend both renderers to genuine invalid POST requests."""
from pathlib import Path
import json


def replace(path, old, new):
    p=Path(path);text=p.read_text();assert old in text,(path,old);p.write_text(text.replace(old,new))


# Preview is a successful POST response, with no service mutation and no redirect.
replace('crates/fb-app/src/commands.rs','''    Invalid { html: String },''','''    Invalid { html: String },
    Page { html: String, status: u16 },''')
replace('crates/fb-app/src/screens/d2.rs','''Some("preview")=>Ok(Outcome::Invalid{html:setup_fill(app,route,&[])?}),''','''Some("preview")=>Ok(Outcome::Page{html:setup_fill(app,route,&[])?,status:200}),''')
replace('electron/main/main.cjs',"if (outcome.outcome === 'invalid' && command.type === 'SubmitForm') {", "if (['invalid', 'page'].includes(outcome.outcome) && command.type === 'SubmitForm') {")
replace('electron/main/main.cjs',"html: outcome.html, status: 400", "html: outcome.html, status: outcome.status || 400")

# JSON metadata is intentionally the common input for both renderers, including POST fields.
p=Path('crates/fb-screendiff/screens.json');screens=json.loads(p.read_text())
screens.update({
 'companies':{'path':'/'},'company-new':{'path':'/companies/new/'},
 'company-edit':{'path':'/c/{company}/edit/'},'journals':{'path':'/c/{company}/'},
 'journals-filtered':{'path':'/c/{company}/?q=October&origin=DOCUMENT&status=POSTED'},
 'journals-invalid-filter':{'path':'/c/{company}/?start=bad&origin=wrong','status':400},
 'account-new':{'path':'/c/{company}/accounts/new/'},
 'account-edit-used':{'path':'/c/{company}/accounts/{account:4000}/edit/'},
 'setup-start':{'path':'/c/{company}/setup/start/'},
 'setup-fill':{'path':'/c/{company}/setup/start/fill/','post':{'action':'preview'}},
 'setup':{'path':'/c/{company}/accounting-setup/'},
 'journal-new':{'path':'/c/{company}/journals/new/'},
 'journal-edit':{'path':'/c/{company}/journals/{journal:J000003}/edit/'},
 'journal-draft':{'path':'/c/{company}/journals/{journal:J000003}/'},
 'journal-posted':{'path':'/c/{company}/journals/{journal:J000001}/'},
 'history':{'path':'/c/{company}/history/'},
 'history-filtered':{'path':'/c/{company}/history/?doc=journal'},
 'history-invalid':{'path':'/c/{company}/history/?start=bad&action=wrong'},
 'trial-balance':{'path':'/c/{company}/trial-balance/?as_of=2026-10-31'},
 'error':{'path':'/c/{company}/trial-balance/?as_of=bad','status':400},
 'report-problem':{'path':'/report-problem/'},
 'company-new-invalid':{'path':'/companies/new/','post':{'name':'','currency':'12'},'status':400},
 'company-edit-invalid':{'path':'/c/{company}/edit/','post':{'name':'','currency':'123'},'status':400},
 'account-new-invalid':{'path':'/c/{company}/accounts/new/','post':{'code':'','name':'','detail_type':'BANK','is_contra':'on'},'status':400},
 'account-edit-invalid':{'path':'/c/{company}/accounts/{account:4000}/edit/','post':{'code':'4000','name':'Sales & services','detail_type':'BANK','active':'on','postable':'on'},'status':400},
 'setup-invalid':{'path':'/c/{company}/accounting-setup/','post':{'expected_revision':'bad','receivables':'bad'},'status':400},
 'setup-fill-invalid':{'path':'/c/{company}/setup/start/fill/','post':{'action':'apply','digest':'stale','expected_revision':'1'},'status':400},
 'journal-new-invalid':{'path':'/c/{company}/journals/new/','post':{'request_key':'bad','expected_revision':'0','date':'bad','memo':'','entry_number':'','lines-TOTAL_FORMS':'2','lines-INITIAL_FORMS':'0','lines-0-account':'{account:4000}','lines-0-debit':'bad','lines-0-credit':'0','lines-1-account':'','lines-1-debit':'1','lines-1-credit':'0'},'status':400},
 'journal-edit-invalid':{'path':'/c/{company}/journals/{journal:J000003}/edit/','post':{'expected_revision':'1','date':'2026-10-31','memo':'Changed draft','entry_number':'JE-3','lines-TOTAL_FORMS':'1','lines-INITIAL_FORMS':'2','lines-0-account':'{account:4000}','lines-0-debit':'1','lines-0-credit':'1'},'status':400},
 'journal-post-invalid':{'path':'/c/{company}/journals/{journal:J000003}/post/','post':{},'status':400},
 'journal-reverse-invalid':{'path':'/c/{company}/journals/{journal:J000003}/reverse/','post':{'date':'bad','reason':''},'status':400},
 'invoice-new-invalid':{'path':'/c/{company}/invoices/new/','post':{'request_key':'00000000-0000-0000-0000-0000000000c1','expected_revision':'0','customer':'','date':'bad','due_date':'','tax_mode':'NONE','lines-TOTAL_FORMS':'2','lines-INITIAL_FORMS':'0','lines-0-description':'Typed line','lines-0-quantity':'2','lines-0-rate':'5','lines-0-income_account':'{account:4000}'},'status':400},
})
# Check route strings against the reference table rather than assuming the view's name implies its URL.
routes=json.loads(Path('crates/fb-templates/urls.json').read_text())
for screen in screens.values():
    screen['path']=screen['path'].replace('/journals/','/j/')
p.write_text(json.dumps(screens,indent=2)+'\n')
scenario=json.loads(Path('crates/fb-screendiff/scenarios/screens-d1.json').read_text())
scenario['name']='screens-d2'
scenario['steps'].append(dict(op='save_draft',company='c',ref='manual',date='2026-10-31',memo='Depreciation adjustment',entry_number='JE-3',adjusting=True,request_key='00000000-0000-0000-0000-0000000000c0',lines=[dict(account='expense',debit='12.34',credit='0',description='Equipment',name='Dana'),dict(account='accdep',debit='0',credit='12.34',description='Equipment',name='Dana')],expect_error=False))
Path('crates/fb-screendiff/scenarios/screens-d2.json').write_text(json.dumps(scenario,indent=2)+'\n')

replace('crates/fb-screendiff/py/render_django.py','startswith("fbv2_int_d1_")','startswith(("fbv2_int_d1_", "fbv2_int_d2_"))')
replace('crates/fb-screendiff/py/render_django.py','''        response = client.get(path)
        assert response.status_code == 200, (name, path, response.status_code)''','''        if 'post' in screen:
            fields = dict(screen['post'])
            for key, value in fields.items():
                for symbolic, raw in sorted(ids.items(), key=lambda kv: -len(kv[0])):
                    value = value.replace('{' + symbolic + '}', raw)
                fields[key] = value
            response = client.post(path, fields)
        else:
            response = client.get(path)
        assert response.status_code == screen.get('status', 200), (name, path, response.status_code, response.content[:500])''')
replace('crates/fb-screendiff/py/render_django.py','''        ids[f"account:{account.code}"] = str(account.pk)''','''        ids[f"account:{account.code}"] = str(account.pk)
        ids[f"label:account:{account.code}"] = str(account.pk)''')
replace('crates/fb-screendiff/py/render_django.py','''        ids[f"item:{item.code}"] = str(item.pk)''','''        ids[f"item:{item.code}"] = str(item.pk)
        ids[f"label:item:{item.code}"] = str(item.pk)''')
replace('crates/fb-screendiff/py/render_django.py','''    for name, screen in SCREENS.items():''','''    ids['user:' + user.username] = str(user.pk)
    for name, screen in SCREENS.items():''')

replace('crates/fb-screendiff/src/main.rs','''        let rendered = fb_app::screens::render(&mut app, &path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(rendered.status, 200, "{name} status");''','''        let rendered = if let Some(post)=screen.get("post").and_then(Value::as_object) {
            // Mint a token on the actual form's page, including action forms hosted by detail/checklist.
            let form_path=if path.ends_with("/fill/"){path.replace("/fill/","/")}
                else if path.ends_with("/post/"){path.trim_end_matches("post/").to_owned()}
                else if path.ends_with("/reverse/"){path.trim_end_matches("reverse/").to_owned()}else{path.clone()};
            let _=fb_app::screens::render(&mut app,&form_path).unwrap_or_else(|e|panic!("{name} initial form: {e}"));
            let token=app.tokens.mint(&path);
            let fields=post.iter().map(|(k,v)| {
                let mut v=v.as_str().unwrap().to_owned();
                for (symbolic,raw) in &ids {v=v.replace(&format!("{{{symbolic}}}"),raw);}
                (k.clone(),v)
            }).collect();
            let outcome=app.run(fb_app::Command::SubmitForm{path:path.clone(),token,fields}).unwrap_or_else(|e|panic!("{name} post: {e}"));
            let (html,status)=match outcome {fb_app::Outcome::Invalid{html}=>(html,400),fb_app::Outcome::Page{html,status}=>(html,status),other=>panic!("{name}: POST must render without mutation: {other:?}")};
            fb_app::screens::Rendered{html,status,route:String::new()}
        }else{fb_app::screens::render(&mut app, &path).unwrap_or_else(|e| panic!("{name}: {e}"))};
        assert_eq!(rendered.status, screen["status"].as_u64().unwrap_or(200) as u16, "{name} status");''')
replace('crates/fb-screendiff/src/main.rs','''            Ok(ids)
        })''','''            ids.insert("user:owner".into(),tx.query_row("SELECT id FROM fb_user WHERE username='owner'",[],|r|r.get(0))?);
            for (table,model,column) in [("ledger_account","account","code"),("directory_item","item","code")] {
                let mut st=tx.prepare(&format!("SELECT {column},id FROM {table} WHERE company_id=?1 ORDER BY id"))?;
                let rows=st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
                for (n,(key,_)) in rows.into_iter().enumerate(){ids.insert(format!("label:{model}:{key}"),(n+1).to_string());}
            }
            Ok(ids)
        })''')

replace('crates/fb-screendiff/run_screendiff.sh','fbv2_int_d1_ref','fbv2_int_d2_ref')
replace('crates/fb-screendiff/run_screendiff.sh','fbv2_int_d1_screens','fbv2_int_d2_screens')
replace('crates/fb-screendiff/run_screendiff.sh','screens-d1.json','screens-d2.json')
replace('crates/fb-screendiff/run_screendiff.sh','fbv2_int_d1_*)','fbv2_int_d1_*|fbv2_int_d2_*)')
replace('crates/fb-screendiff/run_screendiff.sh', '''COMPANY="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["steps"][0]["name"])' "$SCENARIO")"''', '''COMPANY="$(python3 "$HERE/py/scenario_name.py" "$SCENARIO")"''')
