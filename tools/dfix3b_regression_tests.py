"""Install FIX3b Rust regression tests before applying the service fixes."""
from pathlib import Path
root = Path(__file__).resolve().parents[1]
path = root / 'crates/fb-app/tests/d7b_flow.rs'
text = path.read_text()
start = text.index('    let command=Command::SubmitForm {path,token:', text.index('fn undo_slices_revision'))
end = text.index('\n}\n#[test]', start)
text = text[:start] + '''    let before=fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap();
    for (revision, reason, message) in [
        ("0000000000002", "Synthetic correction", "This reconciliation changed after you opened it."),
        ("abc", "Synthetic correction", "Reload the page and try again."),
        ("", "Synthetic correction", "Reload the page and try again."),
        ("-1", "Synthetic correction", "Reload the page and try again."),
        ("1000000001", "Synthetic correction", "Reload the page and try again."),
        ("abc", "", "Reload the page and try again."),
        ("-1", "", "Reload the page and try again."),
    ] {
        let command=Command::SubmitForm {path:path.clone(),token:f.app.tokens.mint(&path),fields:fields(json!({"reconciliation":field(&html,"reconciliation"),"revision":revision,"undo_key":field(&html,"undo_key"),"reason":reason}))};
        let outcome=f.app.run_renderer(command).unwrap();
        assert_eq!(before,fb_db::test_support::snapshot(f.app.open_mut().unwrap().book.connection()).unwrap());
        let Outcome::Invalid {html}=outcome else {panic!("invalid revision admitted: {revision}")};
        assert!(html.contains(message), "{revision}: wrong validation message");
        assert_eq!(field(&html,"revision"),fb_core::py::py_truncate(revision,12));
    }''' + text[end:]
path.write_text(text)
path = root / 'crates/fb-core/src/banking/dfix3_undo_tests.rs'
with path.open('a') as out:
    out.write('''
#[test]
fn required_revisions_and_keys_precede_reason_lookup_and_retries() {
    let mut f=Fx::new();
    let r=f.begin("0.00");
    let r=f.finish(&r);
    let key=r["id"].as_str().unwrap();
    let before=f.snap();
    for revision in [None,Some(-1),Some(1_000_000_001),Some(i64::MAX)] {
        for reason in ["Synthetic correction", ""] {
            assert_eq!(undo_last(&mut f.book,f.u,f.c,key,&f.bank.to_db(),revision,&json!(reason),&json!(new_id())).unwrap_err().message(),"Reload the page and try again.");
            assert_eq!(before,f.snap());
        }
        assert_eq!(reconcile_now(&mut f.book,f.u,f.c,key,&f.bank.to_db(),revision,&r["completion_key"]).unwrap_err().message(),"Reload the page and try again.");
        assert_eq!(before,f.snap());
    }
    for (revision,message) in [(Some(0),"This reconciliation changed after you opened it. Reload the reconciliation history and try again."),(Some(1_000_000_000),"This reconciliation changed after you opened it. Reload the reconciliation history and try again.")] {
        assert_eq!(undo_last(&mut f.book,f.u,f.c,key,&f.bank.to_db(),revision,&json!("Synthetic correction"),&json!(new_id())).unwrap_err().message(),message);
        assert_eq!(before,f.snap());
    }
}
''')
print('Installed screen and core no-write regression tests')
