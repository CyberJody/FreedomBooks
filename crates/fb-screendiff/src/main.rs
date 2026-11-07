//! `fb-screendiff <scenario.json> <django dir> <out dir>`: apply the scenario to a fresh book with the
//! harness runner, render every screen in `screens.json` with fb-app/fb-templates, normalize both sides
//! (NORMALIZATION.md) and compare. Exit 1 on any difference. The Django side is rendered beforehand by
//! `py/render_django.py` from the same scenario (see `run_screendiff.sh`).
mod normalize;

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use fb_db::rusqlite::OptionalExtension;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: fb-screendiff <scenario.json> <django dir> <out dir>");
        std::process::exit(2);
    }
    let (scenario_path, django_dir, out_dir) = (Path::new(&args[1]), Path::new(&args[2]), Path::new(&args[3]));
    let scenario: Value = fb_db::parse_unique_json(&std::fs::read_to_string(scenario_path).expect("scenario")).expect("scenario json");
    let screens: Value = serde_json::from_str(&std::fs::read_to_string(std::env::var_os("FB_SCREENDIFF_SCREENS").map(std::path::PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("screens.json"))).unwrap()).unwrap();
    std::fs::create_dir_all(out_dir.join("rust")).unwrap();
    // The scenario book is temporary, unlike the retained HTML evidence. Honor the job's on-disk
    // TMPDIR so parallel wave builds do not make every fully durable journal wait on the source disk.
    let book_path = std::env::temp_dir().join(format!("fb-screendiff-{}.fbbook", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", book_path.display()));
    }
    let key = fb_app::keys::new_master_key().unwrap();
    {
        let mut runner = fb_harness::Runner::with_key(&scenario, &book_path, &key);
        runner.run();
        let dump = runner.dump();
        let bad: Vec<&Value> = dump["outcomes"].as_array().unwrap().iter().filter(|o| o["ok"] != true).collect();
        assert!(bad.is_empty(), "scenario steps failed on the Rust side: {bad:?}");
    }
    let mut app = fb_app::App::new();
    app.open_book(&book_path, &key).expect("open scenario book");
    // Fixed version/OS for "Report a problem", so its approved desktop page compares exactly.
    app.host = fb_app::Host::new(normalize::desktop::VERSION, normalize::desktop::OS);
    // DESKTOP-BACKUPS: an empty, not-yet-created backup folder, like the Django side's.
    let data = std::path::absolute(out_dir.join("rust-data")).unwrap();
    let _ = std::fs::remove_dir_all(&data);
    app.backups.configure(&data).unwrap();
    // Both renderers receive the same display date, including inherited form defaults.
    app.today = || {
        let clock: Value = serde_json::from_str(include_str!("../clock.json")).unwrap();
        chrono::DateTime::parse_from_rfc3339(clock["local_noon"].as_str().unwrap()).unwrap().date_naive()
    };
    app.now = || {
        let clock: Value = serde_json::from_str(include_str!("../clock.json")).unwrap();
        chrono::DateTime::parse_from_rfc3339(clock["local_noon"].as_str().unwrap()).unwrap().with_timezone(&chrono::Utc)
    };
    let ids = rust_ids(&mut app, scenario["steps"][0]["name"].as_str().unwrap());
    std::fs::write(out_dir.join("rust/ids.json"), serde_json::to_string_pretty(&ids).unwrap()).unwrap();
    let django_ids: BTreeMap<String, String> = serde_json::from_str(&std::fs::read_to_string(django_dir.join("ids.json")).expect("django ids.json")).unwrap();
    let py_map = normalize::IdMap::new(&django_ids).reference();
    let rs_map = normalize::IdMap::new(&ids);
    let mut failures = 0;
    let mut hidden = 0;
    for (name, screen) in screens.as_object().unwrap() {
        let mut path = screen["path"].as_str().unwrap().to_owned();
        let mut keys: Vec<&String> = ids.keys().collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
        for k in keys {
            path = path.replace(&format!("{{{k}}}"), &ids[k]);
        }
        // N14: a plugin route in a release build (plugins hidden) must be refused, GET and POST.
        let route_name = fb_app::screens::resolve(path.split('?').next().unwrap()).map(|r| r.name);
        if !app.plugins && screen["template"].is_null() && route_name.as_deref().is_some_and(fb_app::screens::plugin_route) {
            let outcome = if screen.get("post").is_some() {
                let token = app.tokens.mint(&path);
                app.run(fb_app::Command::SubmitForm { path: path.clone(), token, fields: vec![] }).map(|_| 200)
            } else {
                fb_app::screens::render(&mut app, &path).map(|r| r.status)
            };
            let outcome = outcome.map_err(|e| match e { fb_app::AppError::NotFound(m) => format!("not found: {m}"), other => other.to_string() });
            match normalize::release::hidden_outcome(outcome) {
                Ok(()) => { hidden += 1; println!("HIDDEN {name} (release build refuses {})", route_name.unwrap()); }
                Err(why) => { failures += 1; println!("DIFF {name}: hidden plugin screen {why}"); }
            }
            continue;
        }
        let rendered = if let Some(template)=screen["template"].as_str(){
            let mut vm=screen["context"].clone();
            if let Some(audience)=vm["audience"].as_str(){vm["note"]=serde_json::json!(fb_templates::django::print_toolbar_note(audience,false));}
            fb_app::screens::Rendered{html:app.screens.render(template,&vm).unwrap(),status:200,route:String::new()}
        }else if let Some(post)=screen.get("post").and_then(Value::as_object) {
            // Mint a token on the actual form's page, including action forms hosted by detail/checklist.
            let form_path = if path.contains("/checks/") && path.ends_with("/record/") {
                path.trim_end_matches("record/").to_owned()
            } else if path.contains("/estimates/") && path.ends_with("/state/") {
                path.trim_end_matches("state/").to_owned()
            } else if path.ends_with("/fill/") {
                path.replace("/fill/", "/")
            } else if path.ends_with("/post/") {
                path.trim_end_matches("post/").to_owned()
            } else if path.ends_with("/reverse/") && !path.contains("/receipts/") {
                path.trim_end_matches("reverse/").to_owned()
            } else if path.ends_with("/status/") && path.contains("/lists/") {
                path.trim_end_matches("status/").to_owned()
            } else if path.ends_with("/status/") && path.contains("/sales-tax/") {
                format!(
                    "{}/sales-tax/rates/",
                    path.split("/sales-tax/").next().unwrap()
                )
            } else if path.ends_with("/reminders/settings/") {
                path.trim_end_matches("settings/").to_owned()
            } else {
                path.clone()
            };
            let _ = fb_app::screens::render(&mut app, &form_path)
                .unwrap_or_else(|e| panic!("{name} initial form: {e}"));
            let token = app.tokens.mint(&path);
            let fields = post
                .iter()
                .map(|(k, v)| {
                    let mut v = v.as_str().unwrap().to_owned();
                    for (symbolic, raw) in &ids {
                        v = v.replace(&format!("{{{symbolic}}}"), raw);
                    }
                    let mut k=k.clone();
                    for (symbolic,raw) in &ids {k=k.replace(&format!("{{{symbolic}}}"),raw);}
                    (k, v)
                })
                .collect();
            let outcome = app
                .run(fb_app::Command::SubmitForm {
                    path: path.clone(),
                    token,
                    fields,
                })
                .unwrap_or_else(|e| panic!("{name} post: {e}"));
            let (html, status) = match outcome {
                fb_app::Outcome::Invalid { html } => (html, 400),
                fb_app::Outcome::Page { html, status } => (html, status),
                other => panic!("{name}: POST must render without mutation: {other:?}"),
            };
            fb_app::screens::Rendered {
                html,
                status,
                route: String::new(),
            }
        } else {
            fb_app::screens::render(&mut app, &path).unwrap_or_else(|e| panic!("{name}: {e}"))
        };
        if name=="company-edit" {
            let recorded=normalize::numbering::snapshot(&rendered.html,&rs_map).expect("numbering settings section");
            std::fs::write(out_dir.join("numbering-settings.norm.html"),&recorded).unwrap();
            let fixture=Path::new(env!("CARGO_MANIFEST_DIR")).join("recorded/numbering-settings.html");
            if std::env::var_os("FB_RECORD_NUMBERING_SCREEN").is_some() {std::fs::write(&fixture,&recorded).unwrap();}
            match std::fs::read_to_string(fixture) {
                Ok(expected) if expected==recorded=>println!("RECORDED MATCH numbering-settings"),
                _=>{failures+=1;println!("DIFF numbering-settings: recorded settings section changed");}
            }
        }
        let expected_status=screen["status"].as_u64().unwrap_or(200) as u16;
        let status_differs=rendered.status!=expected_status;
        if status_differs {eprintln!("DIFF {name}: Django status {expected_status}, Rust status {}",rendered.status);failures+=1;}
        std::fs::write(out_dir.join(format!("rust/{name}.html")), &rendered.html).unwrap();
        let django = std::fs::read_to_string(django_dir.join(format!("{name}.html"))).expect("django render");
        let django = if app.plugins { django } else { normalize::release::django_before(&django) };
        let normalize:fn(&str,&normalize::IdMap)->String=match screen["media"].as_str(){Some("csv")=>normalize::normalize_csv,Some("location")=>normalize::normalize_location,_=>normalize::normalize};
        let is_post = screen.get("post").is_some();
        let a = normalize(&django, &py_map.for_screen(&path, is_post));
        let a = if app.plugins { a } else { normalize::release::django_after(&a) };
        let b = normalize(&rendered.html, &rs_map.for_screen(&path, is_post));
        std::fs::write(out_dir.join(format!("rust/{name}.norm.html")), &b).unwrap();
        std::fs::write(out_dir.join(format!("rust/{name}.django.norm.html")), &a).unwrap();
        match normalize::first_difference(&a, &b) {
            None if status_differs => println!("DIFF {name}: response status differs"),
            None => println!("MATCH {name} ({} bytes normalized)", a.len()),
            Some(report) => {
                if !status_differs {failures += 1;}
                println!("DIFF {name}\n{report}");
            }
        }
    }
    if !app.plugins {
        println!("HIDDEN — {hidden} plugin screens refused by the release build (N14)");
    }
    println!("{} — {} screens, {failures} with differences", if failures == 0 { "SCREENS MATCH" } else { "SCREENS DIFFER" }, screens.as_object().unwrap().len());
    std::process::exit(if failures == 0 { 0 } else { 1 });
}

/// The Rust book's ids under the same symbolic names as `render_django.py` writes.
fn rust_ids(app: &mut fb_app::App, company_name: &str) -> BTreeMap<String, String> {
    let open = app.open.as_mut().unwrap();
    open.book
        .read(|tx| -> fb_db::rusqlite::Result<BTreeMap<String, String>> {
            let mut ids = BTreeMap::new();
            let company: String = tx.query_row("SELECT id FROM ledger_company WHERE name=?1", [company_name], |r| r.get(0))?;
            ids.insert("company".to_owned(), company.clone());
            let mut companies=tx.prepare("SELECT name,id FROM ledger_company")?;
            for row in companies.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (name,id)=row?;ids.insert(format!("company:{name}"),id);}
            let queries: [(&str, &str); 18] = [
                ("depositapplication", "SELECT i.number,d.id FROM prepayments_depositapplication d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("depositrefund", "SELECT i.number,d.id FROM prepayments_depositrefund d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("creditapplication", "SELECT i.number||':'||(SELECT count(*) FROM credits_creditapplication a WHERE a.credit_memo_id=d.credit_memo_id AND a.id<=d.id),d.id FROM credits_creditapplication d JOIN credits_creditmemo m ON m.id=d.credit_memo_id JOIN documents_documentidentity i ON i.id=m.identity_id WHERE d.company_id=?1"),
                ("deposit", "SELECT i.number,d.id FROM banking_deposit d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("estimate", "SELECT i.number,d.id FROM sales_estimate d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("bill", "SELECT i.number, d.id FROM payables_bill d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("billpayment", "SELECT i.number, d.id FROM payables_billpayment d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("check", "SELECT i.number, d.id FROM checks_check d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("creditmemo", "SELECT i.number, d.id FROM credits_creditmemo d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("creditrefund", "SELECT i.number, d.id FROM credits_creditrefund d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                ("vendor", "SELECT display_name, id FROM directory_vendor WHERE company_id=?1"),
                ("taxitem", "SELECT name, id FROM salestax_taxitem WHERE company_id=?1"),
                ("account", "SELECT code, id FROM ledger_account WHERE company_id=?1"),
                ("invoice", "SELECT i.number, inv.id FROM sales_invoice inv JOIN documents_documentidentity i ON i.id=inv.identity_id WHERE inv.company_id=?1"),
                ("customer", "SELECT display_name, id FROM directory_customer WHERE company_id=?1"),
                ("item", "SELECT code, id FROM directory_item WHERE company_id=?1"),
                ("journal", "SELECT printf('J%06d', number), id FROM ledger_journal WHERE company_id=?1"),
                ("receipt", "SELECT i.number, r.id FROM sales_receipt r JOIN documents_documentidentity i ON i.id=r.identity_id WHERE r.company_id=?1"),
            ];
            let mut events=tx.prepare("SELECT occurred_at FROM ledger_auditevent")?;
            for row in events.query_map([],|r|r.get::<_,String>(0))? {
                let value=row?;
                if let Ok(d)=chrono::DateTime::parse_from_rfc3339(&value){let stamp=d.format("%Y-%m-%d %H:%M").to_string();ids.insert(format!("d5_event_stamp:{stamp}"),stamp);}
            }
            let mut apps=tx.prepare("SELECT i.number,a.id FROM credits_creditapplication a JOIN credits_creditmemo m ON m.id=a.credit_memo_id JOIN documents_documentidentity i ON i.id=m.identity_id WHERE a.company_id=?1 ORDER BY a.id")?;
            let mut seen=std::collections::HashMap::<String,usize>::new();
            for (n,row) in apps.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.enumerate(){let (number,_)=row?;let next=seen.entry(number.clone()).or_default();*next+=1;ids.insert(format!("label:creditapplication:{number}:{next}"),(n+1).to_string());}
            for (model, sql) in queries {
                let mut st = tx.prepare(sql)?;
                let mut rows=st.query_map([&company], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
                if model=="customer" || model=="vendor" {rows.sort_by(|a,b|a.1.cmp(&b.1));}
                let mut counts=std::collections::HashMap::new();for (key,_) in &rows{*counts.entry(key.clone()).or_insert(0)+=1;}
                for (n,(k,v)) in rows.into_iter().enumerate() {
                    let key=format!("{model}:{k}{}",if (model=="customer"||model=="vendor")&&counts[&k]>1{format!(":#{}",n+1)}else{String::new()});
                    if model=="customer" || model=="vendor" {ids.insert(format!("label:{key}"),(n+1).to_string());}
                    ids.insert(key,v);
                }
            }
            let mut reconciliations=tx.prepare("SELECT a.code || ':' || r.statement_date,r.id FROM banking_reconciliation r JOIN ledger_account a ON a.id=r.account_id WHERE r.company_id=?1")?;
            for row in reconciliations.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (key,id)=row?;ids.insert(format!("reconciliation:{key}"),id);}
            ids.insert("proposal_digest:current".into(),fb_core::first_run::propose(tx,fb_core::CompanyId::parse(&company).unwrap()).unwrap()["digest"].as_str().unwrap().into());
            let mut st=tx.prepare("SELECT occurred_at FROM ledger_auditevent")?;
            for stamp in st.query_map([],|r|r.get::<_,String>(0))? {
                let stamp=chrono::DateTime::parse_from_rfc3339(&stamp?).unwrap();
                let zone: chrono_tz::Tz = std::env::var("FB_TIME_ZONE").unwrap_or_else(|_| "America/New_York".into()).parse().expect("display time zone");
                let local=stamp.with_timezone(&zone).format("%Y-%m-%d %H:%M:%S").to_string();
                let utc=stamp.format("%Y-%m-%d %H:%M:%S").to_string();
                ids.insert(format!("document_stamp:{utc}"),utc.clone());
                ids.insert(format!("audit_local_stamp:{local}"),local);ids.insert(format!("audit_utc_stamp:{utc}"),utc);
            }
            // Every company's posted journals: a screen may show another scenario company's journal (J-ATTACH).
            let mut st=tx.prepare("SELECT number,posted_at FROM ledger_journal WHERE posted_at IS NOT NULL")?;
            for row in st.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))? {
                let (number,stamp)=row?;let stamp=chrono::DateTime::parse_from_rfc3339(&stamp).unwrap();
                ids.insert(format!("posted_stamp:J{number:06}"),stamp.format("%Y-%m-%d %H:%M:%S UTC").to_string());
            }
            ids.insert("user:owner".into(),tx.query_row("SELECT id FROM fb_user WHERE username='owner'",[],|r|r.get(0))?);
            for (table,model,column) in [("ledger_account","account","code"),("directory_item","item","code")] {
                let mut st=tx.prepare(&format!("SELECT {column},id FROM {table} WHERE company_id=?1 ORDER BY id"))?;
                let rows=st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
                for (n,(key,_)) in rows.into_iter().enumerate(){ids.insert(format!("label:{model}:{key}"),(n+1).to_string());}
            }
            ids.insert("bulk_digest:current".into(),fb_core::salestax::bulk_preview(tx,fb_core::CompanyId::parse(&company).unwrap()).unwrap()["digest"].as_str().unwrap().into());
            let mut st=tx.prepare("SELECT id,name FROM ledger_company WHERE id<>?1")?;
            for row in st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
                let (co,name)=row?;
                for (model,table,column) in [("account","ledger_account","code"),("customer","directory_customer","display_name"),("item","directory_item","code"),("vendor","directory_vendor","display_name")] {
                    let mut st=tx.prepare(&format!("SELECT id,{column} FROM {table} WHERE company_id=?1 ORDER BY id"))?;
                    let rows=st.query_map([&co],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
                    let mut counts=std::collections::HashMap::new();for (_,value) in &rows{*counts.entry(value.clone()).or_insert(0)+=1;}
                    for (n,(id,value)) in rows.into_iter().enumerate(){let suffix=if ["customer","vendor"].contains(&model)&&counts[&value]>1{format!(":#{}",n+1)}else{String::new()};let key=format!("{model}:{name}:{value}{suffix}");ids.insert(format!("label:{key}"),(n+1).to_string());ids.insert(key,id);}
                }
            }

            let mut st=tx.prepare("SELECT id,name FROM ledger_company WHERE id<>?1")?;
            for row in st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {
                let (co,name)=row?;
                for (model,table) in [("deposit","banking_deposit"),("bill","payables_bill"),("billpayment","payables_billpayment"),("check","checks_check"),("creditmemo","credits_creditmemo"),("creditrefund","credits_creditrefund"),("receipt","sales_receipt"),("invoice","sales_invoice"),("transfer","transfers_transfer")] {
                    let mut st=tx.prepare(&format!("SELECT d.id,i.number FROM {table} d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"))?;
                    for row in st.query_map([&co],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (id,number)=row?;ids.insert(format!("{model}:{name}:{number}"),id);}
                }
            }
            let mut st=tx.prepare("SELECT id,number FROM ledger_journal WHERE company_id=?1 ORDER BY id")?;
            for (n,row) in st.query_map([&company],|r|r.get::<_,i64>(1))?.enumerate(){ids.insert(format!("label:journal:J{:06}",row?), (n+1).to_string());}
            let mut st=tx.prepare("SELECT id,number FROM documents_documentidentity WHERE company_id=?1")?;
            for row in st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?{let (id,number)=row?;ids.insert(format!("identity:{number}"),id);}
            let mut st=tx.prepare("SELECT j.id,j.number,c.name FROM ledger_journal j JOIN ledger_company c ON c.id=j.company_id WHERE j.company_id<>?1 ORDER BY j.company_id,j.id")?;
            for row in st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?)))?{let (id,n,name)=row?;ids.insert(format!("journal:{name}:J{n:06}"),id);}
            let mut st=tx.prepare("SELECT d.id,d.number,c.name FROM documents_documentidentity d JOIN ledger_company c ON c.id=d.company_id WHERE d.company_id<>?1")?;
            for row in st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?{let (id,n,name)=row?;ids.insert(format!("identity:{name}:{n}"),id);}
            for row in tx.prepare("SELECT c.name,j.number,j.id FROM ledger_journal j JOIN ledger_company c ON c.id=j.company_id WHERE j.company_id<>?1")?.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?)))? {
                let (name,number,id)=row?;ids.insert(format!("journal:{name}:J{number:06}"),id);
            }
            for row in tx.prepare("SELECT c.name,i.number,i.id FROM documents_documentidentity i JOIN ledger_company c ON c.id=i.company_id")?.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))? {
                let (company,number,id)=row?;ids.insert(format!("identity:{company}:{number}"),id);
            }
            for table in ["checks_checkrevision","payables_billrevision","payables_billpayment","payables_paymentreversal","transfers_transfer","transfers_transfervoid"] {
                for row in tx.prepare(&format!("SELECT created_at FROM {table}"))?.query_map([],|r|r.get::<_,String>(0))? {
                    let stamp=row?;let t=chrono::DateTime::parse_from_rfc3339(&stamp).unwrap();let plain=t.format("%Y-%m-%d %H:%M:%S").to_string();
                    ids.insert(format!("document_stamp:{plain}"),plain);
                    let display=fb_templates::django::default_datetime(&stamp);ids.insert(format!("document_default_stamp:{display}"),display);
                }
            }
            // D8 GL provenance includes bank deposits and customer-deposit refunds in other books.
            for (table,model) in [("banking_deposit","deposit"),("prepayments_depositrefund","depositrefund")] {
                let mut st=tx.prepare(&format!("SELECT d.id,i.number,c.name FROM {table} d JOIN documents_documentidentity i ON i.id=d.identity_id JOIN ledger_company c ON c.id=d.company_id WHERE d.company_id<>?1"))?;
                for row in st.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))? {
                    let (id,number,name)=row?;ids.insert(format!("{model}:{name}:{number}"),id);
                }
            }
            // D9: the same symbolic names as render_django.py writes for its two companies.
            for name in ["D9 feed books","D9 AI books"] {
            if let Some(d9)=tx.query_row("SELECT id FROM ledger_company WHERE name=?1",[name],|r|r.get::<_,String>(0)).optional()? {
                for (model,sql) in [
                    ("bfconnection","SELECT label,id FROM bankfeeds_connection WHERE company_id=?1"),
                    ("bfrule","SELECT name,id FROM bankfeeds_rule WHERE company_id=?1"),
                    ("bfaccount","SELECT f.remote_id,f.id FROM bankfeeds_account f JOIN bankfeeds_connection c ON c.id=f.connection_id WHERE c.company_id=?1"),
                    (&*format!("account:{name}"),"SELECT code,id FROM ledger_account WHERE company_id=?1"),
                    ("bfline","SELECT j.memo||':'||l.position,l.id FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE j.company_id=?1"),
                    (&*format!("check:{name}"),"SELECT i.number,d.id FROM checks_check d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                    (&*format!("transfer:{name}"),"SELECT i.number,d.id FROM transfers_transfer d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                    (&*format!("receipt:{name}"),"SELECT i.number,d.id FROM sales_receipt d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                    (&*format!("bill:{name}"),"SELECT i.number,d.id FROM payables_bill d JOIN documents_documentidentity i ON i.id=d.identity_id WHERE d.company_id=?1"),
                    ("attachment","SELECT original_name,id FROM attachments_attachment WHERE company_id=?1"),
                ] {
                    for row in tx.prepare(sql)?.query_map([&d9],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))? {let (key,id)=row?;ids.insert(format!("{model}:{key}"),id);}
                }
                for row in tx.prepare("SELECT uploaded_at FROM attachments_attachment WHERE company_id=?1 UNION SELECT deleted_at FROM attachments_attachment WHERE company_id=?1 AND deleted_at IS NOT NULL")?.query_map([&d9],|r|r.get::<_,String>(0))? {
                    let raw=row?;
                    if let Ok(t)=chrono::DateTime::parse_from_rfc3339(&raw){let text=t.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M").to_string();ids.insert(format!("attachment_stamp:{text}"),text);}
                }
            }
            }
            for row in tx.prepare("SELECT a.code,(SELECT l.id FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE l.account_id=a.id AND j.status='POSTED' ORDER BY j.date,j.id,l.position,l.id LIMIT 1) FROM ledger_account a WHERE a.company_id=?1")?.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?)))? {
                if let (code,Some(id))=row? {ids.insert(format!("line:first:{code}"),id);}
            }
            for row in tx.prepare("SELECT l.id,j.number,l.position FROM ledger_journalline l JOIN ledger_journal j ON j.id=l.journal_id WHERE j.company_id=?1")?.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?)))? {
                let (id,number,position)=row?;ids.insert(format!("line:J{number:06}:{position}"),id);
            }
            for row in tx.prepare("SELECT r.id,a.code,r.statement_date,c.name FROM banking_reconciliation r JOIN ledger_account a ON a.id=r.account_id JOIN ledger_company c ON c.id=r.company_id WHERE r.company_id<>?1")?.query_map([&company],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))? {
                let (id,code,date,name)=row?;ids.insert(format!("reconciliation:{name}:{code}:{date}"),id);
            }
            for row in tx.prepare("SELECT occurred_at FROM ledger_auditevent WHERE action IN('reconciliation.completed','reconciliation.undone')")?.query_map([],|r|r.get::<_,String>(0))? {
                let raw=row?;
                let parsed=chrono::DateTime::parse_from_rfc3339(&raw).unwrap();
                let stamp=parsed.format("%Y-%m-%d %H:%M").to_string();ids.insert(format!("reconciliation_stamp:{stamp}"),stamp);
                let stamp=parsed.format("%Y-%m-%dT%H:%M:%S+00:00").to_string();ids.insert(format!("reconciliation_csv_stamp:{stamp}"),stamp);
            }
            Ok(ids)
        })
        .expect("read ids")
}

