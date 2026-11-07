"""Append PRINT regressions to the existing encrypted-book fixtures."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
p=ROOT/'crates/fb-app/tests/d4_flow.rs'
p.write_text(p.read_text()+r'''

#[test]
fn print_invoice_uses_stored_revision_and_only_current_open_balance_without_writes() {
    let mut f = Fixture::new();
    let invoice = f.invoice();
    let target = format!("/c/{}/invoices/{invoice}/print/", f.c);
    let draft = render(&mut f.app,&target);
    assert!(draft.contains("Draft — not posted. No amount due."));
    assert!(!draft.contains("id=\"print-balance\""));
    f.app.run(Command::PostInvoice { company_id:f.c.clone(),invoice_id:invoice.clone(),expected_revision:1 }).unwrap();
    let before = f.journal_count();
    let open = render(&mut f.app,&target);
    assert!(open.contains("id=\"print-total\">100.00"),"{open}");
    assert!(open.contains("id=\"print-balance\">100.00"),"{open}");
    assert!(open.contains("Revision 2 of 2"));
    assert!(open.contains("Net 31"));
    let old = render(&mut f.app,&format!("/c/{}/invoices/{invoice}/revisions/1/print/",f.c));
    assert!(old.contains("Earlier version"));
    assert!(old.contains("Revision 1 of 2"));
    assert!(!old.contains("id=\"print-balance\""));
    assert_eq!(f.journal_count(),before);
    let unrelated = uuid::Uuid::new_v4();
    assert!(screens::render(&mut f.app,&format!("/c/{unrelated}/invoices/{invoice}/print/")).is_err());
    assert!(screens::render(&mut f.app,&format!("/c/{}/invoices/{invoice}/revisions/99/print/",f.c)).is_err());
    assert_eq!(f.journal_count(),before);
}
''')
p=ROOT/'crates/fb-app/tests/d8_flow.rs'
p.write_text(p.read_text()+r'''

#[test]
fn print_capability_is_single_use_and_bound_to_exact_report_filters_without_audit_writes() {
    let (mut app,c,_) = fixture();
    app.now=||chrono::DateTime::parse_from_rfc3339("2028-02-29T17:15:00Z").unwrap().with_timezone(&chrono::Utc);
    let before=audit(&mut app);
    let target=format!("/c/{c}/reports/profit-and-loss/?start=2028-01-01&end=2028-02-29&output=print&literal=%24");
    let page=screens::render(&mut app,&target).unwrap();
    assert_eq!(page.status,200);
    assert!(page.html.contains("Printed 2028-02-29 17:15 UTC"));
    assert!(page.html.contains("report-paper"));
    let token=page.html.split_once("name=\"fb_command_token\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    let cmd=|path:String,token:String|Command::SubmitForm{path,token,fields:vec![("action".into(),"desktop_print".into())]};
    assert!(app.run_renderer(cmd(target.replace("end=2028-02-29","end=2028-02-28"),token.clone())).is_err());
    // A token never authorizes a different filter set; the matching URL can still consume it.
    assert!(matches!(app.run_renderer(cmd(target.clone(),token)),Ok(Outcome::Done{..})));
    let page=screens::render(&mut app,&target).unwrap();
    let token=page.html.split_once("name=\"fb_command_token\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    let result=app.run_renderer(cmd(target.clone(),token.clone())).unwrap();
    let Outcome::Done{data}=result else {panic!()};
    assert_eq!(data["desktop_print"],true);
    assert_eq!(data["target"],target);
    assert!(data["print_token"].as_str().unwrap().len()>20);
    assert!(app.run_renderer(cmd(target,token)).is_err());
    assert_eq!(audit(&mut app),before);
    let normal=format!("/c/{c}/reports/profit-and-loss/");
    let token=app.tokens.mint(&normal);
    assert!(app.run_renderer(cmd(normal,token)).is_err());
    assert_eq!(audit(&mut app),before);
    for report in ["profit-and-loss","balance-sheet","general-ledger"] {
        let r=page(&mut app,&c,report,"end=2028-02-29&as_of=2028-02-29&output=print");
        assert_eq!(r.status,200);
        assert!(r.html.contains("id=\"print-now\""));
    }
}
'''.replace('let page=screens::render','let rendered=screens::render').replace('page.status','rendered.status').replace('page.html','rendered.html'))
print('PRINT: encrypted-book current/historical invoice, access and print capability regressions added')
