"""Asserted PRINT integration edits; run once on the named FIX3 base."""
from pathlib import Path
import json
ROOT=Path(__file__).resolve().parents[1]
def edit(name, old, new):
    p=ROOT/name
    text=p.read_text()
    assert text.count(old)==1,(name,old,text.count(old))
    p.write_text(text.replace(old,new))

edit('crates/fb-templates/src/lib.rs','mod d8;','mod d8;\nmod printing;')
edit('crates/fb-templates/src/lib.rs','    ("banking/registers/list.html",', ''.join(
    f'    ("{n}.html", include_str!("../templates/{n}.html")),\n' for n in [
        'sales/print/document','prepayments/print','banking/registers/print','banking/reconcile/report_print','ledger/reports/print'])+'    ("banking/registers/list.html",')
edit('crates/fb-templates/src/lib.rs','        d8::register(&mut env);','        d8::register(&mut env);\n        printing::register(&mut env);')
edit('crates/fb-app/src/screens.rs','mod d8;','mod d8;\nmod printing;')
edit('crates/fb-app/src/screens.rs','        _ if d4::is_route(&name)', '        _ if printing::is_document(&name) => printing::document(app, &route, target)?,\n        _ if d4::is_route(&name)')
edit('crates/fb-app/src/screens.rs','    let route = resolve(path).ok_or_else(|| AppError::NotFound(path.to_owned()))?;\n    match route.name.as_str()',
     '    if get(fields, "action") == Some("desktop_print") { return printing::submit(app, path, fields); }\n    let route = resolve(path).ok_or_else(|| AppError::NotFound(path.to_owned()))?;\n    match route.name.as_str()')
edit('crates/fb-app/src/screens/d5.rs','mod records;','pub(super) mod records;')
edit('crates/fb-app/src/session.rs','    pub today: fn() -> chrono::NaiveDate,','    pub today: fn() -> chrono::NaiveDate,\n    /// Presentation instant for print generation; frozen alongside today by screen tooling.\n    pub now: fn() -> chrono::DateTime<chrono::Utc>,')
edit('crates/fb-app/src/session.rs','            today: local_today,','            today: local_today,\n            now: chrono::Utc::now,')
edit('crates/fb-screendiff/src/main.rs','    let ids = rust_ids(', '''    app.now = || {
        let clock: Value = serde_json::from_str(include_str!("../clock.json")).unwrap();
        chrono::DateTime::parse_from_rfc3339(clock["local_noon"].as_str().unwrap()).unwrap().with_timezone(&chrono::Utc)
    };
    let ids = rust_ids(''')
edit('crates/fb-app/src/screens/d8.rs','''    // The separate Print wave owns ledger/reports/print.html. Keep its route available for that wave.
    if output == "print" {
        return Ok((not_ported(app, route, path)?, 200));
    }''','''    if output == "print" {
        vm["title"] = json!(match route.name.as_str() { "profit_loss"=>"Profit and loss", "balance_sheet"=>"Balance sheet", _=>"General ledger" });
        vm["body"] = json!(template.replace(".html", "_body.html"));
        vm["generated"] = json!((app.now)().to_rfc3339());
        vm["screen_url"] = json!(format!("{path}?{}",with(params,Some(("output","")))));
        let target = format!("{path}?{}",urlencode(params));
        return super::printing::draw(app,"ledger/reports/print.html",&mut vm,&target,"internal");
    }''')
edit('crates/fb-app/src/screens/d7b.rs','''    if output == "print" {
        return Ok((
            not_ported(app, r, &rec_url("reconcile:report", c, key))?,
            200,
        ));
    }''','''    if output == "print" {
        let path = rec_url("reconcile:report",c,key);
        vm["screen_url"] = json!(format!("{path}?view={}",s(&vm,"view")));
        vm["generated"] = json!((app.now)().to_rfc3339());
        let target = format!("{path}?{}",urlencode(p));
        return super::printing::draw(app,"banking/reconcile/report_print.html",&mut vm,&target,"internal");
    }''')
edit('crates/fb-app/src/screens/d7b.rs','''    if output == "print" {
        return Ok((not_ported(app, r, _path)?, 200));
    }''','''    if output == "print" {
        for k in ["rows","totals","labels","opening","closing","total"] { vm[k]=result[k].clone(); }
        vm["start"]=data["start"].clone();vm["end"]=data["end"].clone();
        let filters=filter_text(&data);
        vm["filtered"]=json!(!filters.is_empty());vm["filter_text"]=json!(filters);
        vm["generated"]=json!((app.now)().to_rfc3339());
        let query=register_query(&FILTERS.iter().map(|k|(k.to_string(),s(&data,k).into())).collect::<Vec<_>>(),false);
        vm["screen_url"]=json!(if query.is_empty(){_path.to_owned()}else{format!("{_path}?{query}")});
        let target=format!("{_path}?{}",urlencode(p));
        return super::printing::draw(app,"banking/registers/print.html",&mut vm,&target,"internal");
    }''')
print('PRINT: wired routes, report contexts and one shared frozen presentation clock')
